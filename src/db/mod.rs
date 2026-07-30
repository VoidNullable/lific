pub mod migrate;
pub mod models;
pub mod queries;

use crossbeam_queue::ArrayQueue;
use rusqlite::Connection;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use crate::error::LificError;

/// Number of read connections in the pool.
/// SQLite WAL mode supports unlimited concurrent readers.
const READ_POOL_SIZE: usize = 8;

/// Database pool with read/write splitting.
///
/// SQLite allows concurrent reads but only one writer at a time.
/// - Writes go through a single Mutex-protected connection.
/// - Reads pull from a lock-free pool of read-only connections.
/// - Readers never block each other. Readers never block writers.
#[derive(Clone)]
pub struct DbPool {
    writer: Arc<Mutex<Connection>>,
    readers: Arc<ArrayQueue<Connection>>,
    path: PathBuf,
}

/// RAII guard that returns the read connection to the pool on drop.
pub struct ReadConn {
    conn: Option<Connection>,
    pool: Arc<ArrayQueue<Connection>>,
}

impl std::ops::Deref for ReadConn {
    type Target = Connection;
    fn deref(&self) -> &Connection {
        self.conn.as_ref().unwrap()
    }
}

impl Drop for ReadConn {
    fn drop(&mut self) {
        if let Some(conn) = self.conn.take() {
            // Best-effort return to pool; if full, connection is dropped
            let _ = self.pool.push(conn);
        }
    }
}

impl DbPool {
    /// Acquire a read-only connection from the pool.
    pub fn read(&self) -> Result<ReadConn, LificError> {
        match self.readers.pop() {
            Some(conn) => Ok(ReadConn {
                conn: Some(conn),
                pool: Arc::clone(&self.readers),
            }),
            None => {
                // Pool exhausted — open a fresh read connection
                let conn = open_read_connection(&self.path)?;
                Ok(ReadConn {
                    conn: Some(conn),
                    pool: Arc::clone(&self.readers),
                })
            }
        }
    }

    /// Acquire the exclusive write connection.
    ///
    /// LIF-155: stamps the current actor context (task-local set by the
    /// REST middleware / MCP wrapper / CLI default) onto `_actor_state`
    /// so the audit triggers attribute every write that follows. The
    /// exclusive guard makes the stamp race-free: nobody else can write
    /// between the stamp and the mutation.
    pub fn write(&self) -> Result<std::sync::MutexGuard<'_, Connection>, LificError> {
        let guard = self
            .writer
            .lock()
            .map_err(|e| LificError::Internal(format!("write lock poisoned: {e}")))?;
        crate::actor::stamp(&guard, &crate::actor::current());
        Ok(guard)
    }
}

fn apply_pragmas(conn: &Connection) -> Result<(), LificError> {
    conn.execute_batch(
        "PRAGMA journal_mode = WAL;
         PRAGMA synchronous = NORMAL;
         PRAGMA foreign_keys = ON;
         PRAGMA busy_timeout = 5000;
         PRAGMA cache_size = -8000;
         PRAGMA mmap_size = 67108864;",
    )?;
    // Match the read pool's statement-cache headroom so prepare_cached()
    // on the write connection (used for many reads in CLI/tests too) keeps
    // every distinct static query compiled. See open_read_connection().
    conn.set_prepared_statement_cache_capacity(64);
    Ok(())
}

/// Disable SQLite's memory-usage statistics before the first connection is
/// created. When memstatus is on (the default), every sqlite3 malloc/free in
/// the entire process serializes on one global mutex (`mem0`) — with many
/// threads (e.g. the parallel test runner) this becomes a futex storm that
/// burns more CPU in the kernel than the actual queries. We never read
/// sqlite3_memory_used(), so the stats are pure overhead. Must run before
/// SQLite initializes; once it has, the call returns SQLITE_MISUSE and is a
/// harmless no-op.
fn disable_sqlite_memstatus() {
    static ONCE: std::sync::Once = std::sync::Once::new();
    ONCE.call_once(|| unsafe {
        rusqlite::ffi::sqlite3_config(rusqlite::ffi::SQLITE_CONFIG_MEMSTATUS, 0i32);
    });
}

fn open_read_connection(path: &Path) -> Result<Connection, LificError> {
    let conn = Connection::open_with_flags(
        path,
        rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY
            | rusqlite::OpenFlags::SQLITE_OPEN_NO_MUTEX
            | rusqlite::OpenFlags::SQLITE_OPEN_URI,
    )?;
    conn.execute_batch(
        "PRAGMA journal_mode = WAL;
         PRAGMA synchronous = NORMAL;
         PRAGMA foreign_keys = ON;
         PRAGMA busy_timeout = 5000;
         PRAGMA cache_size = -4000;
         PRAGMA mmap_size = 67108864;",
    )?;
    // Hold every distinct static read query the pool runs without LRU
    // eviction. The query layer leans on prepare_cached() to skip SQL
    // recompilation (~2µs/statement → ~80ns cache hit); rusqlite's default
    // capacity is 16, which a read connection can exceed across endpoints.
    conn.set_prepared_statement_cache_capacity(64);
    Ok(conn)
}

/// Hands out a distinct name to every test database. See the comment at the
/// `name` binding in `open_memory` for why this is a counter and not a clock.
#[cfg(test)]
static TEST_DB_SEQ: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

/// Create an in-memory database for testing, with the same writer + reader
/// pool shape as [`open`]. An anonymous `:memory:` database cannot be shared
/// across connections, so this opens a uniquely *named* one in shared-cache
/// mode instead, which every connection in the returned pool can reach.
#[cfg(test)]
pub fn open_memory() -> Result<DbPool, LificError> {
    disable_sqlite_memstatus();

    // Use a unique named in-memory DB so all connections share the same data.
    //
    // LIF-362: the name must be unique per pool, and a clock read is not a
    // safe way to get that. `cache=shared` means two pools that pick the same
    // name get the SAME database rather than two isolated ones, so a
    // collision has one test migrating while another prepares statements
    // against it: `DatabaseLocked` / "database schema is locked: main". This
    // was previously seeded from `SystemTime::now().as_nanos()`, which is
    // fine-grained enough on Linux to hide the problem and coarse enough on
    // Windows to lose the race in CI. Shared-cache in-memory databases are
    // scoped to the process, and every test in a binary shares one process,
    // so a process-local counter is collision-free by construction.
    let name = format!(
        "file:lific_test_{}?mode=memory&cache=shared",
        TEST_DB_SEQ.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
    );

    let writer = Connection::open_with_flags(
        &name,
        rusqlite::OpenFlags::SQLITE_OPEN_READ_WRITE
            | rusqlite::OpenFlags::SQLITE_OPEN_CREATE
            | rusqlite::OpenFlags::SQLITE_OPEN_URI,
    )?;
    writer.execute_batch("PRAGMA foreign_keys = ON;")?;
    migrate::run(&writer)?;

    let readers = ArrayQueue::new(READ_POOL_SIZE);
    for _ in 0..READ_POOL_SIZE {
        let conn = Connection::open_with_flags(
            &name,
            rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY | rusqlite::OpenFlags::SQLITE_OPEN_URI,
        )?;
        conn.execute_batch("PRAGMA foreign_keys = ON;")?;
        let _ = readers.push(conn);
    }

    Ok(DbPool {
        writer: Arc::new(Mutex::new(writer)),
        readers: Arc::new(readers),
        path: PathBuf::from(&name),
    })
}

/// Open (or create) the SQLite database, run migrations, and return a pool.
pub fn open(path: &Path) -> Result<DbPool, LificError> {
    disable_sqlite_memstatus();
    // Writer connection — runs migrations
    let writer = Connection::open(path)?;
    apply_pragmas(&writer)?;
    migrate::run(&writer)?;

    // LIF-155: clear any actor left over from a previous process (the
    // `_actor_state` row persists). Writes before the first request stamp
    // must read as 'system', not as whoever acted last before restart.
    crate::actor::stamp(
        &writer,
        &crate::actor::ActorCtx {
            user_id: None,
            transport: crate::actor::Transport::System,
        },
    );

    // Pre-fill read pool
    let readers = ArrayQueue::new(READ_POOL_SIZE);
    for _ in 0..READ_POOL_SIZE {
        let conn = open_read_connection(path)?;
        let _ = readers.push(conn);
    }

    Ok(DbPool {
        writer: Arc::new(Mutex::new(writer)),
        readers: Arc::new(readers),
        path: path.to_path_buf(),
    })
}

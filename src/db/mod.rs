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
    pub fn write(&self) -> Result<std::sync::MutexGuard<'_, Connection>, LificError> {
        self.writer
            .lock()
            .map_err(|e| LificError::Internal(format!("write lock poisoned: {e}")))
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
    Ok(())
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
    Ok(conn)
}

/// Create an in-memory database for testing.
/// Uses a single connection for both reads and writes since :memory:
/// databases are not shared across connections.
#[cfg(test)]
pub fn open_memory() -> Result<DbPool, LificError> {
    let conn = Connection::open_in_memory()?;
    conn.execute_batch(
        "PRAGMA journal_mode = WAL;
         PRAGMA foreign_keys = ON;",
    )?;
    migrate::run(&conn)?;

    drop(conn);

    // Use a unique named in-memory DB so all connections share the same data.
    let name = format!(
        "file:lific_test_{}?mode=memory&cache=shared",
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
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
    // Writer connection — runs migrations
    let writer = Connection::open(path)?;
    apply_pragmas(&writer)?;
    migrate::run(&writer)?;

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

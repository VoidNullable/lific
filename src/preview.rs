//! LIF-418: structured previews for container attachments.
//!
//! Some uploads are interesting for what is *inside* them rather than for
//! their bytes. A zip attached to a bug report is usually a set of logs, and a
//! SQLite file attached to one is usually a reproduction database. Downloading
//! either just to see what it holds is friction, so this module answers that
//! question server-side and the API returns it as JSON.
//!
//! Both parsers treat their input as hostile.
//!
//! The zip reader never inflates anything. It walks the central directory,
//! which is a flat list of fixed-layout records at the end of the file, and
//! reports names and sizes straight out of it. There is no decompression, so
//! there is no zip bomb: a 10 MB archive claiming a 4 TB uncompressed entry
//! costs us the same handful of microseconds as an empty one, and the
//! declared size is reported as the untrusted number it is.
//!
//! The SQLite reader opens a *copy* of the file with `immutable=1`, `mode=ro`
//! and `query_only`, and runs exactly two shapes of query: a `sqlite_master`
//! scan restricted to `type='table'`, and a `COUNT(*)` per table. Views and
//! triggers are skipped on purpose, since evaluating a view means executing
//! SQL an attacker wrote. The copy means even a bug in that reasoning cannot
//! reach the stored blob.

use serde::Serialize;

use crate::error::LificError;

/// Largest number of zip entries we enumerate. An archive with more is
/// reported truncated rather than paged: this is a preview, and a caller who
/// needs the full manifest should download the file.
pub const MAX_ZIP_ENTRIES: usize = 200;

/// Largest number of tables we count rows for, same reasoning.
pub const MAX_SQLITE_TABLES: usize = 200;

/// One entry in a zip's central directory. `size` and `compressed` are the
/// archive's own claims; nothing is inflated to verify them.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ZipEntry {
    pub name: String,
    pub size: u64,
    pub compressed: u64,
}

/// One table in a SQLite database, with its real row count.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct SqliteTable {
    pub name: String,
    pub rows: i64,
}

/// What `GET /api/attachments/{id}/preview` returns. `kind` is the
/// discriminator, so a client switches on one field.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(tag = "kind", rename_all = "lowercase")]
pub enum Preview {
    Zip {
        entries: Vec<ZipEntry>,
        total_entries: usize,
        truncated: bool,
    },
    Sqlite {
        tables: Vec<SqliteTable>,
    },
    /// No structured preview is available for this file. Not an error: most
    /// attachments are images.
    None,
}

/// Build the preview for a blob, choosing the parser by magic bytes.
///
/// The stored MIME is deliberately not consulted. A SQLite file uploaded
/// before `application/vnd.sqlite3` joined the allowlist may be recorded as
/// something else entirely, and the header is the authoritative answer either
/// way.
pub fn preview_bytes(bytes: &[u8]) -> Result<Preview, LificError> {
    if bytes.starts_with(crate::storage::SQLITE_MAGIC) {
        return sqlite_preview(bytes);
    }
    if bytes.len() >= 4 && bytes[0] == 0x50 && bytes[1] == 0x4B {
        return Ok(zip_preview(bytes));
    }
    Ok(Preview::None)
}

// ── ZIP central directory ────────────────────────────────────

const EOCD_SIG: u32 = 0x0605_4B50;
const EOCD64_LOCATOR_SIG: u32 = 0x0706_4B50;
const EOCD64_SIG: u32 = 0x0606_4B50;
const CENTRAL_FILE_SIG: u32 = 0x0201_4B50;

/// Read a little-endian u16/u32/u64 at `off`, or `None` when it would run off
/// the end. Every field access in this module goes through these, so a
/// truncated or lying archive produces a short entry list rather than a panic.
fn le_u16(bytes: &[u8], off: usize) -> Option<u16> {
    bytes
        .get(off..off + 2)
        .map(|s| u16::from_le_bytes([s[0], s[1]]))
}

fn le_u32(bytes: &[u8], off: usize) -> Option<u32> {
    bytes
        .get(off..off + 4)
        .map(|s| u32::from_le_bytes([s[0], s[1], s[2], s[3]]))
}

fn le_u64(bytes: &[u8], off: usize) -> Option<u64> {
    bytes.get(off..off + 8).map(|s| {
        u64::from_le_bytes([s[0], s[1], s[2], s[3], s[4], s[5], s[6], s[7]])
    })
}

/// Locate the end-of-central-directory record. It sits at the very end unless
/// the archive carries a trailing comment, which can be up to 64 KiB, so the
/// search window is bounded at `65557` bytes (22-byte record + max comment).
fn find_eocd(bytes: &[u8]) -> Option<usize> {
    let window = bytes.len().min(22 + 0xFFFF);
    let start = bytes.len() - window;
    // Scan backwards: the last match is the real record, since a nested zip
    // stored inside this one would leave an earlier one lying around.
    (start..=bytes.len().saturating_sub(22))
        .rev()
        .find(|&i| le_u32(bytes, i) == Some(EOCD_SIG))
}

/// Resolve `(central directory offset, declared entry count)`, following the
/// Zip64 locator when the classic 16-bit / 32-bit fields are saturated.
fn central_directory_location(bytes: &[u8], eocd: usize) -> Option<(usize, usize)> {
    let entries = le_u16(bytes, eocd + 10)? as u64;
    let offset = le_u32(bytes, eocd + 16)? as u64;

    if entries != u64::from(u16::MAX) && offset != u64::from(u32::MAX) {
        return Some((usize::try_from(offset).ok()?, usize::try_from(entries).ok()?));
    }

    // Zip64: a 20-byte locator immediately precedes the EOCD and points at
    // the real, 64-bit record.
    let locator = eocd.checked_sub(20)?;
    if le_u32(bytes, locator) != Some(EOCD64_LOCATOR_SIG) {
        return None;
    }
    let eocd64 = usize::try_from(le_u64(bytes, locator + 8)?).ok()?;
    if le_u32(bytes, eocd64) != Some(EOCD64_SIG) {
        return None;
    }
    let entries = le_u64(bytes, eocd64 + 32)?;
    let offset = le_u64(bytes, eocd64 + 48)?;
    Some((usize::try_from(offset).ok()?, usize::try_from(entries).ok()?))
}

/// Walk a zip's central directory and list what it contains.
///
/// An archive we cannot make sense of comes back as an empty entry list rather
/// than an error: the file is still a perfectly downloadable attachment, and a
/// preview endpoint that 500s on a weird zip is worse than one that shrugs.
pub fn zip_preview(bytes: &[u8]) -> Preview {
    let Some(eocd) = find_eocd(bytes) else {
        return Preview::Zip {
            entries: Vec::new(),
            total_entries: 0,
            truncated: false,
        };
    };
    let (mut cursor, declared) = match central_directory_location(bytes, eocd) {
        Some(location) => location,
        None => {
            return Preview::Zip {
                entries: Vec::new(),
                total_entries: 0,
                truncated: false,
            };
        }
    };

    let mut entries = Vec::new();
    let mut seen = 0usize;
    // `declared` is the archive's own claim, so it bounds the loop but does
    // not drive it: the signature check below is what actually terminates.
    while seen < declared.min(u32::MAX as usize) {
        if le_u32(bytes, cursor) != Some(CENTRAL_FILE_SIG) {
            break;
        }
        let Some(compressed) = le_u32(bytes, cursor + 20) else {
            break;
        };
        let Some(size) = le_u32(bytes, cursor + 24) else {
            break;
        };
        let Some(name_len) = le_u16(bytes, cursor + 28) else {
            break;
        };
        let Some(extra_len) = le_u16(bytes, cursor + 30) else {
            break;
        };
        let Some(comment_len) = le_u16(bytes, cursor + 32) else {
            break;
        };
        let name_start = cursor + 46;
        let Some(raw_name) = bytes.get(name_start..name_start + name_len as usize) else {
            break;
        };

        seen += 1;
        if entries.len() < MAX_ZIP_ENTRIES {
            entries.push(ZipEntry {
                name: sanitize_entry_name(raw_name),
                size: u64::from(size),
                compressed: u64::from(compressed),
            });
        }
        cursor = name_start + name_len as usize + extra_len as usize + comment_len as usize;
    }

    Preview::Zip {
        truncated: seen > entries.len(),
        total_entries: seen,
        entries,
    }
}

/// Entry names come from the archive, so they can hold control characters,
/// invalid UTF-8, or a kilobyte of padding. This is display text, never a
/// path: it is lossily decoded, stripped of control characters, and capped.
fn sanitize_entry_name(raw: &[u8]) -> String {
    String::from_utf8_lossy(raw)
        .chars()
        .filter(|c| !c.is_control())
        .take(255)
        .collect()
}

// ── SQLite ───────────────────────────────────────────────────

/// Open a copy of `bytes` as a read-only SQLite database and list its tables
/// with row counts.
///
/// The copy is the point. `immutable=1` already promises SQLite the file will
/// not change, which suppresses WAL and journal creation, but writing the copy
/// into a fresh temporary directory means the stored, content-addressed blob
/// is not even reachable from the connection.
pub fn sqlite_preview(bytes: &[u8]) -> Result<Preview, LificError> {
    use rusqlite::{Connection, OpenFlags};

    let dir = tempfile::tempdir()
        .map_err(|e| LificError::Internal(format!("preview scratch dir: {e}")))?;
    // A fixed, boring filename: the path goes into a SQLite URI, and a name we
    // chose cannot contain a `?` or `#` that would be parsed as URI syntax.
    let path = dir.path().join("preview.sqlite3");
    std::fs::write(&path, bytes)
        .map_err(|e| LificError::Internal(format!("preview scratch copy: {e}")))?;

    let uri = format!("file:{}?immutable=1&mode=ro", path.display());
    let conn = Connection::open_with_flags(
        uri,
        OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_URI,
    )
    .map_err(|_| LificError::BadRequest("not a readable sqlite database".into()))?;
    // Belt and braces on top of the read-only open: `query_only` rejects any
    // statement that would write, including one smuggled in through a trigger.
    conn.pragma_update(None, "query_only", true)
        .map_err(|_| LificError::BadRequest("not a readable sqlite database".into()))?;

    let names: Vec<String> = {
        let mut stmt = conn
            .prepare(
                "SELECT name FROM sqlite_master
                 WHERE type = 'table' AND name NOT LIKE 'sqlite_%'
                 ORDER BY name",
            )
            .map_err(|_| LificError::BadRequest("not a readable sqlite database".into()))?;
        let rows = stmt
            .query_map([], |row| row.get::<_, String>(0))
            .map_err(|_| LificError::BadRequest("not a readable sqlite database".into()))?;
        rows.take(MAX_SQLITE_TABLES)
            .collect::<Result<Vec<_>, _>>()
            .map_err(|_| LificError::BadRequest("not a readable sqlite database".into()))?
    };

    let mut tables = Vec::with_capacity(names.len());
    for name in names {
        // The identifier comes from the database being inspected, so it is
        // quoted rather than interpolated raw. A `"` inside a SQLite quoted
        // identifier is escaped by doubling it.
        let quoted = name.replace('"', "\"\"");
        let rows: i64 = conn
            .query_row(&format!("SELECT COUNT(*) FROM \"{quoted}\""), [], |row| {
                row.get(0)
            })
            // A table that will not count (corrupt page, unreadable virtual
            // table) is still worth listing; report it with an unknown count
            // of -1 rather than dropping the whole preview.
            .unwrap_or(-1);
        tables.push(SqliteTable { name, rows });
    }

    Ok(Preview::Sqlite { tables })
}

/// Programmatic container fixtures, shared with the API-level preview tests in
/// `api::attachments` so both layers assert against the same bytes.
#[cfg(test)]
pub(crate) mod fixtures {
    use super::CENTRAL_FILE_SIG;
    use super::EOCD_SIG;

    /// Build a minimal but real zip archive: stored (uncompressed) entries,
    /// local headers, central directory, EOCD. Written by hand so the test
    /// exercises our parser against the format, not against a library's
    /// round-trip of itself.
    pub(crate) fn build_zip(files: &[(&str, &[u8])]) -> Vec<u8> {
        let mut out = Vec::new();
        let mut central = Vec::new();

        for (name, data) in files {
            let offset = out.len() as u32;
            out.extend_from_slice(&0x0403_4B50u32.to_le_bytes()); // local sig
            out.extend_from_slice(&20u16.to_le_bytes()); // version needed
            out.extend_from_slice(&0u16.to_le_bytes()); // flags
            out.extend_from_slice(&0u16.to_le_bytes()); // method: stored
            out.extend_from_slice(&0u16.to_le_bytes()); // mod time
            out.extend_from_slice(&0u16.to_le_bytes()); // mod date
            out.extend_from_slice(&0u32.to_le_bytes()); // crc (unchecked here)
            out.extend_from_slice(&(data.len() as u32).to_le_bytes());
            out.extend_from_slice(&(data.len() as u32).to_le_bytes());
            out.extend_from_slice(&(name.len() as u16).to_le_bytes());
            out.extend_from_slice(&0u16.to_le_bytes()); // extra len
            out.extend_from_slice(name.as_bytes());
            out.extend_from_slice(data);

            central.extend_from_slice(&CENTRAL_FILE_SIG.to_le_bytes());
            central.extend_from_slice(&20u16.to_le_bytes()); // version made by
            central.extend_from_slice(&20u16.to_le_bytes()); // version needed
            central.extend_from_slice(&0u16.to_le_bytes()); // flags
            central.extend_from_slice(&0u16.to_le_bytes()); // method
            central.extend_from_slice(&0u16.to_le_bytes()); // mod time
            central.extend_from_slice(&0u16.to_le_bytes()); // mod date
            central.extend_from_slice(&0u32.to_le_bytes()); // crc
            central.extend_from_slice(&(data.len() as u32).to_le_bytes()); // compressed
            central.extend_from_slice(&(data.len() as u32).to_le_bytes()); // uncompressed
            central.extend_from_slice(&(name.len() as u16).to_le_bytes());
            central.extend_from_slice(&0u16.to_le_bytes()); // extra
            central.extend_from_slice(&0u16.to_le_bytes()); // comment
            central.extend_from_slice(&0u16.to_le_bytes()); // disk start
            central.extend_from_slice(&0u16.to_le_bytes()); // internal attrs
            central.extend_from_slice(&0u32.to_le_bytes()); // external attrs
            central.extend_from_slice(&offset.to_le_bytes());
            central.extend_from_slice(name.as_bytes());
        }

        let cd_offset = out.len() as u32;
        let cd_size = central.len() as u32;
        out.extend_from_slice(&central);
        out.extend_from_slice(&EOCD_SIG.to_le_bytes());
        out.extend_from_slice(&0u16.to_le_bytes()); // this disk
        out.extend_from_slice(&0u16.to_le_bytes()); // cd start disk
        out.extend_from_slice(&(files.len() as u16).to_le_bytes());
        out.extend_from_slice(&(files.len() as u16).to_le_bytes());
        out.extend_from_slice(&cd_size.to_le_bytes());
        out.extend_from_slice(&cd_offset.to_le_bytes());
        out.extend_from_slice(&0u16.to_le_bytes()); // comment len
        out
    }

    /// Serialize a tiny real SQLite database to bytes.
    pub(crate) fn build_sqlite() -> Vec<u8> {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("fixture.sqlite3");
        {
            let conn = rusqlite::Connection::open(&path).unwrap();
            conn.execute_batch(
                "CREATE TABLE widgets (id INTEGER PRIMARY KEY, name TEXT);
                 INSERT INTO widgets (name) VALUES ('a'), ('b'), ('c');
                 CREATE TABLE empty_shelf (id INTEGER PRIMARY KEY);
                 CREATE VIEW widget_names AS SELECT name FROM widgets;",
            )
            .unwrap();
        }
        std::fs::read(&path).unwrap()
    }
}

#[cfg(test)]
mod tests {
    use super::fixtures::{build_sqlite, build_zip};
    use super::*;

    #[test]
    fn zip_preview_lists_entries_with_sizes() {
        let zip = build_zip(&[("a.txt", b"hello"), ("dir/b.log", b"world!!")]);
        let Preview::Zip {
            entries,
            total_entries,
            truncated,
        } = zip_preview(&zip)
        else {
            panic!("expected a zip preview");
        };
        assert_eq!(total_entries, 2);
        assert!(!truncated);
        assert_eq!(
            entries,
            vec![
                ZipEntry {
                    name: "a.txt".into(),
                    size: 5,
                    compressed: 5,
                },
                ZipEntry {
                    name: "dir/b.log".into(),
                    size: 7,
                    compressed: 7,
                },
            ]
        );
    }

    #[test]
    fn zip_preview_caps_entries_and_flags_truncation() {
        let names: Vec<String> = (0..MAX_ZIP_ENTRIES + 5).map(|i| format!("f{i}.txt")).collect();
        let files: Vec<(&str, &[u8])> = names.iter().map(|n| (n.as_str(), &b"x"[..])).collect();
        let zip = build_zip(&files);

        let Preview::Zip {
            entries,
            total_entries,
            truncated,
        } = zip_preview(&zip)
        else {
            panic!("expected a zip preview");
        };
        assert_eq!(entries.len(), MAX_ZIP_ENTRIES);
        assert_eq!(total_entries, MAX_ZIP_ENTRIES + 5);
        assert!(truncated);
    }

    /// A "zip" whose central directory is gone must not panic or error. This
    /// is the shape a truncated upload takes.
    #[test]
    fn zip_preview_survives_a_headless_archive() {
        let mut zip = build_zip(&[("a.txt", b"hello")]);
        zip.truncate(10);
        let Preview::Zip {
            entries,
            total_entries,
            truncated,
        } = zip_preview(&zip)
        else {
            panic!("expected a zip preview");
        };
        assert!(entries.is_empty());
        assert_eq!(total_entries, 0);
        assert!(!truncated);
    }

    /// A record that claims a 64 KB filename in a 200-byte file must stop the
    /// walk, not index out of bounds.
    #[test]
    fn zip_preview_rejects_a_lying_name_length() {
        let mut zip = build_zip(&[("a.txt", b"hello")]);
        // Rewrite the central directory entry's name length to something the
        // buffer cannot possibly hold.
        let cd = zip
            .windows(4)
            .position(|w| w == CENTRAL_FILE_SIG.to_le_bytes())
            .unwrap();
        zip[cd + 28..cd + 30].copy_from_slice(&0xFFFFu16.to_le_bytes());
        let Preview::Zip { entries, .. } = zip_preview(&zip) else {
            panic!("expected a zip preview");
        };
        assert!(entries.is_empty());
    }

    #[test]
    fn sqlite_preview_lists_tables_with_row_counts() {
        let db = build_sqlite();
        let Preview::Sqlite { tables } = sqlite_preview(&db).unwrap() else {
            panic!("expected a sqlite preview");
        };
        assert_eq!(
            tables,
            vec![
                SqliteTable {
                    name: "empty_shelf".into(),
                    rows: 0,
                },
                SqliteTable {
                    name: "widgets".into(),
                    rows: 3,
                },
            ],
            "views and sqlite_ internals must not be listed"
        );
    }

    #[test]
    fn preview_bytes_dispatches_on_magic_not_mime() {
        assert!(matches!(
            preview_bytes(&build_sqlite()).unwrap(),
            Preview::Sqlite { .. }
        ));
        assert!(matches!(
            preview_bytes(&build_zip(&[("a", b"b")])).unwrap(),
            Preview::Zip { .. }
        ));
        assert_eq!(preview_bytes(b"just some text").unwrap(), Preview::None);
    }

    #[test]
    fn preview_serializes_with_a_kind_discriminator() {
        let json = serde_json::to_value(Preview::None).unwrap();
        assert_eq!(json, serde_json::json!({ "kind": "none" }));

        let json = serde_json::to_value(Preview::Zip {
            entries: vec![ZipEntry {
                name: "a.txt".into(),
                size: 5,
                compressed: 3,
            }],
            total_entries: 1,
            truncated: false,
        })
        .unwrap();
        assert_eq!(
            json,
            serde_json::json!({
                "kind": "zip",
                "entries": [{ "name": "a.txt", "size": 5, "compressed": 3 }],
                "total_entries": 1,
                "truncated": false,
            })
        );

        let json = serde_json::to_value(Preview::Sqlite {
            tables: vec![SqliteTable {
                name: "t".into(),
                rows: 2,
            }],
        })
        .unwrap();
        assert_eq!(
            json,
            serde_json::json!({ "kind": "sqlite", "tables": [{ "name": "t", "rows": 2 }] })
        );
    }

    /// A database whose tables would write on read (a trigger, a view) must
    /// not be able to. `query_only` plus the table-only filter means the
    /// preview never evaluates attacker-authored SQL.
    #[test]
    fn sqlite_preview_does_not_execute_attacker_sql() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("hostile.sqlite3");
        {
            let conn = rusqlite::Connection::open(&path).unwrap();
            conn.execute_batch(
                "CREATE TABLE loot (id INTEGER PRIMARY KEY);
                 CREATE VIEW boom AS SELECT 1/0;
                 CREATE TRIGGER t AFTER INSERT ON loot BEGIN
                     INSERT INTO loot (id) VALUES (NEW.id + 1);
                 END;",
            )
            .unwrap();
        }
        let bytes = std::fs::read(&path).unwrap();
        let Preview::Sqlite { tables } = sqlite_preview(&bytes).unwrap() else {
            panic!("expected a sqlite preview");
        };
        assert_eq!(
            tables,
            vec![SqliteTable {
                name: "loot".into(),
                rows: 0,
            }]
        );
    }

    #[test]
    fn sqlite_preview_rejects_garbage_wearing_the_header() {
        let mut bytes = crate::storage::SQLITE_MAGIC.to_vec();
        bytes.extend_from_slice(&[0xAB; 512]);
        assert!(sqlite_preview(&bytes).is_err());
    }
}

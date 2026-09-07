//! Local project portability. Archive data supplies neither SQL nor extraction destinations.
use std::collections::{BTreeMap, BTreeSet};
use std::fs::{File, OpenOptions};
use std::io::{BufReader, Read, Write};
use std::path::Path;

use rusqlite::{Connection, OptionalExtension, params};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use sha2::{Digest, Sha256};

use crate::db::{self, DbPool};
use crate::error::LificError;
use crate::storage::{AttachmentStore, valid_sha256};

const VERSION: u32 = 1;
const MAX_METADATA: u64 = 64 * 1024 * 1024;
const MAX_BLOB: u64 = 256 * 1024 * 1024;
const MAX_TOTAL: u64 = 2 * 1024 * 1024 * 1024;
const MAX_BLOBS: usize = 10_000;
const MAX_ROWS: usize = 200_000;
const MAX_EXPANDED: u64 = MAX_TOTAL + MAX_METADATA + 16 * 1024 * 1024;

type Result<T> = std::result::Result<T, LificError>;
type Row = Vec<Value>;
type IdMaps = BTreeMap<&'static str, BTreeMap<i64, i64>>;

fn invalid(message: impl Into<String>) -> LificError {
    LificError::BadRequest(format!("project archive: {}", message.into()))
}
fn io(error: std::io::Error) -> LificError {
    LificError::Internal(format!("project archive I/O: {error}"))
}

struct Spec {
    name: &'static str,
    columns: &'static str,
    scope: &'static str,
}
// This list, including column order, is format v1. No SELECT * and no schema dump.
const SPECS: &[Spec] = &[
    Spec {
        name: "projects",
        columns: "id,name,identifier,description,emoji,created_at,updated_at,sort_order",
        scope: "id = ?1",
    },
    Spec {
        name: "modules",
        columns: "id,project_id,name,description,status,created_at,updated_at,emoji",
        scope: "project_id = ?1",
    },
    Spec {
        name: "labels",
        columns: "id,project_id,name,color",
        scope: "project_id = ?1",
    },
    Spec {
        name: "folders",
        columns: "id,project_id,parent_id,name,sort_order",
        scope: "project_id = ?1",
    },
    Spec {
        name: "issues",
        columns: "id,project_id,sequence,title,description,status,priority,module_id,sort_order,start_date,target_date,created_at,updated_at,source,deleted_at",
        scope: "project_id = ?1",
    },
    Spec {
        name: "pages",
        columns: "id,project_id,folder_id,title,content,sort_order,created_at,updated_at,sequence,status,pinned,deleted_at",
        scope: "project_id = ?1",
    },
    Spec {
        name: "plans",
        columns: "id,project_id,sequence,issue_id,title,status,created_at,updated_at",
        scope: "project_id = ?1",
    },
    Spec {
        name: "plan_steps",
        columns: "id,plan_id,parent_step_id,position,title,description,issue_id,done,reopened_via_issue_at,created_at,edited_at",
        scope: "plan_id IN (SELECT id FROM plans WHERE project_id = ?1)",
    },
    Spec {
        name: "comments",
        columns: "id,issue_id,page_id,content,created_at,updated_at,deleted_at,imported_author",
        scope: "issue_id IN (SELECT id FROM issues WHERE project_id = ?1) OR page_id IN (SELECT id FROM pages WHERE project_id = ?1)",
    },
    Spec {
        name: "issue_labels",
        columns: "issue_id,label_id",
        scope: "issue_id IN (SELECT id FROM issues WHERE project_id = ?1)",
    },
    Spec {
        name: "page_labels",
        columns: "page_id,label_id",
        scope: "page_id IN (SELECT id FROM pages WHERE project_id = ?1)",
    },
    Spec {
        name: "issue_relations",
        columns: "source_id,target_id,relation_type",
        scope: "source_id IN (SELECT id FROM issues WHERE project_id = ?1) OR target_id IN (SELECT id FROM issues WHERE project_id = ?1)",
    },
    Spec {
        name: "page_issue_links",
        columns: "page_id,issue_id",
        scope: "page_id IN (SELECT id FROM pages WHERE project_id = ?1) OR issue_id IN (SELECT id FROM issues WHERE project_id = ?1)",
    },
    Spec {
        name: "attachment_links",
        columns: "attachment_id,entity_type,entity_id,created_at",
        scope: "(entity_type = 'issue' AND entity_id IN (SELECT id FROM issues WHERE project_id = ?1)) OR (entity_type = 'page' AND entity_id IN (SELECT id FROM pages WHERE project_id = ?1)) OR (entity_type = 'comment' AND entity_id IN (SELECT c.id FROM comments c LEFT JOIN issues i ON i.id = c.issue_id LEFT JOIN pages p ON p.id = c.page_id WHERE i.project_id = ?1 OR p.project_id = ?1))",
    },
    Spec {
        name: "attachments",
        columns: "id,sha256,filename,mime,size_bytes,created_at,width,height,alt_text,imported_author",
        scope: "id IN (SELECT attachment_id FROM attachment_links WHERE (entity_type = 'issue' AND entity_id IN (SELECT id FROM issues WHERE project_id = ?1)) OR (entity_type = 'page' AND entity_id IN (SELECT id FROM pages WHERE project_id = ?1)) OR (entity_type = 'comment' AND entity_id IN (SELECT c.id FROM comments c LEFT JOIN issues i ON i.id = c.issue_id LEFT JOIN pages p ON p.id = c.page_id WHERE i.project_id = ?1 OR p.project_id = ?1)))",
    },
    Spec {
        name: "audit_log",
        columns: "id,ts,transport,entity_type,entity_id,entity_label,project_id,issue_id,page_id,action,field,old_value,new_value,imported_author,imported_source",
        scope: "project_id = ?1 AND entity_type IN ('project','module','label','folder','issue','page','comment','plan','plan_step') AND (entity_type <> 'project' OR field IS NULL OR field IN ('name','identifier','description','emoji','sort_order'))",
    },
    Spec {
        name: "status_transitions",
        columns: "id,issue_id,from_status,to_status,transport,created_at,imported_author,imported_source",
        scope: "issue_id IN (SELECT id FROM issues WHERE project_id = ?1)",
    },
];

impl Spec {
    fn cols(&self) -> Vec<&'static str> {
        self.columns.split(',').collect()
    }
    fn index(&self, name: &str) -> usize {
        self.cols()
            .iter()
            .position(|c| *c == name)
            .expect("static archive column")
    }
    fn get<'a>(&self, row: &'a Row, name: &str) -> &'a Value {
        &row[self.index(name)]
    }
    fn set(&self, row: &mut Row, name: &str, value: Value) {
        row[self.index(name)] = value;
    }
}
fn spec(name: &str) -> Result<&'static Spec> {
    SPECS
        .iter()
        .find(|s| s.name == name)
        .ok_or_else(|| invalid("unknown table"))
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Table {
    name: String,
    #[serde(deserialize_with = "parse_rows")]
    rows: Vec<Row>,
}

// Reject nested objects/arrays while parsing, before constructing a Value tree.
// Fixed row widths and row counts also bound Vec overhead for tiny JSON inputs.
fn parse_rows<'de, D: serde::Deserializer<'de>>(d: D) -> std::result::Result<Vec<Row>, D::Error> {
    use serde::de::{Error, SeqAccess, Visitor};
    struct Scalar(Value);
    impl<'de> Deserialize<'de> for Scalar {
        fn deserialize<D: serde::Deserializer<'de>>(d: D) -> std::result::Result<Self, D::Error> {
            struct V;
            impl<'de> Visitor<'de> for V {
                type Value = Scalar;
                fn expecting(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
                    f.write_str("a scalar database value")
                }
                fn visit_unit<E: Error>(self) -> std::result::Result<Scalar, E> {
                    Ok(Scalar(Value::Null))
                }
                fn visit_i64<E: Error>(self, v: i64) -> std::result::Result<Scalar, E> {
                    Ok(Scalar(v.into()))
                }
                fn visit_u64<E: Error>(self, v: u64) -> std::result::Result<Scalar, E> {
                    let n = i64::try_from(v).map_err(E::custom)?;
                    Ok(Scalar(n.into()))
                }
                fn visit_f64<E: Error>(self, v: f64) -> std::result::Result<Scalar, E> {
                    serde_json::Number::from_f64(v)
                        .map(|n| Scalar(n.into()))
                        .ok_or_else(|| E::custom("nonfinite number"))
                }
                fn visit_str<E: Error>(self, v: &str) -> std::result::Result<Scalar, E> {
                    Ok(Scalar(v.into()))
                }
                fn visit_string<E: Error>(self, v: String) -> std::result::Result<Scalar, E> {
                    Ok(Scalar(v.into()))
                }
            }
            d.deserialize_any(V)
        }
    }
    struct ParsedRow(Row);
    impl<'de> Deserialize<'de> for ParsedRow {
        fn deserialize<D: serde::Deserializer<'de>>(d: D) -> std::result::Result<Self, D::Error> {
            struct V;
            impl<'de> Visitor<'de> for V {
                type Value = ParsedRow;
                fn expecting(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
                    f.write_str("a bounded row")
                }
                fn visit_seq<A: SeqAccess<'de>>(
                    self,
                    mut a: A,
                ) -> std::result::Result<ParsedRow, A::Error> {
                    let mut row = Vec::new();
                    while let Some(Scalar(v)) = a.next_element()? {
                        if row.len() == 15 {
                            return Err(A::Error::custom("too many columns"));
                        }
                        row.push(v);
                    }
                    if row.is_empty() {
                        return Err(A::Error::custom("empty row"));
                    }
                    Ok(ParsedRow(row))
                }
            }
            d.deserialize_seq(V)
        }
    }
    struct V;
    impl<'de> Visitor<'de> for V {
        type Value = Vec<Row>;
        fn expecting(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
            f.write_str("bounded project rows")
        }
        fn visit_seq<A: SeqAccess<'de>>(self, mut a: A) -> std::result::Result<Vec<Row>, A::Error> {
            let mut rows = Vec::new();
            while let Some(ParsedRow(row)) = a.next_element()? {
                if rows.len() == MAX_ROWS {
                    return Err(A::Error::custom("too many rows"));
                }
                rows.push(row);
            }
            Ok(rows)
        }
    }
    d.deserialize_seq(V)
}
#[derive(Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Blob {
    sha256: String,
    size: u64,
}
#[derive(Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Manifest {
    format_version: u32,
    exported_at: String,
    #[serde(deserialize_with = "parse_tables")]
    tables: Vec<Table>,
    #[serde(deserialize_with = "parse_blobs")]
    blobs: Vec<Blob>,
    #[serde(deserialize_with = "parse_references")]
    external_references: Vec<String>,
}

fn parse_tables<'de, D: serde::Deserializer<'de>>(
    d: D,
) -> std::result::Result<Vec<Table>, D::Error> {
    use serde::de::{Error, SeqAccess, Visitor};
    struct V;
    impl<'de> Visitor<'de> for V {
        type Value = Vec<Table>;
        fn expecting(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
            f.write_str("bounded archive tables")
        }
        fn visit_seq<A: SeqAccess<'de>>(
            self,
            mut a: A,
        ) -> std::result::Result<Self::Value, A::Error> {
            let mut tables = Vec::new();
            let mut rows = 0usize;
            while let Some(table) = a.next_element::<Table>()? {
                rows += table.rows.len();
                if tables.len() == SPECS.len() || rows > MAX_ROWS {
                    return Err(A::Error::custom("too many tables or rows"));
                }
                tables.push(table);
            }
            Ok(tables)
        }
    }
    d.deserialize_seq(V)
}

fn bounded_list<'de, D: serde::Deserializer<'de>, T: Deserialize<'de>, const N: usize>(
    d: D,
) -> std::result::Result<Vec<T>, D::Error> {
    use serde::de::{Error, SeqAccess, Visitor};
    struct V<T, const N: usize>(std::marker::PhantomData<T>);
    impl<'de, T: Deserialize<'de>, const N: usize> Visitor<'de> for V<T, N> {
        type Value = Vec<T>;
        fn expecting(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
            f.write_str("a bounded list")
        }
        fn visit_seq<A: SeqAccess<'de>>(
            self,
            mut a: A,
        ) -> std::result::Result<Self::Value, A::Error> {
            let mut values = Vec::new();
            while let Some(value) = a.next_element()? {
                if values.len() == N {
                    return Err(A::Error::custom("too many list entries"));
                }
                values.push(value);
            }
            Ok(values)
        }
    }
    d.deserialize_seq(V::<T, N>(std::marker::PhantomData))
}

fn parse_blobs<'de, D: serde::Deserializer<'de>>(d: D) -> std::result::Result<Vec<Blob>, D::Error> {
    bounded_list::<D, Blob, MAX_BLOBS>(d)
}
fn parse_references<'de, D: serde::Deserializer<'de>>(
    d: D,
) -> std::result::Result<Vec<String>, D::Error> {
    bounded_list::<D, String, MAX_ROWS>(d)
}
impl Manifest {
    fn rows(&self, name: &str) -> &[Row] {
        &self
            .tables
            .iter()
            .find(|t| t.name == name)
            .expect("validated table")
            .rows
    }
}

#[derive(Debug, Serialize)]
pub struct Report {
    pub project: String,
    pub rows: usize,
    pub blobs: usize,
    pub external_references: Vec<String>,
}

fn number(value: &Value) -> Result<i64> {
    value
        .as_i64()
        .ok_or_else(|| invalid("expected an integer ID"))
}
fn text(value: &Value) -> Result<&str> {
    value.as_str().ok_or_else(|| invalid("expected text"))
}
fn sql_value(value: &Value) -> Result<rusqlite::types::Value> {
    use rusqlite::types::Value as Sql;
    match value {
        Value::Null => Ok(Sql::Null),
        Value::String(s) => Ok(Sql::Text(s.clone())),
        Value::Number(n) => n
            .as_i64()
            .map(Sql::Integer)
            .or_else(|| n.as_f64().map(Sql::Real))
            .ok_or_else(|| invalid("invalid number")),
        _ => Err(invalid("rows may contain only text, numbers, or null")),
    }
}

fn read_table(
    conn: &Connection,
    s: &Spec,
    project: i64,
    bytes: &mut u64,
    remaining_rows: &mut usize,
) -> Result<Table> {
    let columns = s.cols().into_iter().map(|c| {
        if c == "imported_author" {
            let actor = match s.name { "comments" => "user_id", "attachments" => "uploader_id", _ => "actor_user_id" };
            format!("COALESCE(imported_author, (SELECT COALESCE(NULLIF(display_name,''), username) || ' (imported)' FROM users WHERE id = {}.{actor}), 'Unknown author (imported)')", s.name)
        } else { c.to_string() }
    }).collect::<Vec<_>>().join(",");
    let sql = format!(
        "SELECT {columns} FROM {} WHERE ({}) ORDER BY rowid LIMIT {}",
        s.name,
        s.scope,
        MAX_ROWS + 1
    );
    let mut stmt = conn.prepare(&sql)?;
    let count = s.cols().len();
    let mut cursor = stmt.query([project])?;
    let mut rows = Vec::new();
    while let Some(row) = cursor.next()? {
        let mut values = Vec::with_capacity(count);
        for index in 0..count {
            use rusqlite::types::ValueRef;
            values.push(match row.get_ref(index)? {
                ValueRef::Null => Value::Null,
                ValueRef::Integer(v) => v.into(),
                ValueRef::Real(v) => serde_json::Number::from_f64(v)
                    .ok_or_else(|| invalid("nonfinite value"))?
                    .into(),
                ValueRef::Text(v) => {
                    *bytes += v.len() as u64;
                    if *bytes > MAX_METADATA {
                        return Err(invalid("metadata exceeds 64 MiB"));
                    }
                    std::str::from_utf8(v)
                        .map_err(|_| invalid("invalid UTF-8 in database"))?
                        .into()
                }
                ValueRef::Blob(_) => return Err(invalid("unexpected SQL blob")),
            });
        }
        rows.push(values);
        *remaining_rows = remaining_rows
            .checked_sub(1)
            .ok_or_else(|| invalid("too many rows"))?;
    }
    Ok(Table {
        name: s.name.into(),
        rows,
    })
}

fn table_for_entity(entity: &str) -> Result<&'static str> {
    match entity {
        "project" => Ok("projects"),
        "module" => Ok("modules"),
        "label" => Ok("labels"),
        "folder" => Ok("folders"),
        "issue" => Ok("issues"),
        "page" => Ok("pages"),
        "comment" => Ok("comments"),
        "plan" => Ok("plans"),
        "plan_step" => Ok("plan_steps"),
        _ => Err(invalid("unsupported entity type")),
    }
}

// Foreign keys which can legitimately point outside a project are detached on
// export and reported. On import every remaining reference must resolve locally.
fn foreign_keys(s: &Spec) -> Vec<(&'static str, &'static str, bool)> {
    let mut keys = Vec::new();
    for c in s.cols() {
        let target = match c {
            "project_id" => "projects",
            "module_id" => "modules",
            "label_id" => "labels",
            "folder_id" | "parent_id" => "folders",
            "plan_id" => "plans",
            "parent_step_id" => "plan_steps",
            "issue_id" | "source_id" | "target_id" => "issues",
            "page_id" => "pages",
            "attachment_id" => "attachments",
            _ => continue,
        };
        let optional = matches!(
            c,
            "module_id" | "folder_id" | "parent_id" | "parent_step_id"
        ) || (c == "issue_id" && matches!(s.name, "plans" | "plan_steps"));
        keys.push((c, target, optional));
    }
    keys
}

fn source_maps(m: &Manifest) -> Result<IdMaps> {
    let mut maps = IdMaps::new();
    for t in &m.tables {
        let s = spec(&t.name)?;
        if !s.cols().contains(&"id") {
            continue;
        }
        let mut ids = BTreeMap::new();
        for row in &t.rows {
            let id = number(s.get(row, "id"))?;
            if id <= 0 || ids.insert(id, id).is_some() {
                return Err(invalid("duplicate or nonpositive row ID"));
            }
        }
        maps.insert(s.name, ids);
    }
    Ok(maps)
}

fn detach_external(m: &mut Manifest) -> Result<()> {
    let maps = source_maps(m)?;
    let mut external = RewriteState::new(&m.external_references)?;
    for t in &mut m.tables {
        let s = spec(&t.name)?;
        if s.name == "audit_log" {
            continue;
        }
        let mut kept = Vec::new();
        for mut row in t.rows.drain(..) {
            let mut keep = true;
            for (column, target, nullable) in foreign_keys(s) {
                let v = s.get(&row, column);
                if v.is_null() {
                    continue;
                }
                let id = number(v)?;
                if !maps[target].contains_key(&id) {
                    external.record(&[
                        s.name,
                        ".",
                        column,
                        " references source ",
                        target,
                        " ID ",
                        &id.to_string(),
                        "; not imported",
                    ])?;
                    if nullable {
                        s.set(&mut row, column, Value::Null);
                    } else {
                        keep = false;
                    }
                }
            }
            if keep {
                kept.push(row);
            }
        }
        t.rows = kept;
    }
    m.external_references = external.references.into_iter().collect();
    Ok(())
}

fn collect_manifest(conn: &Connection, project: &str) -> Result<Manifest> {
    let id: i64 = conn
        .query_row(
            "SELECT id FROM projects WHERE identifier = ?1",
            [project],
            |r| r.get(0),
        )
        .optional()?
        .ok_or_else(|| invalid("project not found"))?;
    let mut m = Manifest {
        format_version: VERSION,
        exported_at: chrono::Utc::now().to_rfc3339(),
        tables: Vec::new(),
        blobs: Vec::new(),
        external_references: Vec::new(),
    };
    let mut bytes = 0;
    let mut remaining_rows = MAX_ROWS;
    for s in SPECS {
        m.tables
            .push(read_table(conn, s, id, &mut bytes, &mut remaining_rows)?);
    }
    detach_external(&mut m)?;
    let s = spec("attachments")?;
    let mut blobs = BTreeMap::new();
    for row in m.rows("attachments") {
        let hash = text(s.get(row, "sha256"))?.to_string();
        let size = s
            .get(row, "size_bytes")
            .as_u64()
            .ok_or_else(|| invalid("invalid blob size"))?;
        if let Some(previous) = blobs.insert(hash, size)
            && previous != size
        {
            return Err(invalid("conflicting blob sizes"));
        }
    }
    m.blobs = blobs
        .into_iter()
        .map(|(sha256, size)| Blob { sha256, size })
        .collect();
    validate_manifest(&m)?;
    let maps = source_maps(&m)?;
    let mut external = RewriteState::new(&m.external_references)?;
    for t in &m.tables {
        let s = spec(&t.name)?;
        for row in &t.rows {
            for column in ["description", "content", "old_value", "new_value"] {
                if s.cols().contains(&column)
                    && let Some(body) = s.get(row, column).as_str()
                {
                    remap_markdown(body, &maps, &mut external)?;
                }
            }
        }
    }
    m.external_references = external.references.into_iter().collect();
    validate_manifest(&m)?;
    Ok(m)
}

fn encode_manifest(m: &Manifest) -> Result<Vec<u8>> {
    struct Bounded(Vec<u8>);
    impl Write for Bounded {
        fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
            if self.0.len() as u64 + bytes.len() as u64 > MAX_METADATA {
                return Err(std::io::Error::other("metadata exceeds 64 MiB"));
            }
            self.0.extend_from_slice(bytes);
            Ok(bytes.len())
        }
        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }
    let mut out = Bounded(Vec::new());
    serde_json::to_writer(&mut out, m).map_err(|e| invalid(e.to_string()))?;
    Ok(out.0)
}

fn regular(path: &Path) -> Result<File> {
    if !std::fs::symlink_metadata(path)
        .map_err(io)?
        .file_type()
        .is_file()
    {
        return Err(invalid("input must be a regular file, not a symlink"));
    }
    let mut opts = OpenOptions::new();
    opts.read(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        opts.custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK);
    }
    let f = opts.open(path).map_err(io)?;
    if !f.metadata().map_err(io)?.is_file() {
        return Err(invalid("input is not a regular file"));
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        if f.metadata().map_err(io)?.nlink() != 1 {
            return Err(invalid("hard-linked inputs are not accepted"));
        }
    }
    Ok(f)
}

fn check_store(store: &AttachmentStore) -> Result<()> {
    match std::fs::symlink_metadata(store.dir()) {
        Ok(m) if !m.file_type().is_dir() => Err(invalid(
            "attachment storage must be a directory, not a symlink",
        )),
        Ok(_) => Ok(()),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(e) => Err(io(e)),
    }
}

fn verified_blob(path: &Path, size: u64, hash: &str) -> Result<Vec<u8>> {
    if size > MAX_BLOB {
        return Err(invalid("blob exceeds 256 MiB"));
    }
    let file = regular(path)?;
    if file.metadata().map_err(io)?.len() != size {
        return Err(invalid("blob size mismatch"));
    }
    let mut bytes = Vec::new();
    file.take(size + 1).read_to_end(&mut bytes).map_err(io)?;
    if bytes.len() as u64 != size || format!("{:x}", Sha256::digest(&bytes)) != hash {
        return Err(invalid("blob checksum mismatch"));
    }
    Ok(bytes)
}

fn append<W: Write>(tar: &mut tar::Builder<W>, name: &str, bytes: &[u8]) -> Result<()> {
    let mut header = tar::Header::new_ustar();
    header.set_size(bytes.len() as u64);
    header.set_mode(0o600);
    header.set_cksum();
    tar.append_data(&mut header, name, bytes).map_err(io)
}

fn report(m: &Manifest) -> Result<Report> {
    Ok(Report {
        project: text(spec("projects")?.get(&m.rows("projects")[0], "identifier"))?.to_string(),
        rows: m.tables.iter().map(|t| t.rows.len()).sum(),
        blobs: m.blobs.len(),
        external_references: m.external_references.clone(),
    })
}

pub fn export(pool: &DbPool, store: &AttachmentStore, project: &str, out: &Path) -> Result<Report> {
    check_store(store)?;
    store.with_lock(|store| {
        check_store(store)?;
        let conn = pool.read()?;
        let tx = conn.unchecked_transaction()?;
        let m = collect_manifest(&tx, project)?;
        let metadata = encode_manifest(&m)?;
        let parent = out
            .parent()
            .filter(|p| !p.as_os_str().is_empty())
            .unwrap_or_else(|| Path::new("."));
        let mut staged = tempfile::NamedTempFile::new_in(parent).map_err(io)?;
        {
            let gzip =
                flate2::write::GzEncoder::new(staged.as_file_mut(), flate2::Compression::default());
            let mut tar = tar::Builder::new(gzip);
            append(&mut tar, "manifest.json", &metadata)?;
            for blob in &m.blobs {
                let bytes = verified_blob(&store.path_for(&blob.sha256)?, blob.size, &blob.sha256)?;
                append(&mut tar, &format!("blobs/{}", blob.sha256), &bytes)?;
            }
            tar.into_inner().map_err(io)?.finish().map_err(io)?;
        }
        staged.as_file().sync_all().map_err(io)?;
        if staged.as_file().metadata().map_err(io)?.len() > MAX_TOTAL {
            return Err(invalid("compressed archive exceeds 2 GiB"));
        }
        tx.commit()?;
        staged.persist_noclobber(out).map_err(|e| io(e.error))?;
        sync_directory(parent)?;
        report(&m)
    })
}

#[cfg_attr(
    not(unix),
    expect(clippy::unnecessary_wraps, reason = "directory fsync is Unix-only")
)]
fn sync_directory(_path: &Path) -> Result<()> {
    #[cfg(unix)]
    File::open(_path).and_then(|f| f.sync_all()).map_err(io)?;
    Ok(())
}

fn validate_manifest(m: &Manifest) -> Result<()> {
    if m.external_references.len() > MAX_ROWS {
        return Err(invalid("too many external references"));
    }
    if m.format_version != VERSION {
        return Err(invalid("unsupported format version"));
    }
    if m.tables.len() != SPECS.len() {
        return Err(invalid("missing or extra tables"));
    }
    let mut seen = BTreeSet::new();
    let mut total_rows = 0usize;
    for t in &m.tables {
        let s = spec(&t.name)?;
        if !seen.insert(s.name) {
            return Err(invalid("duplicate table"));
        }
        total_rows += t.rows.len();
        if total_rows > MAX_ROWS {
            return Err(invalid("too many rows"));
        }
        for row in &t.rows {
            if row.len() != s.cols().len() {
                return Err(invalid("invalid column count"));
            }
            for (column, value) in s.cols().into_iter().zip(row) {
                sql_value(value)?;
                if value.is_null() {
                    continue;
                }
                let integer = column == "id"
                    || column.ends_with("_id")
                    || (s.name == "projects" && column == "sort_order")
                    || matches!(
                        column,
                        "sequence"
                            | "position"
                            | "done"
                            | "pinned"
                            | "width"
                            | "height"
                            | "size_bytes"
                    );
                let valid_type = if integer {
                    value.as_i64().is_some()
                } else if column == "sort_order" {
                    value.as_f64().is_some()
                } else {
                    value.is_string()
                };
                if !valid_type {
                    return Err(invalid(format!("invalid type for {}.{column}", s.name)));
                }
                if matches!(column, "done" | "pinned") && !matches!(value.as_i64(), Some(0 | 1)) {
                    return Err(invalid(format!("invalid boolean for {}.{column}", s.name)));
                }
                if s.name == "attachments"
                    && column == "mime"
                    && !crate::storage::ALLOWED_MIMES.contains(&text(value)?)
                {
                    return Err(invalid("unsupported attachment MIME type"));
                }
                if s.name == "attachments" && column == "filename" {
                    let filename = text(value)?;
                    if filename.len() > 1024 || filename.chars().any(char::is_control) {
                        return Err(invalid(
                            "attachment filename is too long or contains controls",
                        ));
                    }
                }
            }
        }
    }
    if m.rows("projects").len() != 1 {
        return Err(invalid("archive must contain exactly one project"));
    }
    let maps = source_maps(m)?;
    let project_id = number(&m.rows("projects")[0][0])?;
    let identifier = text(spec("projects")?.get(&m.rows("projects")[0], "identifier"))?;
    db::queries::validate_identifier(identifier)?;
    for t in &m.tables {
        let s = spec(&t.name)?;
        let mut sequences = BTreeSet::new();
        for row in &t.rows {
            if s.cols().contains(&"sequence") {
                let sequence = number(s.get(row, "sequence"))?;
                if sequence <= 0 || !sequences.insert(sequence) {
                    return Err(invalid("duplicate or invalid readable sequence"));
                }
            }
            if s.cols().contains(&"project_id") && number(s.get(row, "project_id"))? != project_id {
                return Err(invalid("cross-project row"));
            }
            if s.name == "audit_log" {
                table_for_entity(text(s.get(row, "entity_type"))?)?;
                continue;
            }
            for (column, target, _) in foreign_keys(s) {
                let v = s.get(row, column);
                if !v.is_null() && !maps[target].contains_key(&number(v)?) {
                    return Err(invalid("unresolved foreign key"));
                }
            }
            if s.name == "attachment_links" {
                let entity = text(s.get(row, "entity_type"))?;
                if !matches!(entity, "issue" | "page" | "comment") {
                    return Err(invalid("invalid attachment parent"));
                }
                if !maps[table_for_entity(entity)?].contains_key(&number(s.get(row, "entity_id"))?)
                {
                    return Err(invalid("unresolved attachment parent"));
                }
            }
            if s.name == "comments" {
                let issue = s.get(row, "issue_id");
                let page = s.get(row, "page_id");
                if issue.is_null() == page.is_null() {
                    return Err(invalid("comment must have exactly one parent"));
                }
            }
        }
    }
    // Parent chains must be acyclic and remain within their own plan.
    for (table, parent) in [("folders", "parent_id"), ("plan_steps", "parent_step_id")] {
        let s = spec(table)?;
        let rows: BTreeMap<i64, &Row> = m
            .rows(table)
            .iter()
            .map(|r| Ok((number(&r[0])?, r)))
            .collect::<Result<_>>()?;
        let mut completed = BTreeMap::<i64, usize>::new();
        for id in rows.keys() {
            let mut chain = BTreeSet::new();
            let mut order = Vec::new();
            let mut current = *id;
            let mut depth = 0;
            while !completed.contains_key(&current) {
                if !chain.insert(current) {
                    return Err(invalid("cyclic parent chain"));
                }
                order.push(current);
                if order.len() > 128 {
                    return Err(invalid("parent nesting exceeds 128 levels"));
                }
                let r = rows[&current];
                let p = s.get(r, parent);
                if p.is_null() {
                    break;
                }
                let p = number(p)?;
                if table == "plan_steps" && s.get(r, "plan_id") != s.get(rows[&p], "plan_id") {
                    return Err(invalid("step parent belongs to another plan"));
                }
                current = p;
            }
            if let Some(existing) = completed.get(&current) {
                depth = *existing;
            }
            for id in order.into_iter().rev() {
                depth += 1;
                if depth > 128 {
                    return Err(invalid("parent nesting exceeds 128 levels"));
                }
                completed.insert(id, depth);
            }
        }
    }
    if m.blobs.len() > MAX_BLOBS {
        return Err(invalid("too many blobs"));
    }
    let mut hashes = BTreeMap::new();
    let mut total = 0u64;
    for b in &m.blobs {
        if !valid_sha256(&b.sha256)
            || b.size > MAX_BLOB
            || hashes.insert(b.sha256.as_str(), b.size).is_some()
        {
            return Err(invalid("invalid or duplicate blob descriptor"));
        }
        total += b.size;
        if total > MAX_TOTAL {
            return Err(invalid("blobs exceed 2 GiB"));
        }
    }
    let s = spec("attachments")?;
    let mut used = BTreeSet::new();
    for r in m.rows("attachments") {
        let hash = text(s.get(r, "sha256"))?;
        if hashes.get(hash).copied() != s.get(r, "size_bytes").as_u64() {
            return Err(invalid("missing blob descriptor or size mismatch"));
        }
        used.insert(hash);
    }
    if used.len() != hashes.len() {
        return Err(invalid("unreferenced blob"));
    }
    let linked: BTreeSet<i64> = m
        .rows("attachment_links")
        .iter()
        .map(|r| number(&r[0]))
        .collect::<Result<_>>()?;
    if linked.len() != maps["attachments"].len() {
        return Err(invalid("unlinked attachment metadata"));
    }
    Ok(())
}

struct Staged {
    manifest: Manifest,
    dir: tempfile::TempDir,
}
fn stage(path: &Path) -> Result<Staged> {
    let file = regular(path)?;
    if file.metadata().map_err(io)?.len() > MAX_TOTAL {
        return Err(invalid("compressed archive exceeds 2 GiB"));
    }
    let gzip = flate2::bufread::GzDecoder::new(BufReader::new(file.take(MAX_TOTAL + 1)));
    let mut tar = tar::Archive::new(gzip.take(MAX_EXPANDED + 1));
    let dir = tempfile::tempdir().map_err(io)?;
    let mut manifest = None;
    let mut seen = BTreeSet::new();
    for entry in tar.entries().map_err(io)?.raw(true) {
        let mut entry = entry.map_err(io)?;
        if !entry.header().entry_type().is_file() {
            return Err(invalid("only regular file entries are accepted"));
        }
        let name = std::str::from_utf8(&entry.path_bytes())
            .map_err(|_| invalid("invalid entry name"))?
            .to_string();
        if !seen.insert(name.clone()) {
            return Err(invalid("duplicate archive entry"));
        }
        if seen.len() > MAX_BLOBS + 1 {
            return Err(invalid("too many archive entries"));
        }
        if name == "manifest.json" {
            if manifest.is_some() || seen.len() != 1 {
                return Err(invalid("manifest must be the first entry"));
            }
            if entry.size() > MAX_METADATA {
                return Err(invalid("metadata exceeds 64 MiB"));
            }
            let mut bytes = Vec::new();
            entry
                .by_ref()
                .take(MAX_METADATA + 1)
                .read_to_end(&mut bytes)
                .map_err(io)?;
            if bytes.len() as u64 > MAX_METADATA {
                return Err(invalid("metadata exceeds 64 MiB"));
            }
            let m: Manifest = serde_json::from_slice(&bytes)
                .map_err(|e| invalid(format!("invalid manifest: {e}")))?;
            validate_manifest(&m)?;
            manifest = Some(m);
        } else {
            let hash = name
                .strip_prefix("blobs/")
                .filter(|h| valid_sha256(h))
                .ok_or_else(|| invalid("unsafe or unknown archive path"))?;
            let m = manifest
                .as_ref()
                .ok_or_else(|| invalid("manifest must be first"))?;
            let b = m
                .blobs
                .iter()
                .find(|b| b.sha256 == hash)
                .ok_or_else(|| invalid("unexpected blob"))?;
            if entry.size() != b.size {
                return Err(invalid("blob size mismatch"));
            }
            let mut dest = OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(dir.path().join(hash))
                .map_err(io)?;
            let mut hasher = Sha256::new();
            let mut count = 0u64;
            let mut buf = [0u8; 64 * 1024];
            loop {
                let n = entry.read(&mut buf).map_err(io)?;
                if n == 0 {
                    break;
                }
                count += n as u64;
                if count > b.size {
                    return Err(invalid("blob exceeds declared size"));
                }
                hasher.update(&buf[..n]);
                dest.write_all(&buf[..n]).map_err(io)?;
            }
            if count != b.size || format!("{:x}", hasher.finalize()) != hash {
                return Err(invalid("blob checksum mismatch"));
            }
            dest.sync_all().map_err(io)?;
        }
    }
    // Consume the gzip trailer and reject hidden tar entries after its end marker.
    let mut rest = tar.into_inner();
    let mut tail = Vec::new();
    rest.by_ref()
        .take(1025)
        .read_to_end(&mut tail)
        .map_err(io)?;
    if tail.len() > 1024 || tail.iter().any(|b| *b != 0) || rest.limit() == 0 {
        return Err(invalid("unexpected trailing archive data"));
    }
    let mut compressed = rest.into_inner().into_inner();
    let mut byte = [0];
    if compressed.read(&mut byte).map_err(io)? != 0 {
        return Err(invalid("trailing compressed data or multiple gzip members"));
    }
    let m = manifest.ok_or_else(|| invalid("missing manifest"))?;
    if seen.len() != m.blobs.len() + 1 {
        return Err(invalid("missing blob"));
    }
    Ok(Staged { manifest: m, dir })
}

fn allocate_maps(conn: &Connection, m: &Manifest) -> Result<IdMaps> {
    let mut maps = source_maps(m)?;
    for (table, ids) in &mut maps {
        let mut next: i64 = conn.query_row(&format!("SELECT max(COALESCE((SELECT max(id) FROM {table}),0), COALESCE((SELECT seq FROM sqlite_sequence WHERE name = ?1),0))"), [table], |r| r.get(0))?;
        for value in ids.values_mut() {
            next = next
                .checked_add(1)
                .ok_or_else(|| invalid("ID space exhausted"))?;
            *value = next;
        }
    }
    Ok(maps)
}

/// Bound report growth and output appends across the entire export/import.
struct RewriteState {
    references: BTreeSet<String>,
    reference_bytes: usize,
    output_bytes: usize,
    max_references: usize,
    max_reference_bytes: usize,
    max_output_bytes: usize,
}

impl RewriteState {
    fn new(references: &[String]) -> Result<Self> {
        let mut state = Self {
            references: BTreeSet::new(),
            reference_bytes: 0,
            output_bytes: 0,
            max_references: MAX_ROWS,
            max_reference_bytes: MAX_METADATA as usize,
            max_output_bytes: MAX_METADATA as usize,
        };
        for reference in references {
            state.record(&[reference])?;
        }
        Ok(state)
    }

    fn record(&mut self, parts: &[&str]) -> Result<()> {
        let size = parts
            .iter()
            .try_fold(0usize, |n, part| n.checked_add(part.len()))
            .ok_or_else(|| invalid("external reference size overflow"))?;
        if size > self.max_reference_bytes {
            return Err(invalid("external reference byte limit exceeded"));
        }
        // This temporary is bounded even for a single malformed, enormous ID.
        // Duplicates remain legal after the count/total-byte budget is full.
        let message = parts.concat();
        if self.references.contains(&message) {
            return Ok(());
        }
        if self.references.len() >= self.max_references {
            return Err(invalid("too many external references"));
        }
        if size
            > self
                .max_reference_bytes
                .saturating_sub(self.reference_bytes)
        {
            return Err(invalid("external reference byte limit exceeded"));
        }
        self.reference_bytes += size;
        self.references.insert(message);
        Ok(())
    }

    fn append(&mut self, output: &mut String, part: &str) -> Result<()> {
        if part.len() > self.max_output_bytes.saturating_sub(self.output_bytes) {
            return Err(invalid("rewritten content byte limit exceeded"));
        }
        self.output_bytes += part.len();
        output.push_str(part);
        Ok(())
    }
}

fn remap_markdown(body: &str, maps: &IdMaps, external: &mut RewriteState) -> Result<String> {
    let needle = "/api/attachments/";
    let mut output = String::new();
    let mut rest = body;
    let mut scanned = 0;
    let mut absolute_token = false;
    let mut previous_slash = false;
    while let Some(index) = rest.find(needle) {
        let prefix = &rest[..index];
        let position = body.len() - rest.len() + index;
        // Scan each original character once, keeping URL context across multiple
        // references in the same URL. `//` covers any scheme casing and network
        // paths. Neither '=' (unquoted HTML attributes/query strings) nor '(' in
        // a URL path ends its context. Ambiguous tokens are preserved, not moved.
        for c in body[scanned..position].chars() {
            if c.is_whitespace() || matches!(c, '"' | '\'' | '<' | '>') {
                absolute_token = false;
                previous_slash = false;
            } else {
                absolute_token |= previous_slash && c == '/';
                previous_slash = c == '/';
            }
        }
        scanned = position;
        let absolute = absolute_token || previous_slash;
        external.append(&mut output, prefix)?;
        rest = &rest[index + needle.len()..];
        let digits = rest.bytes().take_while(u8::is_ascii_digit).count();
        if digits == 0 {
            external.append(&mut output, needle)?;
            continue;
        }
        let old = &rest[..digits];
        if absolute {
            external.record(&[
                "absolute attachment URL references source ID ",
                old,
                "; URL retained for manual review",
            ])?;
            external.append(&mut output, needle)?;
            external.append(&mut output, old)?;
        } else if let Some(new) = old
            .parse::<i64>()
            .ok()
            .and_then(|id| maps["attachments"].get(&id))
        {
            external.append(&mut output, needle)?;
            external.append(&mut output, &new.to_string())?;
        } else {
            // Never let a dangling source ID bind to an unrelated destination blob.
            external.record(&[
                "source attachment ID ",
                old,
                " is outside the archive; link disabled",
            ])?;
            external.append(&mut output, "/unresolved-source-attachment/")?;
            external.append(&mut output, old)?;
        }
        rest = &rest[digits..];
    }
    external.append(&mut output, rest)?;
    Ok(output)
}

fn imported_row(
    s: &Spec,
    original: &Row,
    maps: &IdMaps,
    external: &mut RewriteState,
) -> Result<Row> {
    let mut row = original.clone();
    if s.cols().contains(&"imported_author") {
        let author = s
            .get(original, "imported_author")
            .as_str()
            .unwrap_or("Unknown author");
        let author = if author.ends_with(" (imported)") {
            author.to_string()
        } else {
            format!("{author} (imported)")
        };
        s.set(&mut row, "imported_author", author.into());
    }
    for (column, target, _) in foreign_keys(s) {
        let v = s.get(original, column);
        if v.is_null() {
            continue;
        }
        let mapped = maps[target].get(&number(v)?);
        if s.name == "audit_log" && mapped.is_none() {
            s.set(&mut row, column, Value::Null);
        } else {
            s.set(
                &mut row,
                column,
                (*mapped.ok_or_else(|| invalid("unresolved ID"))?).into(),
            );
        }
    }
    if s.cols().contains(&"id") {
        s.set(
            &mut row,
            "id",
            maps[s.name][&number(s.get(original, "id"))?].into(),
        );
    }
    if s.name == "attachment_links" || s.name == "audit_log" {
        let table = table_for_entity(text(s.get(original, "entity_type"))?)?;
        let source = number(s.get(original, "entity_id"))?;
        // A purged historical entity has no destination ID. Zero is explicitly
        // non-live; the source record is retained in imported_source.
        s.set(
            &mut row,
            "entity_id",
            maps[table].get(&source).copied().unwrap_or(0).into(),
        );
    }
    if matches!(s.name, "audit_log" | "status_transitions") {
        if s.get(original, "imported_source").is_null() {
            let snapshot: BTreeMap<_, _> = s.cols().into_iter().zip(original.iter()).collect();
            s.set(
                &mut row,
                "imported_source",
                serde_json::to_string(&snapshot)
                    .map_err(|e| invalid(e.to_string()))?
                    .into(),
            );
        }
        s.set(&mut row, "transport", "imported".into());
    }
    for c in ["description", "content", "old_value", "new_value"] {
        if s.cols().contains(&c)
            && let Some(body) = s.get(&row, c).as_str()
        {
            let rewritten = remap_markdown(body, maps, external)?;
            s.set(&mut row, c, rewritten.into());
        }
    }
    Ok(row)
}

fn insert_row(conn: &Connection, s: &Spec, row: &Row) -> Result<()> {
    let placeholders = vec!["?"; row.len()].join(",");
    let values = row.iter().map(sql_value).collect::<Result<Vec<_>>>()?;
    conn.execute(
        &format!(
            "INSERT INTO {} ({}) VALUES ({placeholders})",
            s.name, s.columns
        ),
        rusqlite::params_from_iter(values),
    )?;
    Ok(())
}

fn rebuild_derived(conn: &Connection, project: i64, maps: &IdMaps) -> Result<()> {
    for table in ["issues", "pages", "comments", "status_transitions"] {
        for id in maps[table].values() {
            conn.execute("UPDATE sync_seq SET value = value + 1 WHERE id = 1", [])?;
            conn.execute(&format!("UPDATE {table} SET seq = (SELECT value FROM sync_seq WHERE id = 1) WHERE id = ?1"), [id])?;
        }
    }
    conn.execute("INSERT INTO search_index(title,body,entity_type,entity_id,project_id) SELECT title,description,'issue',id,project_id FROM issues WHERE project_id = ?1 AND deleted_at IS NULL", [project])?;
    conn.execute("INSERT INTO search_index(title,body,entity_type,entity_id,project_id) SELECT title,content,'page',id,project_id FROM pages WHERE project_id = ?1 AND deleted_at IS NULL", [project])?;
    conn.execute("INSERT INTO search_index(title,body,entity_type,entity_id,project_id) SELECT '',c.content,'comment',c.id,?1 FROM comments c LEFT JOIN issues i ON i.id = c.issue_id LEFT JOIN pages p ON p.id = c.page_id WHERE (i.project_id = ?1 OR p.project_id = ?1) AND c.deleted_at IS NULL AND (c.issue_id IS NULL OR i.deleted_at IS NULL) AND (c.page_id IS NULL OR p.deleted_at IS NULL)", [project])?;
    for id in maps["attachments"].values() {
        conn.execute("INSERT INTO attachments_fts(filename,extracted_text,attachment_id) SELECT filename,'',id FROM attachments WHERE id = ?1", [id])?;
    }
    Ok(())
}

pub fn import(
    pool: &DbPool,
    store: &AttachmentStore,
    archive: &Path,
    user: &str,
) -> Result<Report> {
    let staged = stage(archive)?;
    check_store(store)?;
    store.with_lock(|store| {
        check_store(store)?;
        let mut conn = pool.write()?;
        let tx = conn.transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
        let admin: i64 = tx.query_row("SELECT id FROM users WHERE username = ?1 COLLATE NOCASE AND is_admin = 1 AND is_active = 1 AND is_bot = 0", [user], |r| r.get(0))
            .optional()?.ok_or_else(|| invalid("--user must name an active destination human admin"))?;
        let m = &staged.manifest;
        let mut result = report(m)?;
        let collision: bool = tx.query_row("SELECT EXISTS(SELECT 1 FROM projects WHERE identifier = ?1 COLLATE NOCASE)", [&result.project], |r| r.get(0))?;
        if collision { return Err(invalid("project identifier already exists; import never merges or overwrites")); }
        let maps = allocate_maps(&tx, m)?;
        let project = maps["projects"][&number(&m.rows("projects")[0][0])?];
        let triggers = db::migrate::suspend_triggers(&tx)?;
        tx.execute_batch("PRAGMA defer_foreign_keys = ON")?;
        let mut external = RewriteState::new(&m.external_references)?;
        for s in SPECS {
            for original in m.rows(s.name) {
                insert_row(&tx, s, &imported_row(s, original, &maps, &mut external)?)?;
            }
        }
        // Preserve the source timestamp while assigning the destination lead.
        tx.execute("UPDATE projects SET lead_user_id = ?1 WHERE id = ?2", params![admin, project])?;
        result.external_references = external.references.into_iter().collect();
        tx.execute("INSERT INTO project_archive_provenance(project_id,format_version,source_project_id,exported_at,external_references) VALUES (?1,?2,?3,?4,?5)", params![project, VERSION, number(&m.rows("projects")[0][0])?, m.exported_at, serde_json::to_string(&result.external_references).map_err(|e| invalid(e.to_string()))?])?;
        rebuild_derived(&tx, project, &maps)?;
        let attachments = spec("attachments")?;
        for row in m.rows("attachments") {
            let mime = text(attachments.get(row,"mime"))?;
            let size = number(attachments.get(row,"size_bytes"))?;
            if db::queries::attachments::is_extractable(mime,size) {
                let hash = text(attachments.get(row,"sha256"))?;
                let bytes = verified_blob(&staged.dir.path().join(hash), u64::try_from(size).map_err(|_|invalid("negative attachment size"))?, hash)?;
                if let Ok(body) = std::str::from_utf8(&bytes) {
                    let id = maps["attachments"][&number(&row[0])?];
                    db::queries::attachments::set_extracted_text(&tx,id,body)?;
                }
            }
        }
        for sql in triggers { tx.execute_batch(&sql)?; }
        // Audit this new local grant, not an imported permission or owner action.
        let prior_actor: (Option<i64>, String) = tx.query_row(
            "SELECT user_id,transport FROM _actor_state WHERE id=1", [],
            |row| Ok((row.get(0)?,row.get(1)?)),
        )?;
        tx.execute("UPDATE _actor_state SET user_id=NULL,transport='cli' WHERE id=1", [])?;
        tx.execute("INSERT INTO project_members(project_id,user_id,role) VALUES (?1,?2,'lead')", params![project,admin])?;
        tx.execute("UPDATE _actor_state SET user_id=?1,transport=?2 WHERE id=1", params![prior_actor.0,prior_actor.1])?;
        let violations: i64 = tx.query_row("SELECT count(*) FROM pragma_foreign_key_check", [], |r| r.get(0))?;
        if violations != 0 { return Err(invalid("import would create invalid foreign keys")); }
        // Lock store before DB. Verify existing files without overwriting them;
        // make new files durable before committing their database rows.
        let mut added = Vec::new();
        let outcome = (|| {
            for blob in &m.blobs {
                let path = store.path_for(&blob.sha256)?;
                match std::fs::symlink_metadata(&path) {
                    Ok(_) => { verified_blob(&path, blob.size, &blob.sha256)?; }
                    Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                        let bytes = verified_blob(&staged.dir.path().join(&blob.sha256), blob.size, &blob.sha256)?;
                        added.push(blob.sha256.clone());
                        store.write_unlocked(&bytes)?;
                    }
                    Err(e) => return Err(io(e)),
                }
            }
            tx.commit()?;
            Ok(())
        })();
        if let Err(e) = outcome {
            // Never remove a pre-existing/shared blob, including a blob whose
            // metadata existed before import but whose file was missing.
            for hash in added {
                let referenced: bool = conn.query_row("SELECT EXISTS(SELECT 1 FROM attachments WHERE sha256 = ?1)", [&hash], |r| r.get(0)).unwrap_or(true);
                if !referenced { let _ = store.delete_unlocked(&hash); }
            }
            return Err(e);
        }
        Ok(result)
    })
}

#[cfg(test)]
mod tests;

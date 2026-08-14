//! Format-native, non-destructive config writers for `lific connect`.
//!
//! The cardinal rule: **never destroy a user's config.** Each writer reads the
//! existing file (if any), inserts or replaces only Lific's own entry under the
//! client's top-level key, and writes the rest back untouched. If a file exists
//! but doesn't parse (e.g. JSONC with comments, which OpenCode/Crush/Zed allow),
//! we refuse to modify it and hand back the snippet to merge by hand.
//!
//! JSON: `serde_json` round-trip, pretty-printed with a trailing newline.
//! TOML: `toml_edit` document round-trip — preserves the user's existing
//!       formatting and comments, setting only `[mcp_servers.lific]`.
//! YAML: `serde_yaml` round-trip (YAML comments are lost — this is called out
//!       in the per-client notes surfaced to the user).

use std::path::Path;

use super::clients::{CompiledEntry, Format};

/// What a write did.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Action {
    /// The file did not exist and was created.
    Created,
    /// The file existed and Lific's entry was inserted or replaced in place.
    Updated,
}

impl Action {
    pub fn as_str(self) -> &'static str {
        match self {
            Action::Created => "created",
            Action::Updated => "updated",
        }
    }
}

/// The result of a successful compile-to-text step (used by `--dry-run`, which
/// renders the *whole* file that would be written without touching disk).
pub struct Rendered {
    pub contents: String,
    pub action: Action,
}

/// Error from a writer that a caller should surface as a per-client failure
/// (and keep going with the other clients).
#[derive(Debug)]
pub struct WriteError {
    pub message: String,
    /// A snippet the user can paste to merge manually, when we refused to touch
    /// an unparseable file.
    pub manual_snippet: Option<String>,
}

impl WriteError {
    fn new(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
            manual_snippet: None,
        }
    }

    fn with_snippet(message: impl Into<String>, snippet: impl Into<String>) -> Self {
        Self {
            message: message.into(),
            manual_snippet: Some(snippet.into()),
        }
    }
}

impl std::fmt::Display for WriteError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.message)
    }
}

impl std::error::Error for WriteError {}

/// Render the full file contents that *would* be written for `entry` merged
/// into whatever currently exists at `path`, without writing anything. Used by
/// both `--dry-run` and the real write path (which then just writes the result).
pub fn render(path: &Path, format: Format, entry: &CompiledEntry) -> Result<Rendered, WriteError> {
    let existing = read_existing(path)?;
    let action = if existing.is_some() {
        Action::Updated
    } else {
        Action::Created
    };
    let existing_str = existing.as_deref().unwrap_or("");
    let contents = match format {
        Format::Json => render_json(existing_str, entry)?,
        Format::Toml => render_toml(existing_str, entry)?,
        Format::Yaml => render_yaml(existing_str, entry)?,
    };
    Ok(Rendered { contents, action })
}

/// Merge `entry` into the config at `path` and write it back, creating parent
/// directories as needed. Returns whether the file was created or updated.
pub fn write(path: &Path, format: Format, entry: &CompiledEntry) -> Result<Action, WriteError> {
    let rendered = render(path, format, entry)?;
    if let Some(parent) = path.parent()
        && !parent.as_os_str().is_empty()
    {
        std::fs::create_dir_all(parent)
            .map_err(|e| WriteError::new(format!("failed to create {}: {e}", parent.display())))?;
    }
    std::fs::write(path, &rendered.contents)
        .map_err(|e| WriteError::new(format!("failed to write {}: {e}", path.display())))?;
    Ok(rendered.action)
}

/// Read the file if present. `Ok(None)` = doesn't exist. An unreadable file is
/// an error (permissions, etc.).
fn read_existing(path: &Path) -> Result<Option<String>, WriteError> {
    match std::fs::read_to_string(path) {
        Ok(s) => Ok(Some(s)),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(WriteError::new(format!(
            "failed to read {}: {e}",
            path.display()
        ))),
    }
}

// ── JSON ─────────────────────────────────────────────────────

fn render_json(existing: &str, entry: &CompiledEntry) -> Result<String, WriteError> {
    // `existing` is "" when the file is absent; treat that as an empty object.
    let mut root: serde_json::Value = if existing.trim().is_empty() {
        serde_json::json!({})
    } else {
        serde_json::from_str(existing).map_err(|e| {
            WriteError::with_snippet(
                format!(
                    "existing config is not valid JSON ({e}); not modifying it. Merge this in by hand:"
                ),
                manual_json_snippet(entry),
            )
        })?
    };

    let serde_json::Value::Object(root_map) = &mut root else {
        return Err(WriteError::with_snippet(
            "existing config is not a JSON object; not modifying it. Merge this in by hand:",
            manual_json_snippet(entry),
        ));
    };

    // Get-or-create the top-level object (mcpServers / servers / mcp / ...).
    let top = root_map
        .entry(entry.top_key.clone())
        .or_insert_with(|| serde_json::json!({}));
    let serde_json::Value::Object(top_map) = top else {
        return Err(WriteError::with_snippet(
            format!(
                "existing `{}` is not an object; not modifying it. Merge this in by hand:",
                entry.top_key
            ),
            manual_json_snippet(entry),
        ));
    };

    // Insert/replace only our own server entry, preserving siblings.
    top_map.insert(entry.name.clone(), entry.value.clone());

    let mut out = serde_json::to_string_pretty(&root)
        .map_err(|e| WriteError::new(format!("failed to serialize JSON: {e}")))?;
    out.push('\n');
    Ok(out)
}

/// The minimal `{ "<top_key>": { "<name>": <value> } }` snippet for manual merge.
fn manual_json_snippet(entry: &CompiledEntry) -> String {
    let snippet = serde_json::json!({
        entry.top_key.clone(): { entry.name.clone(): entry.value.clone() }
    });
    serde_json::to_string_pretty(&snippet).unwrap_or_default()
}

// ── TOML (Codex) ─────────────────────────────────────────────

fn render_toml(existing: &str, entry: &CompiledEntry) -> Result<String, WriteError> {
    use toml_edit::{DocumentMut, Item, Table};

    let mut doc: DocumentMut = if existing.trim().is_empty() {
        DocumentMut::new()
    } else {
        existing.parse::<DocumentMut>().map_err(|e| {
            WriteError::with_snippet(
                format!("existing config.toml does not parse ({e}); not modifying it. Merge by hand:"),
                manual_toml_snippet(entry),
            )
        })?
    };

    // entry.top_key is the dotted table path, e.g. "mcp_servers.lific".
    let parts: Vec<&str> = entry.top_key.split('.').collect();

    // Build our leaf table from the compiled JSON object.
    let mut leaf = Table::new();
    let obj = entry
        .value
        .as_object()
        .ok_or_else(|| WriteError::new("internal: codex compiled value must be an object"))?;
    for (k, v) in obj {
        leaf.insert(k, json_to_toml_value(v)?);
    }

    // Descend/create the parent tables, marking intermediate tables implicit so
    // they render as `[mcp_servers.lific]` rather than an empty `[mcp_servers]`.
    let mut current = doc.as_table_mut();
    let last = parts.len() - 1;
    for part in &parts[..last] {
        let sub = current.entry(part).or_insert_with(|| {
            let mut t = Table::new();
            t.set_implicit(true);
            Item::Table(t)
        });
        current = sub.as_table_mut().ok_or_else(|| {
            WriteError::with_snippet(
                format!("existing `{part}` in config.toml is not a table; not modifying it. Merge by hand:"),
                manual_toml_snippet(entry),
            )
        })?;
    }
    current.insert(parts[last], Item::Table(leaf));

    Ok(doc.to_string())
}

/// Convert a JSON scalar/array/object (the compiled entry's shape) into a
/// toml_edit value. Codex entries carry strings, string arrays, and — for the
/// stdio agent token (LIFIC-18) — a nested `env` object of strings.
fn json_to_toml_value(v: &serde_json::Value) -> Result<toml_edit::Item, WriteError> {
    use toml_edit::{Array, Item, Value, value, InlineTable};
    match v {
        serde_json::Value::String(s) => Ok(value(s.as_str())),
        serde_json::Value::Bool(b) => Ok(value(*b)),
        serde_json::Value::Number(n) => {
            if let Some(i) = n.as_i64() {
                Ok(value(i))
            } else if let Some(f) = n.as_f64() {
                Ok(value(f))
            } else {
                Err(WriteError::new("unsupported TOML number"))
            }
        }
        serde_json::Value::Array(arr) => {
            let mut a = Array::new();
            for item in arr {
                match item {
                    serde_json::Value::String(s) => a.push(s.as_str()),
                    serde_json::Value::Bool(b) => a.push(*b),
                    other => {
                        return Err(WriteError::new(format!(
                            "unsupported TOML array element: {other}"
                        )));
                    }
                }
            }
            Ok(Item::Value(Value::Array(a)))
        }
        // A nested string-only object becomes an inline table, mirroring how
        // a tool's stdio `env = { LIFIC_TOKEN = "..." }` is conventionally
        // written. The key carries the token (LIFIC-18).
        serde_json::Value::Object(map) => {
            let mut inline = InlineTable::new();
            for (k, vv) in map {
                let s = vv.as_str().ok_or_else(|| {
                    WriteError::new(format!("unsupported TOML object value for `{k}`"))
                })?;
                inline.insert(k, Value::from(s.to_string()));
            }
            Ok(Item::Value(Value::InlineTable(inline)))
        }
        other => Err(WriteError::new(format!("unsupported TOML value: {other}"))),
    }
}

fn manual_toml_snippet(entry: &CompiledEntry) -> String {
    use toml_edit::{DocumentMut, Item, Table};
    // Render a standalone document containing only our table.
    let mut doc = DocumentMut::new();
    let parts: Vec<&str> = entry.top_key.split('.').collect();

    let mut leaf = Table::new();
    if let Some(obj) = entry.value.as_object() {
        for (k, v) in obj {
            if let Ok(item) = json_to_toml_value(v) {
                leaf.insert(k, item);
            }
        }
    }

    let mut current = doc.as_table_mut();
    let last = parts.len() - 1;
    for part in &parts[..last] {
        let sub = current.entry(part).or_insert_with(|| {
            let mut t = Table::new();
            t.set_implicit(true);
            Item::Table(t)
        });
        match sub.as_table_mut() {
            Some(t) => current = t,
            None => return doc.to_string(),
        }
    }
    current.insert(parts[last], Item::Table(leaf));
    doc.to_string()
}

// ── YAML (Goose) ─────────────────────────────────────────────

fn render_yaml(existing: &str, entry: &CompiledEntry) -> Result<String, WriteError> {
    let mut root: serde_yaml::Value = if existing.trim().is_empty() {
        serde_yaml::Value::Mapping(serde_yaml::Mapping::new())
    } else {
        serde_yaml::from_str(existing).map_err(|e| {
            WriteError::with_snippet(
                format!("existing config.yaml does not parse ({e}); not modifying it. Merge by hand:"),
                manual_yaml_snippet(entry),
            )
        })?
    };

    let serde_yaml::Value::Mapping(root_map) = &mut root else {
        return Err(WriteError::with_snippet(
            "existing config.yaml is not a mapping; not modifying it. Merge by hand:",
            manual_yaml_snippet(entry),
        ));
    };

    let top_key = serde_yaml::Value::String(entry.top_key.clone());
    let top = root_map
        .entry(top_key)
        .or_insert_with(|| serde_yaml::Value::Mapping(serde_yaml::Mapping::new()));
    let serde_yaml::Value::Mapping(top_map) = top else {
        return Err(WriteError::with_snippet(
            format!(
                "existing `{}` is not a mapping; not modifying it. Merge by hand:",
                entry.top_key
            ),
            manual_yaml_snippet(entry),
        ));
    };

    let value_yaml: serde_yaml::Value = serde_yaml::to_value(&entry.value)
        .map_err(|e| WriteError::new(format!("failed to convert value to YAML: {e}")))?;
    top_map.insert(serde_yaml::Value::String(entry.name.clone()), value_yaml);

    serde_yaml::to_string(&root)
        .map_err(|e| WriteError::new(format!("failed to serialize YAML: {e}")))
}

fn manual_yaml_snippet(entry: &CompiledEntry) -> String {
    let mut top = serde_yaml::Mapping::new();
    let value_yaml =
        serde_yaml::to_value(&entry.value).unwrap_or(serde_yaml::Value::Null);
    top.insert(serde_yaml::Value::String(entry.name.clone()), value_yaml);
    let mut root = serde_yaml::Mapping::new();
    root.insert(
        serde_yaml::Value::String(entry.top_key.clone()),
        serde_yaml::Value::Mapping(top),
    );
    serde_yaml::to_string(&serde_yaml::Value::Mapping(root)).unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cli::connect::clients::{ServerConfig, find_client};

    fn remote() -> ServerConfig {
        ServerConfig::remote("http://127.0.0.1:3456/mcp", "lific_sk-live-K")
    }

    fn tmp() -> std::path::PathBuf {
        std::env::temp_dir().join(format!(
            "lific-connect-writer-{}-{}",
            std::process::id(),
            // A per-call nonce so parallel tests don't collide.
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ))
    }

    #[test]
    fn json_creates_file_when_absent() {
        let dir = tmp();
        let path = dir.join("opencode.json");
        let entry = find_client("opencode").unwrap().compile(&remote());
        let action = write(&path, Format::Json, &entry).unwrap();
        assert_eq!(action, Action::Created);

        let written = std::fs::read_to_string(&path).unwrap();
        assert!(written.ends_with('\n'), "must end with a trailing newline");
        let v: serde_json::Value = serde_json::from_str(&written).unwrap();
        assert_eq!(v["mcp"]["lific"]["type"], "remote");
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn json_preserves_sibling_servers_and_unrelated_keys() {
        let dir = tmp();
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("opencode.json");
        // Pre-existing config with another MCP server and unrelated top keys.
        std::fs::write(
            &path,
            serde_json::to_string_pretty(&serde_json::json!({
                "theme": "dark",
                "mcp": {
                    "other": { "type": "remote", "url": "http://other" }
                }
            }))
            .unwrap(),
        )
        .unwrap();

        let entry = find_client("opencode").unwrap().compile(&remote());
        let action = write(&path, Format::Json, &entry).unwrap();
        assert_eq!(action, Action::Updated);

        let v: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
        // Unrelated key preserved verbatim.
        assert_eq!(v["theme"], "dark");
        // Sibling server preserved verbatim.
        assert_eq!(v["mcp"]["other"]["url"], "http://other");
        // Our entry added.
        assert_eq!(v["mcp"]["lific"]["url"], "http://127.0.0.1:3456/mcp");
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn json_replaces_existing_lific_entry_not_duplicates() {
        let dir = tmp();
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("opencode.json");
        std::fs::write(
            &path,
            serde_json::to_string_pretty(&serde_json::json!({
                "mcp": { "lific": { "type": "remote", "url": "http://stale" } }
            }))
            .unwrap(),
        )
        .unwrap();

        let entry = find_client("opencode").unwrap().compile(&remote());
        write(&path, Format::Json, &entry).unwrap();

        let v: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
        assert_eq!(v["mcp"]["lific"]["url"], "http://127.0.0.1:3456/mcp");
        // Exactly one lific key (object semantics guarantee no dup, but assert
        // the map has just the one server we expect).
        assert_eq!(v["mcp"].as_object().unwrap().len(), 1);
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn json_refuses_to_touch_unparseable_jsonc_and_returns_snippet() {
        let dir = tmp();
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("opencode.json");
        // JSONC with a comment — valid for OpenCode but not strict JSON.
        let original = "{\n  // my config\n  \"mcp\": {}\n}\n";
        std::fs::write(&path, original).unwrap();

        let entry = find_client("opencode").unwrap().compile(&remote());
        let err = write(&path, Format::Json, &entry).unwrap_err();
        assert!(err.manual_snippet.is_some(), "must hand back a snippet");
        let snippet = err.manual_snippet.unwrap();
        assert!(snippet.contains("lific"));
        // File must be byte-for-byte unchanged.
        assert_eq!(std::fs::read_to_string(&path).unwrap(), original);
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn toml_sets_only_our_table_and_preserves_comments() {
        let dir = tmp();
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("config.toml");
        let original = "# Codex config\nmodel = \"gpt-5\"\n\n[mcp_servers.other]\nurl = \"http://other\"\n";
        std::fs::write(&path, original).unwrap();

        let entry = find_client("codex").unwrap().compile(&remote());
        let action = write(&path, Format::Toml, &entry).unwrap();
        assert_eq!(action, Action::Updated);

        let written = std::fs::read_to_string(&path).unwrap();
        // Comment preserved.
        assert!(written.contains("# Codex config"), "comment must survive: {written}");
        // Unrelated top-level key preserved.
        assert!(written.contains("model = \"gpt-5\""));
        // Sibling server preserved.
        assert!(written.contains("[mcp_servers.other]"));
        // Our table added with the env-var token and no inline key.
        let doc: toml_edit::DocumentMut = written.parse().unwrap();
        assert_eq!(
            doc["mcp_servers"]["lific"]["url"].as_str(),
            Some("http://127.0.0.1:3456/mcp")
        );
        assert_eq!(
            doc["mcp_servers"]["lific"]["bearer_token_env_var"].as_str(),
            Some("LIFIC_API_KEY")
        );
        assert!(!written.contains("lific_sk-live-K"), "must not inline the key");
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn toml_creates_fresh_when_absent() {
        let dir = tmp();
        let path = dir.join("config.toml");
        let entry = find_client("codex").unwrap().compile(&remote());
        let action = write(&path, Format::Toml, &entry).unwrap();
        assert_eq!(action, Action::Created);
        let doc: toml_edit::DocumentMut =
            std::fs::read_to_string(&path).unwrap().parse().unwrap();
        assert_eq!(
            doc["mcp_servers"]["lific"]["bearer_token_env_var"].as_str(),
            Some("LIFIC_API_KEY")
        );
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn toml_refuses_unparseable_and_returns_snippet() {
        let dir = tmp();
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("config.toml");
        let original = "this is = = not valid toml [[[\n";
        std::fs::write(&path, original).unwrap();
        let entry = find_client("codex").unwrap().compile(&remote());
        let err = write(&path, Format::Toml, &entry).unwrap_err();
        assert!(err.manual_snippet.is_some());
        assert_eq!(std::fs::read_to_string(&path).unwrap(), original);
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn yaml_sets_extensions_lific_and_preserves_other_extensions() {
        let dir = tmp();
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("config.yaml");
        std::fs::write(
            &path,
            "GOOSE_MODEL: gpt-5\nextensions:\n  other:\n    type: stdio\n    cmd: foo\n",
        )
        .unwrap();

        let entry = find_client("goose").unwrap().compile(&remote());
        let action = write(&path, Format::Yaml, &entry).unwrap();
        assert_eq!(action, Action::Updated);

        let v: serde_yaml::Value =
            serde_yaml::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
        // Unrelated top key preserved.
        assert_eq!(v["GOOSE_MODEL"].as_str(), Some("gpt-5"));
        // Sibling extension preserved.
        assert_eq!(v["extensions"]["other"]["cmd"].as_str(), Some("foo"));
        // Ours added.
        assert_eq!(
            v["extensions"]["lific"]["type"].as_str(),
            Some("streamable_http")
        );
        assert_eq!(v["extensions"]["lific"]["uri"].as_str(), Some("http://127.0.0.1:3456/mcp"));
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn render_does_not_write_to_disk() {
        let dir = tmp();
        let path = dir.join("opencode.json");
        let entry = find_client("opencode").unwrap().compile(&remote());
        let rendered = render(&path, Format::Json, &entry).unwrap();
        assert_eq!(rendered.action, Action::Created);
        assert!(rendered.contents.contains("lific"));
        // Nothing was written.
        assert!(!path.exists());
    }
}

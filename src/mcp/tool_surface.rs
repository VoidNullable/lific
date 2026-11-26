//! The tool surface version advertised in `initialize` (LIF-521).
//!
//! Wrappers and agents that pin against Lific's tools read
//! `capabilities.experimental["dev.lific/toolSurface"]` instead of diffing
//! `tools/list`. It sits in capabilities rather than the instructions because
//! clients do not hand capabilities to the model, so it costs no context.
//!
//! The version is bumped by hand whenever any tool definition changes: a name,
//! description, schema, annotation or anything else `tools/list` returns. The
//! fingerprint is the SHA-256 of the canonical `tools/list` (see
//! [`fingerprint`]), and a test fails until both constants match the live
//! surface. Neither depends on the session: a repository binding changes the
//! instructions, never the tools.

use rmcp::model::{ExperimentalCapabilities, JsonObject};
use serde_json::Value;

/// Bump by one whenever [`TOOL_SURFACE_FINGERPRINT`] changes.
pub(crate) const TOOL_SURFACE_VERSION: u64 = 1;
/// SHA-256 of the canonical `tools/list`; see [`fingerprint`].
pub(crate) const TOOL_SURFACE_FINGERPRINT: &str =
    "sha256:62b52d20ff04978c6d0d3eb16e704fb80616232d81081ffbd6ce99e79cb14127";
/// Every released surface, oldest first. Append one entry per bump, so a
/// fingerprint cannot change in place without the version moving too.
#[cfg(test)]
const TOOL_SURFACE_HISTORY: &[(u64, &str)] = &[(
    1,
    "sha256:62b52d20ff04978c6d0d3eb16e704fb80616232d81081ffbd6ce99e79cb14127",
)];

/// The experimental capability key. Reverse-DNS, like the multi-instance
/// proxy's `dev.lific/instance` provenance key.
pub(crate) const TOOL_SURFACE_CAPABILITY: &str = "dev.lific/toolSurface";

/// `{"dev.lific/toolSurface": {"version": N, "fingerprint": "sha256:..."}}`.
pub(crate) fn experimental_capabilities() -> ExperimentalCapabilities {
    let mut surface = JsonObject::new();
    surface.insert("version".to_owned(), Value::from(TOOL_SURFACE_VERSION));
    surface.insert(
        "fingerprint".to_owned(),
        Value::from(TOOL_SURFACE_FINGERPRINT),
    );
    ExperimentalCapabilities::from([(TOOL_SURFACE_CAPABILITY.to_owned(), surface)])
}

/// `sha256:<hex>` of the tools sorted by name, each serialized as canonical
/// JSON: object keys sorted at every depth, no whitespace, serde_json's string
/// escaping and number formatting, joined into one JSON array.
#[cfg(test)]
pub(crate) fn fingerprint(tools: &[Value]) -> String {
    use sha2::{Digest, Sha256};

    let mut sorted: Vec<&Value> = tools.iter().collect();
    sorted.sort_by(|a, b| {
        let name = |tool: &Value| tool["name"].as_str().unwrap_or_default().to_owned();
        name(a).cmp(&name(b))
    });
    let mut canonical = String::from("[");
    for (index, tool) in sorted.into_iter().enumerate() {
        if index > 0 {
            canonical.push(',');
        }
        write_canonical(tool, &mut canonical);
    }
    canonical.push(']');
    format!("sha256:{:x}", Sha256::digest(canonical.as_bytes()))
}

/// Sorts keys explicitly instead of trusting `serde_json::Map`'s order, which
/// becomes insertion order if any crate in the build enables `preserve_order`.
#[cfg(test)]
fn write_canonical(value: &Value, out: &mut String) {
    match value {
        Value::Object(map) => {
            let mut keys: Vec<&String> = map.keys().collect();
            keys.sort();
            out.push('{');
            for (index, key) in keys.into_iter().enumerate() {
                if index > 0 {
                    out.push(',');
                }
                out.push_str(&Value::from(key.as_str()).to_string());
                out.push(':');
                write_canonical(&map[key], out);
            }
            out.push('}');
        }
        Value::Array(items) => {
            out.push('[');
            for (index, item) in items.iter().enumerate() {
                if index > 0 {
                    out.push(',');
                }
                write_canonical(item, out);
            }
            out.push(']');
        }
        scalar => out.push_str(&scalar.to_string()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rmcp::ServerHandler;

    fn live_tools(mcp: &crate::mcp::LificMcp) -> Vec<Value> {
        mcp.list_tool_names()
            .iter()
            .map(|name| serde_json::to_value(mcp.get_tool(name).unwrap()).unwrap())
            .collect()
    }

    /// The check that makes the version mean something: any change to any
    /// tool definition fails here until both constants move together.
    #[test]
    fn the_committed_fingerprint_matches_the_live_tool_surface() {
        let mcp = crate::mcp::LificMcp::new(crate::db::open_memory().unwrap());
        let tools = live_tools(&mcp);
        assert!(tools.len() > 20, "the real surface, not a stub");
        let live = fingerprint(&tools);
        assert_eq!(
            live, TOOL_SURFACE_FINGERPRINT,
            "tool surface changed: bump TOOL_SURFACE_VERSION and update \
             TOOL_SURFACE_FINGERPRINT to {live}, then append both to TOOL_SURFACE_HISTORY \
             (src/mcp/tool_surface.rs)"
        );
    }

    /// The test above catches a stale fingerprint, bumped version or not.
    /// This one catches the reverse: a new fingerprint under an old version.
    #[test]
    fn every_new_fingerprint_gets_its_own_version() {
        let (version, fingerprint) = *TOOL_SURFACE_HISTORY.last().unwrap();
        assert_eq!(
            (TOOL_SURFACE_VERSION, TOOL_SURFACE_FINGERPRINT),
            (version, fingerprint),
            "append ({TOOL_SURFACE_VERSION}, \"{TOOL_SURFACE_FINGERPRINT}\") to \
             TOOL_SURFACE_HISTORY; a changed fingerprint needs a bumped TOOL_SURFACE_VERSION"
        );
        for (index, (version, _)) in TOOL_SURFACE_HISTORY.iter().enumerate() {
            assert_eq!(
                *version,
                index as u64 + 1,
                "TOOL_SURFACE_HISTORY versions count up by one from 1"
            );
        }
        for (index, (_, fingerprint)) in TOOL_SURFACE_HISTORY.iter().enumerate() {
            assert!(
                !TOOL_SURFACE_HISTORY[..index]
                    .iter()
                    .any(|(_, earlier)| earlier == fingerprint),
                "a fingerprint appears twice in TOOL_SURFACE_HISTORY"
            );
        }
    }

    #[test]
    fn the_fingerprint_ignores_tool_order_and_key_order() {
        let a = serde_json::json!({ "name": "a", "description": "x", "inputSchema": { "type": "object" } });
        let b = serde_json::json!({ "inputSchema": { "type": "object" }, "name": "b" });
        let mut reordered = serde_json::Map::new();
        reordered.insert("inputSchema".into(), a["inputSchema"].clone());
        reordered.insert("description".into(), a["description"].clone());
        reordered.insert("name".into(), a["name"].clone());
        assert_eq!(
            fingerprint(&[a.clone(), b.clone()]),
            fingerprint(&[b.clone(), Value::Object(reordered)])
        );
        let mut changed = a.clone();
        changed["description"] = Value::from("y");
        assert_ne!(fingerprint(&[a, b.clone()]), fingerprint(&[changed, b]));
    }

    #[test]
    fn get_info_advertises_the_surface_in_experimental_capabilities() {
        let mcp = crate::mcp::LificMcp::new(crate::db::open_memory().unwrap());
        let info = serde_json::to_value(mcp.get_info()).unwrap();
        assert_eq!(
            info["capabilities"]["experimental"][TOOL_SURFACE_CAPABILITY],
            serde_json::json!({
                "version": TOOL_SURFACE_VERSION,
                "fingerprint": TOOL_SURFACE_FINGERPRINT,
            })
        );
        assert!(
            !info["instructions"]
                .as_str()
                .unwrap()
                .contains("toolSurface"),
            "the version costs no context: it stays out of the instructions"
        );
    }

    #[test]
    fn a_bound_session_advertises_the_same_surface() {
        let pool = crate::db::open_memory().unwrap();
        let unbound = crate::mcp::LificMcp::new(pool.clone());
        let bound = crate::mcp::LificMcp::new(pool).with_bound_project(Some("LIF".into()));
        assert_eq!(
            unbound.get_info().capabilities.experimental,
            bound.get_info().capabilities.experimental
        );
        assert_eq!(
            fingerprint(&live_tools(&unbound)),
            fingerprint(&live_tools(&bound))
        );
    }
}

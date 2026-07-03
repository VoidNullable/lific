//! LIF-201: enumeration-derived authorization completeness checks — design
//! decision #15 in LIF-DOC-7. "Enumeration-derived" means the manifests
//! below are checked against the *actual* set of REST routes / MCP tools
//! shipped in the binary (parsed from `api/mod.rs` source for REST, read at
//! runtime from the live `ToolRouter` for MCP), not hand-maintained lists
//! that can silently drift. Add a new `.route(...)` or `#[tool]` and forget
//! to classify it here, and `every_rest_route_is_classified` /
//! `every_mcp_tool_is_classified` fail with the exact method+path (or tool
//! name) that needs a manifest entry — the enforcement equivalent of an
//! exhaustive `match`.
//!
//! This module does **not** re-verify the actual runtime behavior (allow vs.
//! deny for each role) — that's the job of `authz.rs`'s unit tests, the
//! `authz_gating_tests` modules in `api/mod.rs` / `mcp/tools.rs`, and
//! `api/members.rs`'s tests. This module's only job is "is every surface
//! accounted for at all," which is a different (and easy to silently miss)
//! failure mode than "is the gate correct."

use std::collections::HashMap;

/// The role level a `Gated` surface checks. Mirrors `crate::authz`'s public
/// gates so the manifest below reads as "which function guards this."
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Gate {
    /// `authz::require_role(.., Role::Viewer)` (read access).
    Viewer,
    /// `authz::require_role(.., Role::Maintainer)` (content mutation).
    Maintainer,
    /// `authz::require_role(.., Role::Lead)` (project settings).
    Lead,
    /// `authz::require_structure_role` (modules/labels/folders).
    StructureRole,
    /// `authz::require_project_delete_role` (`DELETE` project / `delete`
    /// MCP tool with `resource_type="project"`).
    ProjectDelete,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Classification {
    /// Denies below `Gate`; enforced identically flag-on, and (for the
    /// legacy branch) reproduces pre-LIF-194 behavior — see `authz.rs`.
    Gated(Gate),
    /// Cross-project read: never denies, filters via
    /// `authz::visible_project_ids` instead (LIF-DOC-7's "visibility
    /// primitive").
    Filtered,
    /// One handler/tool multiplexes several sub-operations (REST: none;
    /// MCP: `delete`, `list_resources`, `manage_resource` dispatch on an
    /// enum-ish string argument) with *different* classifications per
    /// branch. The `&str` documents each branch's own gate so a reviewer
    /// doesn't have to re-derive it from source — the branches themselves
    /// are exercised by `authz_gating_tests` in both transports.
    Mixed(&'static str),
    /// Deliberately outside `authz.rs`'s project-role model — global
    /// account/instance surfaces, ownership-based checks, health/public
    /// endpoints. `&str` is the reason, checked only for non-emptiness (so
    /// a lazy `Exempt("")` doesn't slip through unreviewed).
    Exempt(&'static str),
}

// ── REST: source-derived route extraction ───────────────────────────

/// Extract every `(METHOD, PATH)` pair registered via `.route(...)` in the
/// given axum router source. Deliberately a small hand-rolled parser rather
/// than pulling in `regex` as a new dependency (LIF-201 is test-only code;
/// AGENTS.md scopes this issue to `src/` — a `dev-dependencies` bump is
/// avoidable here). Works because we control the source formatting: method
/// calls always appear as a bare identifier immediately followed by `(`
/// (`get(`, `.post(`, etc.), which can't collide with a handler function
/// name like `get_issue(` because the character after `get` there is `_`,
/// not `(`.
fn extract_rest_routes(src: &str) -> Vec<(String, String)> {
    const METHODS: [&str; 5] = ["get", "post", "put", "patch", "delete"];
    let mut routes = Vec::new();
    let bytes = src.as_bytes();
    let mut idx = 0;

    while let Some(rel) = src[idx..].find(".route(") {
        let start = idx + rel + ".route(".len();
        let mut depth = 1usize;
        let mut in_string = false;
        let mut i = start;
        while i < bytes.len() && depth > 0 {
            match bytes[i] {
                b'"' => in_string = !in_string,
                b'(' if !in_string => depth += 1,
                b')' if !in_string => depth -= 1,
                _ => {}
            }
            i += 1;
        }
        let call_body = &src[start..i.saturating_sub(1)];
        idx = i;

        let Some(q1) = call_body.find('"') else { continue };
        let Some(q2_rel) = call_body[q1 + 1..].find('"') else { continue };
        let path = call_body[q1 + 1..q1 + 1 + q2_rel].to_string();

        for method in METHODS {
            let needle = format!("{method}(");
            let mut search_from = 0;
            while let Some(pos) = call_body[search_from..].find(needle.as_str()) {
                let abs = search_from + pos;
                let word_start = abs == 0
                    || !call_body[..abs]
                        .chars()
                        .next_back()
                        .is_some_and(|c| c.is_alphanumeric() || c == '_');
                if word_start {
                    routes.push((method.to_ascii_uppercase(), path.clone()));
                    break;
                }
                search_from = abs + needle.len();
            }
        }
    }
    routes
}

/// The classification manifest for every route `api::router` registers
/// (`src/api/mod.rs`). Hand-authored, checked for completeness (not
/// correctness — see module docs) by `every_rest_route_is_classified`.
fn rest_manifest() -> HashMap<(&'static str, &'static str), Classification> {
    use Classification::{Exempt, Filtered, Gated};
    use Gate::*;

    const NOT_PROJECT_SCOPED: &str =
        "global account/instance surface, not project-scoped — gated (or intentionally open) independent of authz.rs";
    const OWNERSHIP: &str = "author-or-admin ownership check (ownership, not project role)";
    const PUBLIC: &str = "unauthenticated by design (auth screen / health check)";

    HashMap::from([
        // ── Public / instance ──
        (("GET", "/api/instance"), Exempt(PUBLIC)),
        (("GET", "/api/instance/settings"), Exempt(NOT_PROJECT_SCOPED)),
        (("PATCH", "/api/instance/settings"), Exempt(NOT_PROJECT_SCOPED)),
        (("GET", "/api/health"), Exempt(PUBLIC)),
        // ── Auth: self-service, own account, not project-scoped ──
        (("POST", "/api/auth/signup"), Exempt(PUBLIC)),
        (("POST", "/api/auth/login"), Exempt(PUBLIC)),
        (("POST", "/api/auth/auto-login"), Exempt("public but instance-flag gated — LIF-215")),
        (("POST", "/api/auth/logout"), Exempt(NOT_PROJECT_SCOPED)),
        (("GET", "/api/auth/me"), Exempt(NOT_PROJECT_SCOPED)),
        (("PATCH", "/api/auth/me"), Exempt(NOT_PROJECT_SCOPED)),
        (("POST", "/api/auth/me/password"), Exempt(NOT_PROJECT_SCOPED)),
        (("DELETE", "/api/auth/me/sessions"), Exempt(NOT_PROJECT_SCOPED)),
        (("GET", "/api/auth/keys"), Exempt(NOT_PROJECT_SCOPED)),
        (("POST", "/api/auth/keys"), Exempt(NOT_PROJECT_SCOPED)),
        (("DELETE", "/api/auth/keys/{id}"), Exempt(NOT_PROJECT_SCOPED)),
        (("GET", "/api/auth/bots"), Exempt(NOT_PROJECT_SCOPED)),
        (("POST", "/api/auth/bots"), Exempt(NOT_PROJECT_SCOPED)),
        (("POST", "/api/auth/bots/{id}/disconnect"), Exempt(NOT_PROJECT_SCOPED)),
        (("DELETE", "/api/auth/bots/{id}"), Exempt(NOT_PROJECT_SCOPED)),
        (("GET", "/api/users"), Exempt("global user directory for UI dropdowns; auth-required only, via the outer auth_middleware_wrapper")),
        // ── Comments: Viewer to read/create (project resolved from parent) ──
        (("GET", "/api/issues/{issue_id}/comments"), Gated(Viewer)),
        (("POST", "/api/issues/{issue_id}/comments"), Gated(Viewer)),
        (("GET", "/api/pages/{page_id}/comments"), Gated(Viewer)),
        (("POST", "/api/pages/{page_id}/comments"), Gated(Viewer)),
        (("PUT", "/api/comments/{id}"), Exempt(OWNERSHIP)),
        (("DELETE", "/api/comments/{id}"), Exempt(OWNERSHIP)),
        // ── Projects ──
        (("GET", "/api/projects"), Filtered),
        (("POST", "/api/projects"), Exempt("open to any authenticated user; creator auto-becomes lead — LIF-DOC-7 decision #13")),
        (("PUT", "/api/projects/reorder"), Exempt("require_authenticated only; instance-wide sidebar chrome, not a project edit — LIF-233")),
        (("GET", "/api/projects/{id}"), Gated(Viewer)),
        (("PUT", "/api/projects/{id}"), Gated(Lead)),
        (("DELETE", "/api/projects/{id}"), Gated(ProjectDelete)),
        (("GET", "/api/projects/{id}/board"), Gated(Viewer)),
        (("GET", "/api/projects/{id}/issue-counts"), Gated(Viewer)),
        (("POST", "/api/projects/{id}/import/github"), Gated(Lead)),
        // ── Membership management (LIF-199, REST/web-only) ──
        (("GET", "/api/projects/{id}/members"), Gated(Viewer)),
        (("POST", "/api/projects/{id}/members"), Gated(Lead)),
        (("PATCH", "/api/projects/{id}/members/{user_id}"), Gated(Lead)),
        (("DELETE", "/api/projects/{id}/members/{user_id}"), Gated(Lead)),
        // ── Saved views (LIF-242, REST/web-only) ──
        // Gated(Viewer) is the *project-role* floor only — every one of
        // these additionally enforces strict per-user ownership in the
        // query layer (db::queries::views::get_owned_view), which this
        // manifest's Gate enum has no vocabulary for since it isn't a
        // project-role check at all: a project's Viewer, Maintainer, and
        // Lead can each only ever see/modify their *own* saved views, never
        // a teammate's. See src/api/views.rs's module doc comment.
        (("GET", "/api/projects/{id}/views"), Gated(Viewer)),
        (("POST", "/api/projects/{id}/views"), Gated(Viewer)),
        (("PATCH", "/api/projects/{id}/views/{view_id}"), Gated(Viewer)),
        (("DELETE", "/api/projects/{id}/views/{view_id}"), Gated(Viewer)),
        // ── Issues ──
        (("GET", "/api/issues"), Filtered),
        (("POST", "/api/issues"), Gated(Maintainer)),
        (("GET", "/api/issues/{id}"), Gated(Viewer)),
        (("PUT", "/api/issues/{id}"), Gated(Maintainer)),
        (("DELETE", "/api/issues/{id}"), Gated(Maintainer)),
        (("GET", "/api/issues/resolve/{identifier}"), Gated(Viewer)),
        (("POST", "/api/issues/link"), Gated(Maintainer)),
        (("POST", "/api/issues/unlink"), Gated(Maintainer)),
        // ── Activity / export (all read-side, Viewer) ──
        (("GET", "/api/issues/{id}/activity"), Gated(Viewer)),
        (("GET", "/api/pages/{id}/activity"), Gated(Viewer)),
        (("GET", "/api/projects/{id}/activity"), Gated(Viewer)),
        (("GET", "/api/projects/{id}/activity/actors"), Gated(Viewer)),
        (("GET", "/api/projects/{id}/insights"), Gated(Viewer)),
        (("GET", "/api/export/issues/{identifier}"), Gated(Viewer)),
        (("GET", "/api/export/pages/{identifier}"), Gated(Viewer)),
        (("GET", "/api/export/projects/{identifier}"), Gated(Viewer)),
        // ── Structure: modules / labels / folders ──
        (("GET", "/api/modules"), Gated(Viewer)),
        (("POST", "/api/modules"), Gated(StructureRole)),
        (("GET", "/api/modules/{id}"), Gated(Viewer)),
        (("PUT", "/api/modules/{id}"), Gated(StructureRole)),
        (("DELETE", "/api/modules/{id}"), Gated(StructureRole)),
        (("GET", "/api/labels"), Gated(Viewer)),
        (("POST", "/api/labels"), Gated(StructureRole)),
        (("PUT", "/api/labels/{id}"), Gated(StructureRole)),
        (("DELETE", "/api/labels/{id}"), Gated(StructureRole)),
        (("POST", "/api/labels/{id}/merge"), Gated(StructureRole)),
        (("GET", "/api/folders"), Gated(Viewer)),
        (("POST", "/api/folders"), Gated(StructureRole)),
        (("DELETE", "/api/folders/{id}"), Gated(StructureRole)),
        // ── Pages ──
        (("GET", "/api/pages"), Filtered),
        (("POST", "/api/pages"), Gated(Maintainer)),
        (("GET", "/api/pages/{id}"), Gated(Viewer)),
        (("PUT", "/api/pages/{id}"), Gated(Maintainer)),
        (("DELETE", "/api/pages/{id}"), Gated(Maintainer)),
        // ── Plans ──
        (("GET", "/api/plans"), Filtered),
        (("POST", "/api/plans"), Gated(Maintainer)),
        (("GET", "/api/plans/{id}"), Gated(Viewer)),
        (("PUT", "/api/plans/{id}"), Gated(Maintainer)),
        (("DELETE", "/api/plans/{id}"), Gated(Maintainer)),
        (("GET", "/api/plans/resolve/{identifier}"), Gated(Viewer)),
        (("GET", "/api/plans/{id}/activity"), Gated(Viewer)),
        (("POST", "/api/plans/{id}/steps"), Gated(Maintainer)),
        (("PUT", "/api/plans/{plan_id}/steps/{step_id}"), Gated(Maintainer)),
        (("DELETE", "/api/plans/{plan_id}/steps/{step_id}"), Gated(Maintainer)),
        // ── Search ──
        (("GET", "/api/search"), Filtered),
    ])
}

#[test]
fn every_rest_route_is_classified() {
    let src = include_str!("api/mod.rs");
    let routes = extract_rest_routes(src);
    assert!(
        routes.len() > 40,
        "sanity check: route extraction found suspiciously few routes ({}) — the parser in \
         extract_rest_routes() is probably out of sync with api/mod.rs's formatting",
        routes.len()
    );

    let manifest = rest_manifest();
    let mut unclassified: Vec<String> = routes
        .iter()
        .filter(|(m, p)| !manifest.contains_key(&(m.as_str(), p.as_str())))
        .map(|(m, p)| format!("{m} {p}"))
        .collect();
    unclassified.sort();
    unclassified.dedup();

    assert!(
        unclassified.is_empty(),
        "New REST route(s) registered in api/mod.rs are not classified in \
         `rest_manifest()` (src/authz_coverage_tests.rs). Every route must be \
         Gated(<level>), Filtered, or Exempt(<reason>) before it ships — \
         see LIF-DOC-7 decision #15. Unclassified:\n  {}",
        unclassified.join("\n  ")
    );
}

#[test]
fn rest_manifest_has_no_stale_entries() {
    // The inverse of the completeness check: a manifest entry for a route
    // that was renamed/removed would otherwise sit there un-exercised
    // forever, silently hiding the fact that the *real* new route name is
    // unclassified (extract_rest_routes finds it, but a human skimming the
    // manifest sees an entry and assumes it's covered).
    let src = include_str!("api/mod.rs");
    let routes: std::collections::HashSet<(String, String)> =
        extract_rest_routes(src).into_iter().collect();

    let mut stale: Vec<String> = rest_manifest()
        .keys()
        .filter(|(m, p)| !routes.contains(&(m.to_string(), p.to_string())))
        .map(|(m, p)| format!("{m} {p}"))
        .collect();
    stale.sort();

    assert!(
        stale.is_empty(),
        "rest_manifest() has entries that no longer match a real route in api/mod.rs \
         (renamed or removed) — remove them so the manifest can't mask an unclassified \
         replacement route:\n  {}",
        stale.join("\n  ")
    );
}

#[test]
fn rest_manifest_exempt_reasons_are_non_empty() {
    for ((method, path), classification) in rest_manifest() {
        if let Classification::Exempt(reason) = classification {
            assert!(
                !reason.trim().is_empty(),
                "{method} {path} is Exempt() with no reason — every Exempt needs a reason \
                 a reviewer can check, not a bare exemption"
            );
        }
    }
}

// ── MCP: runtime tool enumeration ────────────────────────────────────

/// The classification manifest for every MCP tool `LificMcp` registers
/// (`src/mcp/tools.rs`, `#[tool_router]` block). Checked against the tool
/// names the live `ToolRouter` reports at runtime (`list_tool_names`,
/// `src/mcp/mod.rs`) — not a hand-copied list of `#[tool]` fns, so a
/// rename that only touches one side still gets caught.
fn mcp_manifest() -> HashMap<&'static str, Classification> {
    use Classification::{Filtered, Gated, Mixed};
    use Gate::*;

    HashMap::from([
        ("search", Filtered),
        ("get_activity", Gated(Viewer)),
        ("list_issues", Gated(Viewer)),
        ("get_issue", Gated(Viewer)),
        ("export_issue", Gated(Viewer)),
        ("create_issue", Gated(Maintainer)),
        ("update_issue", Gated(Maintainer)),
        ("edit_issue", Gated(Maintainer)),
        ("get_board", Gated(Viewer)),
        ("link_issues", Gated(Maintainer)),
        ("unlink_issues", Gated(Maintainer)),
        ("get_page", Gated(Viewer)),
        ("export_page", Gated(Viewer)),
        ("export_project", Gated(Viewer)),
        ("create_page", Gated(Maintainer)),
        ("update_page", Gated(Maintainer)),
        ("edit_page", Gated(Maintainer)),
        (
            "delete",
            Mixed(
                "dispatches on resource_type: issue/plan/page = Maintainer (page falls back to \
                 workspace-admin when project-less); project = ProjectDelete; \
                 module/label/folder = StructureRole",
            ),
        ),
        (
            "list_resources",
            Mixed(
                "dispatches on resource_type: project = Filtered (visible_project_ids); \
                 module/label/folder/issue/plan = Viewer; page = Viewer when project given, \
                 else Filtered",
            ),
        ),
        (
            "manage_resource",
            Mixed(
                "dispatches on (resource_type, action): project/create = open to any \
                 authenticated user (mirrors REST decision #13); project/update = Lead; \
                 module|label|folder (create/update) = StructureRole",
            ),
        ),
        ("add_comment", Gated(Viewer)),
        ("list_comments", Gated(Viewer)),
        ("create_plan", Gated(Maintainer)),
        ("get_plan", Gated(Viewer)),
        ("edit_plan_step", Gated(Maintainer)),
        ("update_plan_step", Gated(Maintainer)),
    ])
}

#[test]
fn every_mcp_tool_is_classified() {
    let db = crate::db::open_memory().expect("test db");
    let mcp = crate::mcp::LificMcp::new(db);
    let tools = mcp.list_tool_names();
    assert!(
        tools.len() > 15,
        "sanity check: only found {} MCP tools at runtime — list_tool_names()/ToolRouter \
         wiring is probably broken",
        tools.len()
    );

    let manifest = mcp_manifest();
    let mut unclassified: Vec<&str> = tools
        .iter()
        .filter(|name| !manifest.contains_key(name.as_str()))
        .map(|s| s.as_str())
        .collect();
    unclassified.sort();
    unclassified.dedup();

    assert!(
        unclassified.is_empty(),
        "New MCP tool(s) registered in src/mcp/tools.rs are not classified in \
         `mcp_manifest()` (src/authz_coverage_tests.rs). Every tool must be \
         Gated(<level>), Filtered, or Mixed(<per-branch summary>) before it ships — \
         see LIF-DOC-7 decision #15. Unclassified:\n  {}",
        unclassified.join("\n  ")
    );
}

#[test]
fn mcp_manifest_has_no_stale_entries() {
    let db = crate::db::open_memory().expect("test db");
    let mcp = crate::mcp::LificMcp::new(db);
    let tools: std::collections::HashSet<String> = mcp.list_tool_names().into_iter().collect();

    let mut stale: Vec<&str> = mcp_manifest()
        .keys()
        .filter(|name| !tools.contains(**name))
        .copied()
        .collect();
    stale.sort();

    assert!(
        stale.is_empty(),
        "mcp_manifest() has entries that no longer match a real MCP tool (renamed or \
         removed) — remove them so the manifest can't mask an unclassified replacement \
         tool:\n  {}",
        stale.join("\n  ")
    );
}

#[test]
fn mcp_manifest_mixed_reasons_are_non_empty() {
    for (name, classification) in mcp_manifest() {
        if let Classification::Mixed(reason) = classification {
            assert!(
                !reason.trim().is_empty(),
                "MCP tool '{name}' is Mixed() with no per-branch summary"
            );
        }
    }
}



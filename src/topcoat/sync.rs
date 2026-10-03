//! Browser replica and shared realtime transport for Topcoat screens.
//!
//! Load [`SCRIPT_PATH`] after the session bridge. `lificSync.ensureProject(id)`
//! resolves to an immutable issue/page snapshot, catching up after bootstrap.
//! `peekProject(id)` reads the current snapshot without making requests.
//! Screens call `setActiveProject(id)` on navigation, subscribe to changes,
//! and call `refreshProject(id)` after mutations. Scope changes invalidate all
//! replicas and in-flight requests; public screens use REST only.
//!
//! The browser rejects sequences outside JavaScript's exact integer range.
//! It never resumes from a rounded cursor. Supporting the entire wire i64
//! range requires lossless JSON parsing at the session request boundary.

use topcoat::router::{response::Response, route};

pub(crate) const SCRIPT_PATH: &str = "/__topcoat-sync.js";
pub(crate) const BROWSER_SCRIPT: &str = include_str!("assets/sync.js");

#[route(GET "/__topcoat-sync.js")]
async fn browser_script() -> topcoat::Result<Response> {
    Ok(Response::builder()
        .header("content-type", "text/javascript; charset=utf-8")
        .header("cache-control", "no-cache")
        .body(topcoat::router::Body::from(BROWSER_SCRIPT))?)
}

// LIF-430: `WebAssets` in src/server.rs embeds web/dist/ via rust-embed, but
// without a build script cargo has no idea the crate depends on those files.
// Rebuild the frontend, run `cargo build --release`, and cargo could declare
// the crate fresh and ship the *previous* bundle embedded in the binary —
// silently. That happened mid-incident during LIF-428 and sent the diagnosis
// down a false path (web/dist held index-DYCyqY48.js while the binary kept
// serving index-C49UV6aq.js).
//
// `rerun-if-changed` on a directory watches it recursively, so any change to
// the built bundle invalidates the crate and forces a re-embed.

use std::path::Path;
use std::time::SystemTime;

fn main() {
    // CI and clean checkouts may not have a built frontend yet. Create the
    // embed directory up-front so the rust-embed derive can compile cleanly even
    // when the web bundle has not been generated.
    let dist = Path::new("web/dist");
    if !dist.exists() {
        std::fs::create_dir_all(dist).expect("create missing web/dist directory");
    }

    // The built bundle: changing it must trigger a re-embed.
    println!("cargo:rerun-if-changed=web/dist");
    // The frontend sources: changing them cannot rebuild the bundle for us,
    // but it re-runs this script so the staleness check below gets a chance
    // to point out that web/dist no longer matches web/src.
    println!("cargo:rerun-if-changed=web/src");

    let src = Path::new("web/src");

    match (newest_mtime(dist), newest_mtime(src)) {
        (None, _) => {
            println!(
                "cargo:warning=web/dist is missing or empty; the binary will ship without the web UI (run `bun run build` in web/)"
            );
        }
        (Some(dist_mtime), Some(src_mtime)) if src_mtime > dist_mtime => {
            println!(
                "cargo:warning=web/src is newer than web/dist; the embedded frontend is stale (run `bun run build` in web/)"
            );
        }
        _ => {}
    }
}

/// Newest file modification time anywhere under `dir`, or `None` if the
/// directory is missing or contains no files.
fn newest_mtime(dir: &Path) -> Option<SystemTime> {
    let mut newest: Option<SystemTime> = None;
    let entries = std::fs::read_dir(dir).ok()?;
    for entry in entries.flatten() {
        let path = entry.path();
        let candidate = if path.is_dir() {
            newest_mtime(&path)
        } else {
            entry.metadata().ok().and_then(|m| m.modified().ok())
        };
        if let Some(t) = candidate
            && newest.is_none_or(|n| t > n)
        {
            newest = Some(t);
        }
    }
    newest
}

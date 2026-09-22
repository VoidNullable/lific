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
    // The frontend must be built before this crate; never create web/dist here.
    // Builds must not mutate the source tree.
    let dist = Path::new("web/dist");

    // The built bundle: changing it must trigger a re-embed.
    println!("cargo:rerun-if-changed=web/dist");
    // The frontend sources: changing them cannot rebuild the bundle for us,
    // but it re-runs this script so the staleness check below gets a chance
    // to point out that web/dist no longer matches web/src.
    println!("cargo:rerun-if-changed=web/src");

    let src = Path::new("web/src");

    if matches!(std::env::var("PROFILE").as_deref(), Ok("release" | "dist"))
        && !has_frontend_entry(dist)
    {
        panic!(
            "web/dist/index.html is missing or empty; build the frontend first with `devenv tasks run lific:web:build`"
        );
    }

    match (newest_mtime(dist), newest_mtime(src)) {
        (None, _) => {
            println!(
                "cargo:warning=web/dist is missing or empty; development builds use the frontend dev server (run `devenv tasks run lific:web:build` for an embedded UI)"
            );
        }
        (Some(dist_mtime), Some(src_mtime)) if src_mtime > dist_mtime => {
            println!(
                "cargo:warning=web/src is newer than web/dist; the embedded frontend is stale (run `devenv tasks run lific:web:build`)"
            );
        }
        _ => {}
    }
}

fn has_frontend_entry(dist: &Path) -> bool {
    dist.join("index.html")
        .metadata()
        .is_ok_and(|entry| entry.is_file() && entry.len() > 0)
}

/// Newest file modification time anywhere under `dir`, or `None` if the
/// directory is missing or contains no files.
fn newest_mtime(dir: &Path) -> Option<SystemTime> {
    let mut newest: Option<SystemTime> = None;
    let entries = std::fs::read_dir(dir).ok()?;
    for entry in entries.flatten() {
        if entry.file_name() == ".gitkeep" {
            continue;
        }
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

#[cfg(test)]
mod tests {
    use super::{has_frontend_entry, newest_mtime};

    #[test]
    fn assets_without_an_html_entry_are_not_a_shippable_frontend() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("bundle.js"), "console.log('hello')").unwrap();
        assert!(!has_frontend_entry(dir.path()));
        std::fs::write(dir.path().join("index.html"), "").unwrap();
        assert!(!has_frontend_entry(dir.path()));
        std::fs::write(dir.path().join("index.html"), "<!doctype html>").unwrap();
        assert!(has_frontend_entry(dir.path()));
    }

    #[test]
    fn checkout_placeholder_is_not_a_built_frontend() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join(".gitkeep"), "").unwrap();
        assert_eq!(newest_mtime(dir.path()), None);

        let bundle = dir.path().join("index.html");
        std::fs::write(&bundle, "fixture frontend").unwrap();
        assert_eq!(
            newest_mtime(dir.path()),
            Some(std::fs::metadata(bundle).unwrap().modified().unwrap())
        );
    }
}

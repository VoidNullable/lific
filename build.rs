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

/// The Windows application manifest. `consoleAllocationPolicy=detached` keeps
/// Windows 11 24H2+ from opening a console window when Explorer starts lific
/// at logon (the background service's Run entry), while a terminal still gets
/// an ordinary attached, waited-for CLI. Older Windows ignores the element.
/// Deliberately minimal: no supportedOS, code page or long-path settings, so
/// nothing else about the binary's runtime behavior changes.
const WINDOWS_MANIFEST: &str = r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<assembly xmlns="urn:schemas-microsoft-com:asm.v1" manifestVersion="1.0">
  <trustInfo xmlns="urn:schemas-microsoft-com:asm.v3">
    <security>
      <requestedPrivileges>
        <requestedExecutionLevel level="asInvoker" uiAccess="false"/>
      </requestedPrivileges>
    </security>
  </trustInfo>
  <application xmlns="urn:schemas-microsoft-com:asm.v3">
    <windowsSettings>
      <consoleAllocationPolicy xmlns="http://schemas.microsoft.com/SMI/2024/WindowsSettings">detached</consoleAllocationPolicy>
    </windowsSettings>
  </application>
</assembly>
"#;

fn embed_windows_manifest() {
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() != Ok("windows") {
        return;
    }
    let out_dir = std::env::var_os("OUT_DIR").expect("cargo sets OUT_DIR for build scripts");
    let path = Path::new(&out_dir).join("lific.exe.manifest");
    std::fs::write(&path, WINDOWS_MANIFEST).expect("write the Windows manifest");
    embed_manifest::embed_manifest_file(&path).expect("embed the Windows manifest");
}

/// Gives the Windows main thread the same 8 MB stack Linux and macOS give it,
/// instead of the 1 MB MSVC default. Startup runs clap's derived builders on
/// that thread, and unoptimized builds keep every builder step in its own
/// stack slot: `Command::augment_subcommands` alone reserves over 400 KB, and
/// it grows with each CLI argument. Debug binaries were one subcommand away
/// from overflowing before `--version` could print.
fn reserve_windows_main_stack() {
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("windows")
        && std::env::var("CARGO_CFG_TARGET_ENV").as_deref() == Ok("msvc")
    {
        println!("cargo:rustc-link-arg-bins=/STACK:8388608");
    }
}

fn main() {
    embed_windows_manifest();
    reserve_windows_main_stack();

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

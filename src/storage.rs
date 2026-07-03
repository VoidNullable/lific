//! LIF-262: content-addressed file storage for attachments.
//!
//! Raw bytes never touch SQLite. Each uploaded blob is written to
//! `<data_dir>/attachments/<sha256>`, where `<data_dir>` is the directory
//! containing the database file (see [`AttachmentStore::from_db_path`]). The
//! file name IS the content hash, so identical bytes uploaded twice collapse
//! onto one file — deduplication is a property of the layout, not extra code.
//!
//! The DB's `attachments` table holds the metadata (filename, mime, uploader,
//! size) and points at a blob via its `sha256`. Because the store is
//! content-addressed, writes are idempotent: re-writing an already-present
//! hash is a cheap no-op, and a delete only removes the sidecar file once the
//! caller has confirmed no `attachments` row still references that hash (the
//! orphan GC's job — see `db::queries::attachments`).

use std::path::{Path, PathBuf};

use sha2::{Digest, Sha256};

use crate::error::LificError;

/// Handle to the on-disk attachments directory. Cheap to clone (just a
/// `PathBuf`); threaded through the API layer as an axum `Extension` the same
/// way `AuthConfig` is.
#[derive(Debug, Clone)]
pub struct AttachmentStore {
    dir: PathBuf,
}

impl AttachmentStore {
    /// Build a store rooted at `<parent-of-db>/attachments`. Mirrors
    /// `Config::backup_dir`'s "resolve relative to the database file" rule so
    /// the whole data set (db + backups + attachments) sits together and the
    /// backup task can include it.
    pub fn from_db_path(db_path: &Path) -> Self {
        let dir = match db_path.parent() {
            Some(parent) if !parent.as_os_str().is_empty() => parent.join("attachments"),
            _ => PathBuf::from("attachments"),
        };
        Self { dir }
    }

    /// Construct a store at an explicit directory (tests use a tempdir).
    #[allow(dead_code)]
    pub fn new(dir: PathBuf) -> Self {
        Self { dir }
    }

    /// The attachments directory itself (for the backup set / diagnostics).
    #[allow(dead_code)]
    pub fn dir(&self) -> &Path {
        &self.dir
    }

    /// Absolute path to the sidecar file for a given content hash. Kept
    /// private-ish (only `pub(crate)`) so callers go through
    /// `read`/`write`/`delete` rather than hand-building paths.
    pub(crate) fn path_for(&self, sha256: &str) -> PathBuf {
        self.dir.join(sha256)
    }

    /// Compute the lowercase hex SHA-256 of a byte slice — the content address.
    pub fn hash_bytes(bytes: &[u8]) -> String {
        let digest = Sha256::digest(bytes);
        digest.iter().map(|b| format!("{b:02x}")).collect()
    }

    /// Write `bytes` to `<dir>/<sha256>`, creating the directory if needed.
    /// Idempotent: if the file already exists (same content), this is a no-op
    /// rather than a rewrite. Returns the content hash so the caller can store
    /// it on the metadata row.
    pub fn write(&self, bytes: &[u8]) -> Result<String, LificError> {
        let sha = Self::hash_bytes(bytes);
        std::fs::create_dir_all(&self.dir)
            .map_err(|e| LificError::Internal(format!("create attachments dir: {e}")))?;
        let path = self.path_for(&sha);
        if path.exists() {
            return Ok(sha);
        }
        // Write to a temp file then rename, so a concurrent reader never sees a
        // half-written blob at the final content-addressed path.
        let tmp = path.with_extension("tmp");
        std::fs::write(&tmp, bytes)
            .map_err(|e| LificError::Internal(format!("write attachment: {e}")))?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let _ = std::fs::set_permissions(&tmp, std::fs::Permissions::from_mode(0o600));
        }
        std::fs::rename(&tmp, &path)
            .map_err(|e| LificError::Internal(format!("finalize attachment: {e}")))?;
        Ok(sha)
    }

    /// Read the bytes for a content hash. `NotFound` when the sidecar file is
    /// missing (e.g. the DB row survived but the blob was manually removed).
    pub fn read(&self, sha256: &str) -> Result<Vec<u8>, LificError> {
        let path = self.path_for(sha256);
        std::fs::read(&path).map_err(|e| {
            if e.kind() == std::io::ErrorKind::NotFound {
                LificError::NotFound("attachment bytes not found on disk".into())
            } else {
                LificError::Internal(format!("read attachment: {e}"))
            }
        })
    }

    /// Delete the sidecar file for a content hash. Missing file is treated as
    /// success (idempotent) — the GC only calls this once no DB row references
    /// the hash, so a double-delete or a manual prior removal is fine.
    pub fn delete(&self, sha256: &str) -> Result<(), LificError> {
        let path = self.path_for(sha256);
        match std::fs::remove_file(&path) {
            Ok(()) => Ok(()),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(e) => Err(LificError::Internal(format!("delete attachment: {e}"))),
        }
    }
}

// ── Orphan GC sweep ──────────────────────────────────────────

/// Grace window before an unlinked attachment is collectable: an upload that's
/// been sitting linkless for longer than this is assumed abandoned (the
/// compose that created it never saved). 24h is generous — a draft can live a
/// long time before it's posted.
pub const ORPHAN_GRACE_SECONDS: i64 = 24 * 60 * 60;

/// Delete abandoned attachments: rows with zero links older than the grace
/// window, plus their sidecar blobs when no surviving row shares the content
/// hash. Returns the number of rows collected. Safe to call repeatedly (a
/// background task drives it on an interval — see `start_gc_task`).
pub fn sweep_orphans(
    pool: &crate::db::DbPool,
    store: &AttachmentStore,
    grace_seconds: i64,
) -> Result<usize, LificError> {
    use crate::db::queries::attachments as q;

    let orphans = {
        let conn = pool.read()?;
        q::find_orphans(&conn, grace_seconds)?
    };

    let mut collected = 0;
    for orphan in orphans {
        {
            let conn = pool.write()?;
            q::delete_attachment(&conn, orphan.id)?;
        }
        // Remove the blob only if no other row still references those bytes.
        let remaining = {
            let conn = pool.read()?;
            q::count_rows_for_sha(&conn, &orphan.sha256)?
        };
        if remaining == 0 {
            store.delete(&orphan.sha256)?;
        }
        collected += 1;
    }
    Ok(collected)
}

/// Spawn a background task that sweeps orphaned attachments hourly. Mirrors the
/// backup task's shape (initial delay then a fixed interval).
pub fn start_gc_task(
    pool: crate::db::DbPool,
    store: AttachmentStore,
) -> tokio::task::JoinHandle<()> {
    tokio::spawn(async move {
        // Let the server settle before the first sweep.
        tokio::time::sleep(std::time::Duration::from_secs(60)).await;
        let mut interval = tokio::time::interval(std::time::Duration::from_secs(60 * 60));
        interval.tick().await; // skip the immediate tick
        loop {
            interval.tick().await;
            match sweep_orphans(&pool, &store, ORPHAN_GRACE_SECONDS) {
                Ok(n) if n > 0 => tracing::info!(collected = n, "attachment GC swept orphans"),
                Ok(_) => {}
                Err(e) => tracing::warn!(error = %e, "attachment GC sweep failed"),
            }
        }
    })
}

// ── MIME sniffing + allowlist ────────────────────────────────
//
// Never trust the client-supplied content-type: a browser (or a malicious
// uploader) can claim `image/png` for an HTML file and get it served back
// inline. We sniff the leading magic bytes and cross-check against the
// allowlist, falling back to the declared type only for the formats that have
// no reliable signature (plain text / log).

/// The upload MIME allowlist: images + a few safe document/archive types.
/// Executables and everything else are rejected. Returned type is the
/// canonical MIME we store and serve (so a `image/jpg` claim normalizes to
/// `image/jpeg`, etc.).
pub const ALLOWED_MIMES: &[&str] = &[
    "image/png",
    "image/jpeg",
    "image/gif",
    "image/webp",
    "image/svg+xml",
    "application/pdf",
    "text/plain",
    "application/zip",
];

/// Whether a canonical MIME is an image (drives inline rendering + the
/// `Content-Disposition` decision on download).
pub fn is_image_mime(mime: &str) -> bool {
    mime.starts_with("image/")
}

/// Sniff the real content type from magic bytes, cross-checked against the
/// declared type and the allowlist. Returns the canonical MIME to store, or an
/// error describing why the upload was rejected.
///
/// The declared type is only honored for signature-less formats (plain text),
/// and even then only when it's on the allowlist. Everything with a real
/// signature (images, pdf, zip) is decided purely by the bytes — a lie in the
/// header can't smuggle an executable past this.
pub fn sniff_and_validate(bytes: &[u8], declared: Option<&str>) -> Result<String, LificError> {
    // Signature-based detection first (authoritative).
    if let Some(mime) = sniff_magic(bytes) {
        return Ok(mime.to_string());
    }

    // No recognizable binary signature. Reject anything that structurally
    // looks like an executable or script, regardless of the declared type.
    if looks_executable(bytes) {
        return Err(LificError::BadRequest(
            "rejected: file looks like an executable".into(),
        ));
    }

    // SVG is XML-based (text signature): accept when it declares an svg/xml
    // type and the content opens like SVG/XML.
    let declared = declared.map(|d| d.split(';').next().unwrap_or(d).trim().to_ascii_lowercase());
    if let Some(d) = declared.as_deref() {
        if d == "image/svg+xml" && looks_like_svg(bytes) {
            return Ok("image/svg+xml".to_string());
        }
        // Plain text / logs have no signature — trust the declared type only
        // when it's the text type on the allowlist and the bytes are valid
        // UTF-8 (so we never serve arbitrary binary as text/plain).
        if (d == "text/plain" || d == "text/x-log") && std::str::from_utf8(bytes).is_ok() {
            return Ok("text/plain".to_string());
        }
    }

    // Last resort: valid UTF-8 with no executable markers is treated as plain
    // text (covers `.txt` / `.log` uploaded with no/incorrect content-type).
    if std::str::from_utf8(bytes).is_ok() && !bytes.is_empty() {
        return Ok("text/plain".to_string());
    }

    Err(LificError::BadRequest(
        "rejected: unsupported or unrecognized file type".into(),
    ))
}

/// Detect the canonical MIME from leading magic bytes for the binary formats
/// on the allowlist. `None` when no signature matches.
fn sniff_magic(bytes: &[u8]) -> Option<&'static str> {
    if bytes.len() >= 8 && bytes.starts_with(&[0x89, 0x50, 0x4E, 0x47, 0x0D, 0x0A, 0x1A, 0x0A]) {
        return Some("image/png");
    }
    if bytes.len() >= 3 && bytes.starts_with(&[0xFF, 0xD8, 0xFF]) {
        return Some("image/jpeg");
    }
    if bytes.len() >= 6 && (bytes.starts_with(b"GIF87a") || bytes.starts_with(b"GIF89a")) {
        return Some("image/gif");
    }
    if bytes.len() >= 12 && bytes.starts_with(b"RIFF") && &bytes[8..12] == b"WEBP" {
        return Some("image/webp");
    }
    if bytes.len() >= 5 && bytes.starts_with(b"%PDF-") {
        return Some("application/pdf");
    }
    // ZIP (also the container for docx/xlsx/etc, but we only advertise zip).
    if bytes.len() >= 4
        && bytes[0] == 0x50
        && bytes[1] == 0x4B
        && (bytes[2] == 0x03 || bytes[2] == 0x05 || bytes[2] == 0x07)
    {
        return Some("application/zip");
    }
    None
}

/// Heuristic executable/script sniff for the signature-less path. Blocks the
/// obvious dangerous headers so a "text/plain" claim can't smuggle a binary.
fn looks_executable(bytes: &[u8]) -> bool {
    // ELF, Mach-O (32/64, both endian), PE/DOS (MZ), Java class, shebang, WASM.
    const SIGS: &[&[u8]] = &[
        b"\x7FELF",
        &[0xFE, 0xED, 0xFA, 0xCE],
        &[0xFE, 0xED, 0xFA, 0xCF],
        &[0xCF, 0xFA, 0xED, 0xFE],
        &[0xCE, 0xFA, 0xED, 0xFE],
        b"MZ",
        &[0xCA, 0xFE, 0xBA, 0xBE],
        b"#!",
        &[0x00, 0x61, 0x73, 0x6D], // \0asm (WebAssembly)
    ];
    SIGS.iter().any(|sig| bytes.starts_with(sig))
}

/// Cheap check that the head of a buffer opens like SVG/XML.
fn looks_like_svg(bytes: &[u8]) -> bool {
    let head_len = bytes.len().min(512);
    let head = String::from_utf8_lossy(&bytes[..head_len]).to_ascii_lowercase();
    head.contains("<svg") || head.trim_start().starts_with("<?xml")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tmp_store() -> (AttachmentStore, PathBuf) {
        let dir = std::env::temp_dir().join(format!(
            "lific_store_test_{}_{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        (AttachmentStore::new(dir.clone()), dir)
    }

    #[test]
    fn write_read_roundtrip_and_dedup() {
        let (store, dir) = tmp_store();
        let bytes = b"hello attachment world";
        let sha1 = store.write(bytes).unwrap();
        let sha2 = store.write(bytes).unwrap();
        assert_eq!(sha1, sha2, "same content hashes to same file");
        assert_eq!(store.read(&sha1).unwrap(), bytes);
        // Only one file on disk for the duplicate write.
        let count = std::fs::read_dir(&dir).unwrap().count();
        assert_eq!(count, 1);
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn delete_is_idempotent() {
        let (store, dir) = tmp_store();
        let sha = store.write(b"x").unwrap();
        store.delete(&sha).unwrap();
        store.delete(&sha).unwrap(); // second delete: no error
        assert!(store.read(&sha).is_err());
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn from_db_path_puts_attachments_next_to_db() {
        let store = AttachmentStore::from_db_path(Path::new("/data/lific/lific.db"));
        assert_eq!(store.dir(), Path::new("/data/lific/attachments"));
    }

    #[test]
    fn hash_is_stable_lowercase_hex() {
        // Known SHA-256 of the empty string.
        assert_eq!(
            AttachmentStore::hash_bytes(b""),
            "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855"
        );
    }

    // ── MIME sniffing ────────────────────────────────────────

    #[test]
    fn sniff_png_by_signature_ignores_lying_header() {
        let png = [0x89, 0x50, 0x4E, 0x47, 0x0D, 0x0A, 0x1A, 0x0A, 0, 0];
        assert_eq!(
            sniff_and_validate(&png, Some("application/x-msdownload")).unwrap(),
            "image/png"
        );
    }

    #[test]
    fn sniff_jpeg_gif_webp_pdf_zip() {
        assert_eq!(
            sniff_and_validate(&[0xFF, 0xD8, 0xFF, 0], None).unwrap(),
            "image/jpeg"
        );
        assert_eq!(
            sniff_and_validate(b"GIF89a....", None).unwrap(),
            "image/gif"
        );
        let mut webp = Vec::from(*b"RIFF____WEBPVP8 ");
        webp.extend_from_slice(&[0; 4]);
        assert_eq!(sniff_and_validate(&webp, None).unwrap(), "image/webp");
        assert_eq!(
            sniff_and_validate(b"%PDF-1.7\n%...", None).unwrap(),
            "application/pdf"
        );
        assert_eq!(
            sniff_and_validate(&[0x50, 0x4B, 0x03, 0x04, 0], None).unwrap(),
            "application/zip"
        );
    }

    #[test]
    fn rejects_elf_and_pe_executables() {
        assert!(sniff_and_validate(b"\x7FELF....", Some("text/plain")).is_err());
        assert!(sniff_and_validate(b"MZ\x90\x00", Some("text/plain")).is_err());
        assert!(sniff_and_validate(b"#!/bin/sh\n", Some("text/plain")).is_err());
    }

    #[test]
    fn plain_text_accepted_via_declared_type() {
        assert_eq!(
            sniff_and_validate(b"just some log lines\n", Some("text/plain")).unwrap(),
            "text/plain"
        );
    }

    #[test]
    fn svg_accepted_when_declared_and_looks_like_svg() {
        let svg = br#"<svg xmlns="http://www.w3.org/2000/svg"></svg>"#;
        assert_eq!(
            sniff_and_validate(svg, Some("image/svg+xml")).unwrap(),
            "image/svg+xml"
        );
    }

    #[test]
    fn is_image_mime_classifies() {
        assert!(is_image_mime("image/png"));
        assert!(is_image_mime("image/svg+xml"));
        assert!(!is_image_mime("application/pdf"));
        assert!(!is_image_mime("text/plain"));
    }
}

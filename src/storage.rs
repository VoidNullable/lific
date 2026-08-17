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

    /// Construct a store at an explicit directory. Production always resolves
    /// the directory from the database path via [`Self::from_db_path`]; only
    /// tests point a store at a tempdir, hence the test-scoped allow.
    #[cfg_attr(not(test), allow(dead_code))]
    pub fn new(dir: PathBuf) -> Self {
        Self { dir }
    }

    /// The attachments directory itself. Callers reach the files through
    /// `read`/`write`/`delete`, so only tests need the raw path today.
    #[cfg_attr(not(test), allow(dead_code))]
    pub fn dir(&self) -> &Path {
        &self.dir
    }

    /// Absolute path to the sidecar file for a given content hash. Kept
    /// private-ish (only `pub(crate)`) so callers go through
    /// `read`/`write`/`delete` rather than hand-building paths.
    pub(crate) fn path_for(&self, sha256: &str) -> PathBuf {
        self.dir.join(sha256)
    }

    /// Absolute path to the cached thumbnail for a content hash. Thumbnails
    /// live in a `thumbs/` subdirectory so a plain `read_dir` of the store
    /// still enumerates exactly the original blobs, and so a hash can never
    /// collide with its own derivative.
    pub(crate) fn thumb_path_for(&self, sha256: &str) -> PathBuf {
        self.dir.join("thumbs").join(format!("{sha256}.webp"))
    }

    /// Cache a generated thumbnail. Same temp-file-then-rename dance as
    /// [`Self::write`], so a concurrent reader never sees a partial webp.
    pub fn write_thumb(&self, sha256: &str, bytes: &[u8]) -> Result<(), LificError> {
        let path = self.thumb_path_for(sha256);
        let parent = path
            .parent()
            .ok_or_else(|| LificError::Internal("thumbnail path has no parent".into()))?;
        std::fs::create_dir_all(parent)
            .map_err(|e| LificError::Internal(format!("create thumbnails dir: {e}")))?;
        let tmp = path.with_extension("tmp");
        std::fs::write(&tmp, bytes)
            .map_err(|e| LificError::Internal(format!("write thumbnail: {e}")))?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let _ = std::fs::set_permissions(&tmp, std::fs::Permissions::from_mode(0o600));
        }
        std::fs::rename(&tmp, &path)
            .map_err(|e| LificError::Internal(format!("finalize thumbnail: {e}")))?;
        Ok(())
    }

    /// Read a cached thumbnail. `Ok(None)` when none has been generated yet,
    /// which is the ordinary state for an attachment uploaded before
    /// thumbnails existed.
    pub fn read_thumb(&self, sha256: &str) -> Result<Option<Vec<u8>>, LificError> {
        match std::fs::read(self.thumb_path_for(sha256)) {
            Ok(bytes) => Ok(Some(bytes)),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
            Err(e) => Err(LificError::Internal(format!("read thumbnail: {e}"))),
        }
    }

    /// Drop a cached thumbnail. Missing file is success: thumbnails are a
    /// cache, so every delete here is best-effort and repeatable.
    pub fn delete_thumb(&self, sha256: &str) -> Result<(), LificError> {
        match std::fs::remove_file(self.thumb_path_for(sha256)) {
            Ok(()) => Ok(()),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(e) => Err(LificError::Internal(format!("delete thumbnail: {e}"))),
        }
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
        // The thumbnail is derived from these exact bytes, so it dies with
        // them. Best-effort: a failure to remove a cache file must not block
        // the blob delete the caller actually asked for.
        let _ = self.delete_thumb(sha256);
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
    // LIF-418: media. Every one of these is magic-byte validated below, and
    // none of them is a script-execution vector in a browser, so they serve
    // inline (see `is_inline_safe_mime`) behind the same CSP sandbox as
    // everything else.
    "video/mp4",
    "video/webm",
    "audio/webm",
    "audio/ogg",
    "audio/mpeg",
    // LIF-418: SQLite databases, so `GET /api/attachments/{id}/preview` can
    // list their tables. Always served as a download, never inline.
    "application/vnd.sqlite3",
];

/// The raster image formats we can decode dimensions for and thumbnail. SVG is
/// excluded: it is a vector document with no intrinsic pixel size, and we do
/// not run an XML parser over untrusted uploads to guess one.
pub const RASTER_MIMES: &[&str] = &["image/png", "image/jpeg", "image/gif", "image/webp"];

/// Whether a canonical MIME is a raster image we decode.
pub fn is_raster_mime(mime: &str) -> bool {
    RASTER_MIMES.contains(&mime)
}

/// Whether a canonical MIME is safe to hand a browser as a *top-level
/// document* (`Content-Disposition: inline`).
///
/// This is deliberately an allowlist of raster formats rather than
/// `is_image_mime`. SVG is an image by MIME, but it is also an XML document
/// that can carry `<script>`, and browsers execute that script when the SVG
/// is navigated to directly (or framed) from the app's own origin. Serving
/// one inline is stored XSS: the script runs on the Lific origin and can read
/// the session bearer token the SPA keeps in `localStorage`, escalating any
/// upload-capable account to whoever views the file.
///
/// Rendering is unaffected. `Content-Disposition` is ignored for subresource
/// loads, so `<img src="/api/attachments/1">` still displays an SVG, and
/// scripts never run in that context.
/// LIF-418 extends the list to audio and video. An `<video>` or `<audio>`
/// element needs a real navigable URL with byte-range support, and a media
/// container is not a document: browsers hand mp4/webm/ogg/mp3 to the media
/// pipeline, which has no scripting surface. The CSP sandbox and `nosniff`
/// still ride along on every response, so a container that turned out to be
/// something else could still not execute.
pub fn is_inline_safe_mime(mime: &str) -> bool {
    matches!(
        mime,
        "image/png"
            | "image/jpeg"
            | "image/gif"
            | "image/webp"
            | "video/mp4"
            | "video/webm"
            | "audio/webm"
            | "audio/ogg"
            | "audio/mpeg"
    )
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
    let declared = declared.map(|d| d.split(';').next().unwrap_or(d).trim().to_ascii_lowercase());

    // Signature-based detection first (authoritative).
    if let Some(mime) = sniff_magic(bytes) {
        // One container, two canonical types: a WebM/Matroska file with no
        // video track is `audio/webm`, and telling them apart needs a full
        // track parse. The bytes still decide that this IS a WebM container;
        // the declared type only picks which of the two allowlisted labels we
        // record, and both are inline-safe media, so a lie here buys nothing.
        if mime == "video/webm" && declared.as_deref() == Some("audio/webm") {
            return Ok("audio/webm".to_string());
        }
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
    // SQLite database: a fixed 16-byte header string including its NUL.
    if bytes.starts_with(SQLITE_MAGIC) {
        return Some("application/vnd.sqlite3");
    }
    // ISO base media (mp4): a `ftyp` box at offset 4. The box's major brand
    // says which flavour; QuickTime rides the same container and is not on
    // the allowlist, so it is filtered out rather than relabelled as mp4.
    if bytes.len() >= 12 && &bytes[4..8] == b"ftyp" {
        let brand = &bytes[8..12];
        if brand != b"qt  " {
            return Some("video/mp4");
        }
        return None;
    }
    // EBML (Matroska / WebM). We only advertise WebM, so require the DocType
    // string to appear in the header region rather than accepting any EBML.
    if bytes.starts_with(&[0x1A, 0x45, 0xDF, 0xA3]) {
        let head = &bytes[..bytes.len().min(64)];
        if head.windows(4).any(|w| w == b"webm") {
            return Some("video/webm");
        }
        return None;
    }
    // Ogg container. Vorbis/Opus audio is all we advertise, so it is labelled
    // audio/ogg rather than the generic application/ogg.
    if bytes.starts_with(b"OggS") {
        return Some("audio/ogg");
    }
    // MP3: either an ID3v2 tag or a bare MPEG audio frame header. The frame
    // sync is eleven set bits, and the two fields right after it have
    // reserved encodings that a real frame never uses, so checking them keeps
    // arbitrary 0xFF-leading binary from being waved through as audio.
    if bytes.starts_with(b"ID3") {
        return Some("audio/mpeg");
    }
    if bytes.len() >= 2 && bytes[0] == 0xFF && (bytes[1] & 0xE0) == 0xE0 {
        let version = (bytes[1] >> 3) & 0b11; // 01 is reserved
        let layer = (bytes[1] >> 1) & 0b11; // 00 is reserved
        if version != 0b01 && layer != 0b00 {
            return Some("audio/mpeg");
        }
    }
    None
}

/// The 16-byte header every SQLite database file starts with, NUL included.
pub const SQLITE_MAGIC: &[u8] = b"SQLite format 3\0";

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

// ── Raster decoding: dimensions + thumbnails (LIF-418) ───────
//
// Both entry points below run the `image` crate over bytes a stranger
// uploaded, so both go through `reader_for` and inherit its decode limits. A
// 40-byte PNG header can legitimately declare a 60000x60000 canvas, and
// decoding it would allocate ~14 GB before anything noticed; the limits turn
// that into a decode error instead of an OOM kill.

/// Long edge of a generated thumbnail, in pixels. Images at or under this on
/// both axes get no thumbnail at all: the original is already small enough to
/// send, and a second nearly-identical file would only cost disk.
pub const THUMBNAIL_MAX_EDGE: u32 = 480;

/// Largest pixel count we will decode. 50 megapixels is far beyond any
/// screenshot or photo a tracker attachment plausibly holds, and caps the
/// decoded RGBA buffer at roughly 200 MB.
const MAX_DECODE_PIXELS: u64 = 50_000_000;

/// Build a limited `ImageReader` over an in-memory buffer, with the format
/// guessed from the bytes rather than taken from any header.
fn reader_for(
    bytes: &[u8],
) -> Result<image::ImageReader<std::io::Cursor<&[u8]>>, image::ImageError> {
    let mut reader = image::ImageReader::new(std::io::Cursor::new(bytes))
        .with_guessed_format()
        .map_err(image::ImageError::IoError)?;
    let mut limits = image::Limits::default();
    limits.max_image_width = Some(20_000);
    limits.max_image_height = Some(20_000);
    limits.max_alloc = Some(MAX_DECODE_PIXELS * 4);
    reader.limits(limits);
    Ok(reader)
}

/// Decode just the header of a raster image and return `(width, height)`.
/// `None` when the bytes are not a decodable raster, which is never fatal:
/// dimensions are an optimization, not a correctness requirement.
pub fn image_dimensions(bytes: &[u8]) -> Option<(u32, u32)> {
    reader_for(bytes).ok()?.into_dimensions().ok()
}

/// Downscale a raster image so its long edge is [`THUMBNAIL_MAX_EDGE`] and
/// encode the result as lossless WebP.
///
/// `Ok(None)` means "no thumbnail is warranted": the image already fits inside
/// the thumbnail box, so callers should serve the original. An `Err` means the
/// bytes would not decode, which callers treat as "no thumbnail" too rather
/// than failing the request that triggered generation.
pub fn generate_thumbnail(bytes: &[u8]) -> Result<Option<Vec<u8>>, LificError> {
    let reader =
        reader_for(bytes).map_err(|e| LificError::BadRequest(format!("undecodable image: {e}")))?;
    let (w, h) = reader
        .into_dimensions()
        .map_err(|e| LificError::BadRequest(format!("undecodable image: {e}")))?;
    if w.max(h) <= THUMBNAIL_MAX_EDGE {
        return Ok(None);
    }
    if u64::from(w) * u64::from(h) > MAX_DECODE_PIXELS {
        return Err(LificError::BadRequest(
            "image is too large to thumbnail".into(),
        ));
    }

    let image = reader_for(bytes)
        .and_then(|r| r.decode())
        .map_err(|e| LificError::BadRequest(format!("undecodable image: {e}")))?;
    // `thumbnail` preserves the aspect ratio and fits inside the box, so a
    // 1000x200 strip comes back 480x96 rather than stretched.
    let small = image.thumbnail(THUMBNAIL_MAX_EDGE, THUMBNAIL_MAX_EDGE);
    let rgba = small.to_rgba8();
    let (tw, th) = (rgba.width(), rgba.height());

    let mut out = Vec::new();
    image::codecs::webp::WebPEncoder::new_lossless(std::io::Cursor::new(&mut out))
        .encode(rgba.as_raw(), tw, th, image::ExtendedColorType::Rgba8)
        .map_err(|e| LificError::Internal(format!("encode thumbnail: {e}")))?;
    Ok(Some(out))
}

/// Whether a thumbnail is expected to exist for an attachment, given its mime
/// and recorded dimensions. This is the value serialized as `has_thumbnail`,
/// and it deliberately describes what the thumbnail endpoint will do rather
/// than what is currently on disk: generation is lazy, so "no file yet" and
/// "no thumbnail" are different states and only the second is a 404.
pub fn expects_thumbnail(mime: &str, width: Option<i64>, height: Option<i64>) -> bool {
    if !is_raster_mime(mime) {
        return false;
    }
    let long_edge = width.unwrap_or(0).max(height.unwrap_or(0));
    long_edge > i64::from(THUMBNAIL_MAX_EDGE)
}

/// Cheap check that the head of a buffer opens like SVG/XML.
fn looks_like_svg(bytes: &[u8]) -> bool {
    let head_len = bytes.len().min(512);
    let head = String::from_utf8_lossy(&bytes[..head_len]).to_ascii_lowercase();
    head.contains("<svg") || head.trim_start().starts_with("<?xml")
}

/// Image fixtures shared with the API-level attachment tests, so both layers
/// assert against bytes a real decoder will accept rather than magic-byte
/// stubs that only the sniffer is happy with.
#[cfg(test)]
pub(crate) mod fixtures {
    /// Encode a real solid-colour PNG at the requested size.
    pub(crate) fn png_image(width: u32, height: u32) -> Vec<u8> {
        let buffer = image::RgbaImage::from_pixel(width, height, image::Rgba([9, 40, 90, 255]));
        let mut out = Vec::new();
        image::DynamicImage::ImageRgba8(buffer)
            .write_to(&mut std::io::Cursor::new(&mut out), image::ImageFormat::Png)
            .expect("encode png fixture");
        out
    }
}

#[cfg(test)]
mod tests {
    use super::fixtures::png_image;
    use super::*;

    /// A store rooted in a fresh scratch directory. The caller must keep the
    /// returned [`TempDir`] alive for as long as it uses the store; dropping
    /// it removes the directory, which also happens while a failed assertion
    /// unwinds.
    fn tmp_store() -> (AttachmentStore, tempfile::TempDir) {
        let tmp = tempfile::tempdir().unwrap();
        // A subdirectory that does not exist yet, matching production, where
        // the attachments dir is created on first write.
        let store = AttachmentStore::new(tmp.path().join("attachments"));
        (store, tmp)
    }

    #[test]
    fn write_read_roundtrip_and_dedup() {
        let (store, _tmp) = tmp_store();
        let bytes = b"hello attachment world";
        let sha1 = store.write(bytes).unwrap();
        let sha2 = store.write(bytes).unwrap();
        assert_eq!(sha1, sha2, "same content hashes to same file");
        assert_eq!(store.read(&sha1).unwrap(), bytes);
        // Only one file on disk for the duplicate write.
        let count = std::fs::read_dir(store.dir()).unwrap().count();
        assert_eq!(count, 1);
    }

    #[test]
    fn delete_is_idempotent() {
        let (store, _tmp) = tmp_store();
        let sha = store.write(b"x").unwrap();
        store.delete(&sha).unwrap();
        store.delete(&sha).unwrap(); // second delete: no error
        assert!(store.read(&sha).is_err());
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
    fn only_raster_images_are_inline_safe() {
        assert!(is_inline_safe_mime("image/png"));
        assert!(is_inline_safe_mime("image/jpeg"));
        assert!(is_inline_safe_mime("image/gif"));
        assert!(is_inline_safe_mime("image/webp"));
        assert!(!is_inline_safe_mime("application/pdf"));
        assert!(!is_inline_safe_mime("text/plain"));
    }

    /// SVG is an image by MIME but a scriptable document in a browser.
    /// Serving it inline from our own origin is stored XSS, so it must never
    /// be classified inline-safe no matter how the allowlist evolves.
    #[test]
    fn svg_is_never_inline_safe() {
        assert!(!is_inline_safe_mime("image/svg+xml"));
    }

    /// Every inline-safe type must still be an accepted upload type. The two
    /// lists are allowed to differ, but not to contradict each other.
    #[test]
    fn inline_safe_types_are_all_allowed_uploads() {
        for mime in ALLOWED_MIMES {
            if is_inline_safe_mime(mime) {
                assert!(ALLOWED_MIMES.contains(mime));
            }
        }
        assert!(ALLOWED_MIMES.contains(&"image/svg+xml"));
    }

    // ── LIF-418: media types ─────────────────────────────────

    /// The smallest byte sequences that identify each media container.
    fn mp4_bytes() -> Vec<u8> {
        let mut v = vec![0, 0, 0, 0x20];
        v.extend_from_slice(b"ftypisom");
        v.extend_from_slice(b"\0\0\x02\0isomiso2avc1mp41");
        v
    }

    fn webm_bytes() -> Vec<u8> {
        let mut v = vec![0x1A, 0x45, 0xDF, 0xA3];
        // A trimmed EBML header: enough structure to carry the DocType string
        // our sniffer looks for.
        v.extend_from_slice(&[0x01, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x1F]);
        v.extend_from_slice(b"\x42\x82\x84webm");
        v.extend_from_slice(&[0; 16]);
        v
    }

    #[test]
    fn sniffs_every_new_media_container() {
        assert_eq!(sniff_and_validate(&mp4_bytes(), None).unwrap(), "video/mp4");
        assert_eq!(
            sniff_and_validate(&webm_bytes(), None).unwrap(),
            "video/webm"
        );
        let mut ogg = Vec::from(*b"OggS");
        ogg.extend_from_slice(&[0; 32]);
        assert_eq!(sniff_and_validate(&ogg, None).unwrap(), "audio/ogg");
        let mut id3 = Vec::from(*b"ID3\x03\x00\x00");
        id3.extend_from_slice(&[0; 32]);
        assert_eq!(sniff_and_validate(&id3, None).unwrap(), "audio/mpeg");
        // A bare MPEG frame header (sync + MPEG1 Layer III).
        assert_eq!(
            sniff_and_validate(&[0xFF, 0xFB, 0x90, 0x00, 0, 0, 0, 0], None).unwrap(),
            "audio/mpeg"
        );
    }

    /// The container decides it IS webm; the declared type only chooses
    /// between the two allowlisted labels for that one container.
    #[test]
    fn webm_declared_as_audio_is_recorded_as_audio() {
        assert_eq!(
            sniff_and_validate(&webm_bytes(), Some("audio/webm")).unwrap(),
            "audio/webm"
        );
        // A lie in the other direction cannot make a PNG into a video.
        assert_eq!(
            sniff_and_validate(&png_image(2, 2), Some("video/mp4")).unwrap(),
            "image/png"
        );
    }

    /// QuickTime shares the ISO base media container with mp4 but is not on
    /// the allowlist, so it must not be waved through wearing an mp4 label.
    #[test]
    fn quicktime_is_not_relabelled_as_mp4() {
        let mut mov = vec![0, 0, 0, 0x14];
        mov.extend_from_slice(b"ftypqt  ");
        mov.extend_from_slice(&[0xFF; 16]);
        // No signature match and not valid UTF-8, so nothing is left to fall
        // back to: rejected outright rather than stored as video.
        assert!(sniff_and_validate(&mov, Some("video/mp4")).is_err());
        assert_eq!(sniff_magic(&mov), None);
    }

    /// A 0xFF lead byte is not enough: the version and layer fields both have
    /// reserved encodings that a real MPEG frame never carries.
    #[test]
    fn reserved_mpeg_header_fields_are_not_audio() {
        // Version bits 01 (reserved).
        let reserved_version = [0xFF, 0xEA, 0x00, 0x00, 0xFF, 0xFE];
        assert_ne!(sniff_magic(&reserved_version), Some("audio/mpeg"));
        // Layer bits 00 (reserved).
        let reserved_layer = [0xFF, 0xF9, 0x00, 0x00, 0xFF, 0xFE];
        assert_ne!(sniff_magic(&reserved_layer), Some("audio/mpeg"));
    }

    #[test]
    fn sqlite_databases_are_allowed_and_never_inline() {
        let mut db = SQLITE_MAGIC.to_vec();
        db.extend_from_slice(&[0; 64]);
        assert_eq!(
            sniff_and_validate(&db, None).unwrap(),
            "application/vnd.sqlite3"
        );
        assert!(ALLOWED_MIMES.contains(&"application/vnd.sqlite3"));
        assert!(!is_inline_safe_mime("application/vnd.sqlite3"));
    }

    /// Media plays in place. It is not a scripting surface, and a `<video>`
    /// element cannot work against a forced download.
    #[test]
    fn media_types_serve_inline() {
        for mime in [
            "video/mp4",
            "video/webm",
            "audio/webm",
            "audio/ogg",
            "audio/mpeg",
        ] {
            assert!(is_inline_safe_mime(mime), "{mime} must serve inline");
            assert!(ALLOWED_MIMES.contains(&mime), "{mime} must be uploadable");
        }
    }

    // ── LIF-418: dimensions + thumbnails ─────────────────────

    #[test]
    fn decodes_raster_dimensions() {
        assert_eq!(image_dimensions(&png_image(37, 11)), Some((37, 11)));
        assert_eq!(image_dimensions(b"not an image at all"), None);
    }

    #[test]
    fn raster_mimes_are_the_decodable_ones() {
        for mime in ["image/png", "image/jpeg", "image/gif", "image/webp"] {
            assert!(is_raster_mime(mime));
        }
        // A vector has no intrinsic pixel size, and a video is not decoded.
        assert!(!is_raster_mime("image/svg+xml"));
        assert!(!is_raster_mime("video/mp4"));
    }

    #[test]
    fn thumbnail_fits_the_long_edge_and_preserves_aspect() {
        let thumb = generate_thumbnail(&png_image(1200, 300))
            .unwrap()
            .expect("an oversize image gets a thumbnail");
        let (w, h) = image_dimensions(&thumb).expect("thumbnail decodes");
        assert_eq!((w, h), (480, 120));
        // WebP, per the endpoint's declared content type.
        assert!(thumb.starts_with(b"RIFF"));
        assert_eq!(&thumb[8..12], b"WEBP");
    }

    /// An image already inside the thumbnail box gets none: the original IS
    /// the small version, and a second copy would only cost disk.
    #[test]
    fn small_images_get_no_thumbnail() {
        assert!(generate_thumbnail(&png_image(480, 100)).unwrap().is_none());
        assert!(generate_thumbnail(&png_image(64, 64)).unwrap().is_none());
        assert!(generate_thumbnail(b"not an image").is_err());
    }

    #[test]
    fn expects_thumbnail_tracks_mime_and_long_edge() {
        assert!(expects_thumbnail("image/png", Some(1000), Some(10)));
        assert!(expects_thumbnail("image/jpeg", Some(10), Some(1000)));
        assert!(!expects_thumbnail("image/png", Some(480), Some(480)));
        assert!(!expects_thumbnail("image/png", None, None));
        assert!(!expects_thumbnail("video/mp4", Some(1920), Some(1080)));
        assert!(!expects_thumbnail("application/pdf", Some(1920), Some(1080)));
    }

    #[test]
    fn thumbnails_round_trip_and_live_in_a_subdirectory() {
        let (store, _tmp) = tmp_store();
        let sha = store.write(&png_image(600, 600)).unwrap();
        assert!(store.read_thumb(&sha).unwrap().is_none());

        store.write_thumb(&sha, b"pretend webp").unwrap();
        assert_eq!(store.read_thumb(&sha).unwrap().unwrap(), b"pretend webp");
        assert_eq!(
            store.thumb_path_for(&sha).parent().unwrap(),
            store.dir().join("thumbs")
        );

        // Deleting the blob takes its derivative with it.
        store.delete(&sha).unwrap();
        assert!(store.read_thumb(&sha).unwrap().is_none());
        // And deleting a thumbnail that is already gone is fine.
        store.delete_thumb(&sha).unwrap();
    }
}

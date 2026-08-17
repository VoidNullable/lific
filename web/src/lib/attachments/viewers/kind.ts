// LIF-418: which viewer renders which attachment.
//
// Dispatch is mime-first, extension-second. Extension has to matter because
// the upload path stores a conservative mime: a .json, a .csv and a .patch all
// arrive as `text/plain`, and markdown-embedded attachment links carry no mime
// at all (only the id and the link label). When neither source is conclusive
// the answer is "file", i.e. today's download chip.

export type ViewerKind =
  | "image"
  | "video"
  | "audio"
  | "diff"
  | "csv"
  | "json"
  | "zip"
  | "sqlite"
  | "text"
  | "file";

/** Lowercased extension without the dot, or "" when there is none. */
export function extensionOf(filename: string): string {
  const base = filename.split(/[\\/]/).pop() ?? filename;
  const dot = base.lastIndexOf(".");
  return dot > 0 ? base.slice(dot + 1).toLowerCase() : "";
}

/** Extensions we are willing to open in the text viewer even though the stored
 *  mime is unhelpful (or absent). Source code, config, and logs: things a
 *  reader wants to skim inline rather than download. */
const TEXT_EXTENSIONS = new Set([
  "txt", "text", "log", "out", "err", "md", "markdown", "rst", "adoc",
  "rs", "ts", "tsx", "js", "jsx", "mjs", "cjs", "svelte", "vue",
  "py", "rb", "go", "java", "kt", "kts", "swift", "c", "h", "cc", "cpp",
  "hpp", "cs", "php", "pl", "lua", "r", "scala", "clj", "ex", "exs", "erl",
  "hs", "ml", "zig", "nim", "dart", "sql", "graphql", "gql", "proto",
  "sh", "bash", "zsh", "fish", "ps1", "bat", "cmd",
  "yaml", "yml", "toml", "ini", "cfg", "conf", "env", "properties",
  "html", "htm", "xml", "svg", "css", "scss", "sass", "less",
  "lock", "gitignore", "dockerfile", "makefile", "cmake", "gradle",
]);

/** Checked before the text list, which also claims `svg`: a link to an SVG is
 *  worth showing as a picture, and `<img src>` never executes script (see the
 *  is_inline_safe_mime note in src/storage.rs). */
const IMAGE_EXTENSIONS = new Set([
  "png", "jpg", "jpeg", "gif", "webp", "avif", "bmp", "ico", "svg",
]);

const DIFF_EXTENSIONS = new Set(["patch", "diff"]);
const CSV_EXTENSIONS = new Set(["csv", "tsv", "tab"]);
const ZIP_EXTENSIONS = new Set(["zip"]);
const SQLITE_EXTENSIONS = new Set(["db", "sqlite", "sqlite3"]);
const VIDEO_EXTENSIONS = new Set(["mp4", "webm", "m4v"]);
const AUDIO_EXTENSIONS = new Set(["mp3", "ogg", "oga", "opus", "weba"]);

export interface AttachmentLike {
  filename: string;
  /** Stored mime, when the caller has the attachment record. */
  mime?: string | null;
}

export function viewerKindFor(attachment: AttachmentLike): ViewerKind {
  const mime = (attachment.mime ?? "").toLowerCase().split(";")[0].trim();
  const ext = extensionOf(attachment.filename ?? "");

  // Mime wins outright for media: `.webm` alone cannot tell video from audio,
  // and the server does know which one it stored.
  if (mime.startsWith("image/")) return "image";
  if (mime.startsWith("video/")) return "video";
  if (mime.startsWith("audio/")) return "audio";
  if (mime === "application/zip") return "zip";
  if (mime === "application/vnd.sqlite3" || mime === "application/x-sqlite3") {
    return "sqlite";
  }
  if (mime === "application/json") return "json";

  // Then extension, including for text/* mimes where the extension says what
  // kind of text it is.
  if (IMAGE_EXTENSIONS.has(ext)) return "image";
  if (DIFF_EXTENSIONS.has(ext)) return "diff";
  if (CSV_EXTENSIONS.has(ext)) return "csv";
  if (ext === "json") return "json";
  if (ZIP_EXTENSIONS.has(ext)) return "zip";
  if (SQLITE_EXTENSIONS.has(ext)) return "sqlite";
  if (VIDEO_EXTENSIONS.has(ext)) return "video";
  if (AUDIO_EXTENSIONS.has(ext)) return "audio";

  if (mime.startsWith("text/")) return "text";
  if (TEXT_EXTENSIONS.has(ext)) return "text";
  // Extensionless conventional filenames (Makefile, Dockerfile, LICENSE).
  if (ext === "" && /^(makefile|dockerfile|license|readme|changelog)$/i.test(attachment.filename ?? "")) {
    return "text";
  }

  return "file";
}

/** Viewers that pull the whole file down to render it. Guarded by a size cap
 *  so a 400 MB log does not get fetched into a tab. */
export const INLINE_FETCH_KINDS: ReadonlySet<ViewerKind> = new Set([
  "text",
  "diff",
  "csv",
  "json",
]);

/** Above this, an otherwise-inlineable attachment falls back to the chip. Ten
 *  megabytes of text is already past the point where a browser renders it
 *  comfortably, and the download is one click away. */
export const MAX_INLINE_BYTES = 10 * 1024 * 1024;

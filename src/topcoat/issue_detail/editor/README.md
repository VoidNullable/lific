# Issue description editor

The Rust `editor(cx)` function renders the hidden bootstrap scaffold. After the
issue request resolves, the route mounts `assets/editor.js` with the current
`EditorProps` fields: `route`, `text`, `saved_description`, `dirty`,
`expected_seq`, and `capabilities`.

Input emits `lific:issue-detail-intent` with `EditDescription` and keeps a local
draft. Save, Cmd/Ctrl-S, and Preview explicitly commit `SaveDescription` with
the captured route, description, expected sequence, and edit revision. The route owns the
serialized write queue and acknowledges matching requests with
`lific:issue-detail-applied` (`kind: "editor"`) or
`lific:issue-detail-conflict`. A matching error event preserves the draft and
shows a retryable error. Route generations and edit revisions filter stale
completions. A successful commit returns to the rendered description; a failed
commit keeps the draft editable. Cancel and Escape discard the draft, restore
the latest server version, and cancel pending attachment work without saving.

The preview creates DOM nodes and text nodes directly. It supports common
headings, paragraphs, GFM tables, nested and task lists, quotes, code, emphasis,
safe links, and images. Attachment links and images are resolved through the
active session, and public scope blocks remote images. Raw HTML and
mention-looking text remain text and are never inserted as markup.

Description attachments use the shared attachment client and uploader. Uploads
keep the description draft unsaved. Their generated markdown replaces the remembered textarea selection, and description saves stay paused
until the upload queue finishes. Cmd/Ctrl+B and Cmd/Ctrl+I toggle emphasis;
Cmd/Ctrl+Shift+K inserts a link and suppresses the command palette shortcut.

The Node tests exercise debounce coalescing, serialization, flush ordering,
conflict recovery, explicit save, discard, and edits made while a save is in
flight. The headless Chromium tests cover the rendered editor, safe markdown
images, explicit save error rendering, Save/Cancel/Escape transitions, GFM
structures, selection-aware attachment insertion, keyboard shortcuts, pending
attachment cancellation, and a newer draft queued behind an in-flight save.

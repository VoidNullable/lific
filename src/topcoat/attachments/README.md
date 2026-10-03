# Attachment primitives

Use `Attachment`, `UploadResponse`, `Target`, and `Preview` for the existing REST payloads. `Target::list_path()` supplies the linked issue/page/comment query. Uploads with no target remain unlinked until the saved entity's markdown establishes the link.

Render `list` and `uploader` inside a screen-owned root. Load `SCRIPT` and `STYLESHEET`, create `LificTopcoatAttachments.createClient({session: window.lificSession})`, and mount `LificTopcoatAttachments.attach(root, {client, target, onUploaded, onDeleted})`. The upload callback receives the server record and a markdown snippet. The host owns inserting that snippet, refreshing the entity, and remounting the attachment projection after account or public scope changes. Dispose the old mount when replacing its root.

Pass `DeleteAccess::Uploader(user_id)` for a viewer's own uploads, `Manage` for a maintainer/admin who can delete all attachments on the displayed entity, or `ReadOnly`. REST authorizes every operation. Public scope removes upload/delete controls and refuses mutation requests before opening a transport.

Native original image/audio/video and download URLs use the existing HttpOnly session cookie contract. Native transfers preserve browser byte-range and streaming behavior. Thumbnail requests use bearer headers and display bounded WebP blobs; a 404 selects the cookie-enabled original. Structured previews use `/preview`, and bounded text previews use the original bytes endpoint.

For a caller-owned download destination, `client.streamDownload(id, {open, signal, range, filename, onProgress})` supplies response metadata to `open` and awaits each destination `write` before reading the next chunk. A destination supplies `write`, `close`, and `abort`. The outcome retains status, filename, content type, byte-range metadata, and transfer errors. HTTP failures never open a destination. Account changes abort fetch, reader, and sink; pass the host's disposal signal to cancel a download when its screen is removed.

Browser downloads bypass the buffered Rust `ApiClient::download` helper. The authenticated original REST handler currently reads the stored file into memory; the public original handler streams from disk. This module preserves those server boundaries while streaming the browser response.

//! `upload_attachment(path)`: read the file from the agent's own disk.
//!
//! Only processes that run on the agent's machine do this: the local stdio
//! server and the `lific mcp` proxies, which read the file and forward the
//! bytes themselves. The HTTP server never opens a caller-supplied path.

use std::io::Read;
use std::path::Path;

use serde_json::Value;

/// A file read for upload, with the name to store it under.
pub(crate) struct LocalFile {
    pub(crate) name: String,
    pub(crate) bytes: Vec<u8>,
}

/// Read `path` for upload, refusing anything that is not a regular file of
/// 1 to `max_bytes` bytes. Relative paths resolve against the working
/// directory of the process that reads them.
pub(crate) fn read_upload(path: &str, max_bytes: usize) -> Result<LocalFile, String> {
    let path = path.trim();
    if path.is_empty() {
        return Err("path is empty".into());
    }
    let location = Path::new(path);
    let metadata =
        std::fs::metadata(location).map_err(|error| format!("cannot read {path}: {error}"))?;
    if !metadata.is_file() {
        return Err(format!("{path} is not a regular file"));
    }
    let len = usize::try_from(metadata.len()).unwrap_or(usize::MAX);
    if len > max_bytes {
        return Err(format!("file too large: {len} bytes (max {max_bytes})"));
    }
    // The size can change between the stat and the read, so the read is
    // bounded too.
    let mut bytes = Vec::with_capacity(len);
    std::fs::File::open(location)
        .and_then(|file| file.take(max_bytes as u64 + 1).read_to_end(&mut bytes))
        .map_err(|error| format!("cannot read {path}: {error}"))?;
    if bytes.len() > max_bytes {
        return Err(format!(
            "file too large: over {max_bytes} bytes (max {max_bytes})"
        ));
    }
    if bytes.is_empty() {
        return Err("empty file".into());
    }
    let name = location
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_default();
    Ok(LocalFile { name, bytes })
}

/// Rewrite an outbound `upload_attachment` call that names a `path` into one
/// that carries the file as `content_base64`, so a proxy can send the bytes
/// without the model ever emitting them.
///
/// `None` leaves the message untouched: another method or tool, or no `path`.
/// `Some(Err)` is a tool error for the caller; the call is not forwarded.
pub(crate) fn inline_upload_path(
    message: &Value,
    max_bytes: usize,
) -> Option<Result<Value, String>> {
    if message.get("method").and_then(Value::as_str) != Some("tools/call") {
        return None;
    }
    let params = message.get("params")?;
    if params.get("name").and_then(Value::as_str) != Some("upload_attachment") {
        return None;
    }
    let arguments = params.get("arguments")?.as_object()?;
    let path = match arguments.get("path") {
        None | Some(Value::Null) => return None,
        Some(Value::String(path)) => path,
        Some(_) => return Some(Err("path must be a string".into())),
    };
    if arguments
        .get("content_base64")
        .and_then(Value::as_str)
        .is_some_and(|content| !content.is_empty())
    {
        return Some(Err("pass path or content_base64, not both".into()));
    }
    let file = match read_upload(path, max_bytes) {
        Ok(file) => file,
        Err(error) => return Some(Err(error)),
    };

    use base64::Engine;
    let mut patched = message.clone();
    let arguments = patched["params"]["arguments"]
        .as_object_mut()
        .expect("arguments checked above");
    arguments.remove("path");
    arguments.insert(
        "content_base64".into(),
        Value::String(base64::engine::general_purpose::STANDARD.encode(&file.bytes)),
    );
    let named = arguments
        .get("filename")
        .and_then(Value::as_str)
        .is_some_and(|name| !name.trim().is_empty());
    if !named {
        arguments.insert("filename".into(), Value::String(file.name));
    }
    Some(Ok(patched))
}

/// A `tools/call` answer carrying a tool error, for a call a proxy refused
/// before forwarding it.
pub(crate) fn tool_error_response(id: &Value, message: &str) -> Value {
    serde_json::json!({
        "jsonrpc": "2.0",
        "id": id.clone(),
        "result": {
            "content": [{"type": "text", "text": format!("{}{message}", super::TOOL_ERROR_PREFIX)}],
            "isError": true,
        },
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use base64::Engine;

    fn upload_call(arguments: Value) -> Value {
        serde_json::json!({
            "jsonrpc": "2.0", "id": 4, "method": "tools/call",
            "params": {"name": "upload_attachment", "arguments": arguments}
        })
    }

    #[test]
    fn a_path_upload_becomes_base64_named_after_the_file() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("shot.png");
        std::fs::write(&file, b"\x89PNG\r\n\x1a\nrest").unwrap();
        let message = upload_call(serde_json::json!({
            "path": file.to_str().unwrap(), "entity": "ATT-1"
        }));

        let patched = inline_upload_path(&message, 1024).unwrap().unwrap();
        let arguments = &patched["params"]["arguments"];
        assert!(arguments.get("path").is_none(), "{patched}");
        assert_eq!(arguments["filename"], "shot.png");
        assert_eq!(arguments["entity"], "ATT-1");
        let bytes = base64::engine::general_purpose::STANDARD
            .decode(arguments["content_base64"].as_str().unwrap())
            .unwrap();
        assert_eq!(bytes, b"\x89PNG\r\n\x1a\nrest");
        assert_eq!(patched["id"], 4);
    }

    #[test]
    fn an_explicit_filename_wins_over_the_files_name() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("tmp123.txt");
        std::fs::write(&file, b"log").unwrap();
        let message = upload_call(serde_json::json!({
            "path": file.to_str().unwrap(), "filename": "crash.log"
        }));
        let patched = inline_upload_path(&message, 1024).unwrap().unwrap();
        assert_eq!(patched["params"]["arguments"]["filename"], "crash.log");
    }

    #[test]
    fn calls_without_a_path_pass_untouched() {
        assert!(
            inline_upload_path(
                &upload_call(serde_json::json!({"filename": "a.txt", "content_base64": "YQ=="})),
                1024
            )
            .is_none()
        );
        let other = serde_json::json!({
            "jsonrpc": "2.0", "id": 1, "method": "tools/call",
            "params": {"name": "create_issue", "arguments": {"path": "/etc/passwd"}}
        });
        assert!(inline_upload_path(&other, 1024).is_none());
    }

    #[test]
    fn unreadable_oversized_and_ambiguous_paths_are_refused() {
        let dir = tempfile::tempdir().unwrap();
        let missing = dir.path().join("nope.png");
        let error = inline_upload_path(
            &upload_call(serde_json::json!({"path": missing.to_str().unwrap()})),
            1024,
        )
        .unwrap()
        .unwrap_err();
        assert!(error.starts_with("cannot read "), "{error}");

        let error = inline_upload_path(
            &upload_call(serde_json::json!({"path": dir.path().to_str().unwrap()})),
            1024,
        )
        .unwrap()
        .unwrap_err();
        assert!(error.ends_with("is not a regular file"), "{error}");

        let big = dir.path().join("big.bin");
        std::fs::write(&big, vec![1u8; 11]).unwrap();
        let error = inline_upload_path(
            &upload_call(serde_json::json!({"path": big.to_str().unwrap()})),
            10,
        )
        .unwrap()
        .unwrap_err();
        assert_eq!(error, "file too large: 11 bytes (max 10)");

        let error = inline_upload_path(
            &upload_call(serde_json::json!({
                "path": big.to_str().unwrap(), "content_base64": "YQ=="
            })),
            1024,
        )
        .unwrap()
        .unwrap_err();
        assert_eq!(error, "pass path or content_base64, not both");
    }
}

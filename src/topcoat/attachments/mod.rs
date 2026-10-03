//! Attachment contracts shared by issue, page, comment and public readers.
//!
//! Browser transfers use the dedicated script, never the buffered JSON client's
//! download helper. Screen owners supply entity ids, permissions and destinations.
use super::session::Scope;
use serde::{Deserialize, Serialize};
use topcoat::{
    context::Cx,
    view::{BoxView, ViewExt, view},
};

pub(crate) const SCRIPT: &str = include_str!("assets/attachments.js");
pub(crate) const SCRIPT_PATH: &str = "/__topcoat-attachments.js";
pub(crate) const STYLESHEET: &str = include_str!("assets/attachments.css");
pub(crate) const STYLESHEET_PATH: &str = "/__topcoat-attachments.css";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub(crate) enum Entity {
    Issue,
    Page,
    Comment,
}
impl Entity {
    pub(crate) fn as_str(self) -> &'static str {
        match self {
            Self::Issue => "issue",
            Self::Page => "page",
            Self::Comment => "comment",
        }
    }
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct Target {
    pub(crate) entity_type: Entity,
    pub(crate) entity_id: i64,
}
impl Target {
    pub(crate) fn list_path(self) -> String {
        format!(
            "/attachments?entity_type={}&entity_id={}",
            self.entity_type.as_str(),
            self.entity_id
        )
    }
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct Attachment {
    pub(crate) id: i64,
    pub(crate) filename: String,
    pub(crate) mime: String,
    pub(crate) size_bytes: i64,
    pub(crate) uploader_id: Option<i64>,
    pub(crate) created_at: String,
    #[serde(default)]
    pub(crate) width: Option<i64>,
    #[serde(default)]
    pub(crate) height: Option<i64>,
    #[serde(default)]
    pub(crate) alt_text: Option<String>,
    #[serde(default)]
    pub(crate) has_thumbnail: bool,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct UploadResponse {
    pub(crate) id: i64,
    pub(crate) url: String,
    pub(crate) filename: String,
    pub(crate) mime: String,
    pub(crate) size: i64,
    #[serde(default)]
    pub(crate) width: Option<i64>,
    #[serde(default)]
    pub(crate) height: Option<i64>,
    #[serde(default)]
    pub(crate) alt_text: Option<String>,
    #[serde(default)]
    pub(crate) has_thumbnail: bool,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "lowercase")]
pub(crate) enum Preview {
    Zip {
        entries: Vec<ZipEntry>,
        total_entries: u64,
        truncated: bool,
    },
    Sqlite {
        tables: Vec<SqliteTable>,
    },
    None,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct ZipEntry {
    pub(crate) name: String,
    pub(crate) size: u64,
    pub(crate) compressed: u64,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct SqliteTable {
    pub(crate) name: String,
    pub(crate) rows: u64,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Variant {
    Original,
    Thumbnail,
    Preview,
}
pub(crate) fn url(scope: &Scope, id: i64, variant: Variant) -> String {
    let base = match scope {
        Scope::Private => "/api".to_owned(),
        Scope::Public(project) => format!("/public/api/projects/{}", urlencoding::encode(project)),
    };
    let suffix = match variant {
        Variant::Original => "",
        Variant::Thumbnail => "/thumbnail",
        Variant::Preview => "/preview",
    };
    format!("{base}/attachments/{id}{suffix}")
}

pub(crate) const MAX_INLINE_BYTES: i64 = 10 * 1024 * 1024;
impl Attachment {
    pub(crate) fn viewer_kind(&self) -> &'static str {
        let mime = self
            .mime
            .split(';')
            .next()
            .unwrap_or("")
            .trim()
            .to_ascii_lowercase();
        let filename = self
            .filename
            .rsplit(['/', '\\'])
            .next()
            .unwrap_or("")
            .to_ascii_lowercase();
        let extension = filename.rsplit_once('.').map_or("", |(_, ext)| ext);
        let kind = match mime.as_str() {
            value if value.starts_with("image/") => "image",
            value if value.starts_with("video/") => "video",
            value if value.starts_with("audio/") => "audio",
            "application/zip" => "zip",
            "application/vnd.sqlite3" | "application/x-sqlite3" => "sqlite",
            "application/json" => "json",
            _ => match extension {
                "png" | "jpg" | "jpeg" | "gif" | "webp" | "avif" | "bmp" | "ico" | "svg" => "image",
                "patch" | "diff" => "diff",
                "csv" | "tsv" | "tab" => "csv",
                "json" => "json",
                "zip" => "zip",
                "db" | "sqlite" | "sqlite3" => "sqlite",
                "mp4" | "webm" | "m4v" => "video",
                "mp3" | "ogg" | "oga" | "opus" | "weba" => "audio",
                "txt" | "text" | "log" | "out" | "err" | "md" | "markdown" | "rst" | "adoc"
                | "rs" | "ts" | "tsx" | "js" | "jsx" | "mjs" | "cjs" | "svelte" | "vue" | "py"
                | "rb" | "go" | "java" | "kt" | "kts" | "swift" | "c" | "h" | "cc" | "cpp"
                | "hpp" | "cs" | "php" | "pl" | "lua" | "r" | "scala" | "clj" | "ex" | "exs"
                | "erl" | "hs" | "ml" | "zig" | "nim" | "dart" | "sql" | "graphql" | "gql"
                | "proto" | "sh" | "bash" | "zsh" | "fish" | "ps1" | "bat" | "cmd" | "yaml"
                | "yml" | "toml" | "ini" | "cfg" | "conf" | "env" | "properties" | "html"
                | "htm" | "xml" | "css" | "scss" | "sass" | "less" | "lock" | "gitignore"
                | "dockerfile" | "makefile" | "cmake" | "gradle" => "text",
                _ if mime.starts_with("text/")
                    || matches!(
                        filename.as_str(),
                        "makefile" | "dockerfile" | "license" | "readme" | "changelog"
                    ) =>
                {
                    "text"
                }
                _ => "file",
            },
        };
        if matches!(kind, "text" | "diff" | "csv" | "json") && self.size_bytes > MAX_INLINE_BYTES {
            "file"
        } else {
            kind
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum DeleteAccess {
    ReadOnly,
    Uploader(i64),
    Manage,
}

impl DeleteAccess {
    fn permits(self, attachment: &Attachment) -> bool {
        match self {
            Self::ReadOnly => false,
            Self::Uploader(user_id) => attachment.uploader_id == Some(user_id),
            Self::Manage => true,
        }
    }
}

/// Screens pass server-derived delete capabilities. Public scope always renders read-only.
pub(crate) fn list<'a>(
    cx: &'a Cx,
    scope: &Scope,
    attachments: &[Attachment],
    delete_access: DeleteAccess,
) -> BoxView<'a> {
    let delete_access = match scope {
        Scope::Public(_) => DeleteAccess::ReadOnly,
        Scope::Private => delete_access,
    };
    let rows = attachments
        .iter()
        .map(|attachment| {
            let kind = attachment.viewer_kind();
            let original = url(scope, attachment.id, Variant::Original);
            let alt = attachment
                .alt_text
                .clone()
                .unwrap_or_else(|| attachment.filename.clone());
            (
                attachment.clone(),
                kind,
                original,
                alt,
                delete_access.permits(attachment),
            )
        })
        .collect::<Vec<_>>();
    view! {cx =>
        <ul class="tc-attachments">
            for (attachment, kind, original, alt, can_delete) in rows {
                <li class="tc-attachment" data-attachment-id=(attachment.id.to_string()) data-attachment-kind=(kind)>
                    <div class="tc-attachment__actions">
                        <a href=(original.as_str()) download=(attachment.filename.as_str())>(attachment.filename.as_str())</a>
                        <span>(format!("{} bytes", attachment.size_bytes))</span>
                        if can_delete {
                            <button class="tc-button" type="button" data-attachment-delete=(attachment.id.to_string())>"Delete"</button>
                        }
                    </div>
                    if kind == "image" {
                        <a href=(original.as_str()) aria-label="Open original image">
                            <img data-attachment-image=(attachment.id.to_string()) alt=(alt.as_str()) loading="lazy"
                                width=(attachment.width.map(|value| value.to_string())) height=(attachment.height.map(|value| value.to_string())) />
                        </a>
                    } else if kind == "video" {
                        <video controls="controls" preload="metadata" src=(original.as_str()) aria-label=(attachment.filename.as_str())></video>
                    } else if kind == "audio" {
                        <audio controls="controls" preload="metadata" src=(original.as_str()) aria-label=(attachment.filename.as_str())></audio>
                    } else if kind != "file" {
                        <button class="tc-button" type="button" data-attachment-preview="">"Preview"</button>
                        <pre data-attachment-content="" hidden="hidden"></pre>
                    }
                    <p data-attachment-message="" role="status" aria-live="polite"></p>
                </li>
            }
        </ul>
    }.boxed()
}

/// Link is absent for drafts; saving the entity's markdown establishes its links.
pub(crate) fn uploader<'a>(cx: &'a Cx, scope: &Scope, target: Option<Target>) -> BoxView<'a> {
    match scope {
        Scope::Public(_) => view! {cx =>}.boxed(),
        Scope::Private => {
            let entity_type = target.map(|value| value.entity_type.as_str());
            let entity_id = target.map(|value| value.entity_id.to_string());
            view! {cx =>
                <form class="tc-attachment-upload" data-attachment-upload="" data-attachment-entity=(entity_type) data-attachment-entity-id=(entity_id)>
                    <label>"Attach files "<input type="file" multiple="multiple" data-attachment-files="" /></label>
                    <div class="tc-attachment__actions">
                        <button class="tc-button" type="submit">"Upload"</button>
                        <button class="tc-button" type="button" data-attachment-cancel="" hidden="hidden">"Cancel"</button>
                    </div>
                    <progress data-attachment-progress="" max="1" value="0" hidden="hidden" aria-label="Upload progress"></progress>
                    <p data-attachment-status="" role="status" aria-live="polite"></p>
                </form>
            }.boxed()
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn attachment_targets_match_list_contract() {
        for entity in [Entity::Issue, Entity::Page, Entity::Comment] {
            assert_eq!(
                Target {
                    entity_type: entity,
                    entity_id: 42
                }
                .list_path(),
                format!("/attachments?entity_type={}&entity_id=42", entity.as_str())
            );
        }
    }
    #[test]
    fn attachment_variants_preserve_scope_and_distinct_resources() {
        assert_eq!(
            url(&Scope::Private, 8, Variant::Original),
            "/api/attachments/8"
        );
        assert_eq!(
            url(&Scope::Private, 8, Variant::Thumbnail),
            "/api/attachments/8/thumbnail"
        );
        assert_eq!(
            url(&Scope::public("lif"), 8, Variant::Preview),
            "/public/api/projects/LIF/attachments/8/preview"
        );
    }
    #[test]
    fn attachment_list_and_upload_wire_shapes_keep_server_mime_and_dimensions() {
        let row: Attachment = serde_json::from_value(serde_json::json!({"id":8,"filename":"x.png","mime":"image/png","size_bytes":90,"uploader_id":null,"created_at":"today","width":800,"height":400,"alt_text":"A diagram","has_thumbnail":true})).unwrap();
        assert_eq!(row.width, Some(800));
        assert!(row.has_thumbnail);
        let upload: UploadResponse = serde_json::from_value(serde_json::json!({"id":8,"filename":"x.png","mime":"image/png","size":90,"url":"/api/attachments/8"})).unwrap();
        assert_eq!(upload.size, row.size_bytes);
        assert_eq!(upload.alt_text, None);
        assert!(serde_json::from_value::<Preview>(serde_json::json!({"kind":"zip","entries":[{"name":"a.txt","size":9,"compressed":4}],"total_entries":1,"truncated":false})).is_ok());
    }
    #[tokio::test]
    async fn attachment_public_components_have_no_mutation_controls_and_escape_filenames() {
        let cx = Cx::default();
        let row = Attachment {
            id: 8,
            filename: "<script>.png".into(),
            mime: "image/png".into(),
            size_bytes: 90,
            uploader_id: None,
            created_at: "today".into(),
            width: Some(800),
            height: Some(400),
            alt_text: Some("A <diagram>".into()),
            has_thumbnail: true,
        };
        let html = list(&cx, &Scope::public("lif"), &[row], DeleteAccess::Manage)
            .single()
            .await
            .unwrap()
            .render(&cx);
        assert!(html.contains("&lt;script&gt;.png"));
        assert!(html.contains("/public/api/projects/LIF/attachments/8"));
        assert!(!html.contains("data-attachment-delete"));
        assert!(
            uploader(&cx, &Scope::public("lif"), None)
                .single()
                .await
                .unwrap()
                .render(&cx)
                .is_empty()
        );
    }
    #[tokio::test]
    async fn attachment_viewer_uploader_can_delete_only_owned_rows() {
        let cx = Cx::default();
        let mut row = Attachment {
            id: 8,
            filename: "own.txt".into(),
            mime: "text/plain".into(),
            size_bytes: 4,
            uploader_id: Some(1),
            created_at: "today".into(),
            width: None,
            height: None,
            alt_text: None,
            has_thumbnail: false,
        };
        let own = row.clone();
        row.id = 9;
        row.uploader_id = Some(2);
        let html = list(&cx, &Scope::Private, &[own, row], DeleteAccess::Uploader(1))
            .single()
            .await
            .unwrap()
            .render(&cx);
        assert_eq!(html.matches("data-attachment-delete=").count(), 1);
        assert!(html.contains("data-attachment-delete=\"8\""));
        assert!(!html.contains("data-attachment-delete=\"9\""));
    }
}

//! MCP resources: issues and pages by URI, for hosts that let a person pull a
//! server resource into context (an @-mention) rather than asking the model to
//! call a tool.
//!
//! `resources/read` renders exactly what `get_issue` and `get_page` return,
//! through the same code and the same role checks. `resources/list` is empty
//! on purpose: listing a tracker's contents would dump it, and the templates
//! are enough for a host to build a URI. `attachment://{id}` serves the bytes
//! `get_attachment` links to rather than inlining.

use rmcp::model::{ReadResourceResult, ResourceContents, ResourceTemplate};

use super::LificMcp;
use super::schemas::{GetIssueInput, GetPageInput};
use rmcp::handler::server::wrapper::Parameters;

const ISSUE_TEMPLATE: &str = "lific://{project}/issues/{identifier}";
const PAGE_TEMPLATE: &str = "lific://{project}/pages/{identifier}";
const ATTACHMENT_TEMPLATE: &str = "attachment://{id}";

/// The URI `get_attachment` links to and `resources/read` serves.
pub(crate) fn attachment_uri(id: i64) -> String {
    format!("attachment://{id}")
}

/// The resource templates Lific serves.
pub(crate) fn templates() -> Vec<ResourceTemplate> {
    vec![
        ResourceTemplate::new(ISSUE_TEMPLATE, "issue")
            .with_title("Lific issue")
            .with_description(
                "An issue with its details and recent comments, e.g. lific://APP/issues/APP-42",
            )
            .with_mime_type("text/markdown"),
        ResourceTemplate::new(PAGE_TEMPLATE, "page")
            .with_title("Lific page")
            .with_description("A page, e.g. lific://APP/pages/APP-DOC-3")
            .with_mime_type("text/markdown"),
        ResourceTemplate::new(ATTACHMENT_TEMPLATE, "attachment")
            .with_title("Lific attachment")
            .with_description("An uploaded file's contents, e.g. attachment://12"),
    ]
}

#[derive(Debug, PartialEq, Eq)]
enum LificUri<'a> {
    Issue(&'a str),
    Page(&'a str),
    Attachment(i64),
}

/// Parse `lific://{project}/{issues|pages}/{identifier}`. The identifier must
/// belong to the named project, so a URI cannot claim one project and read
/// another's entity.
fn parse(uri: &str) -> Option<LificUri<'_>> {
    if let Some(id) = uri.strip_prefix("attachment://") {
        return id
            .parse::<i64>()
            .ok()
            .filter(|id| *id > 0 && attachment_uri(*id) == uri)
            .map(LificUri::Attachment);
    }
    let rest = uri.strip_prefix("lific://")?;
    let mut segments = rest.split('/');
    let (project, kind, identifier) = (segments.next()?, segments.next()?, segments.next()?);
    if segments.next().is_some() || project.is_empty() || identifier.is_empty() {
        return None;
    }
    let prefix = identifier.get(..project.len() + 1)?;
    if !prefix.eq_ignore_ascii_case(&format!("{project}-")) {
        return None;
    }
    match kind {
        "issues" => Some(LificUri::Issue(identifier)),
        "pages" => Some(LificUri::Page(identifier)),
        _ => None,
    }
}

impl LificMcp {
    /// Serve `resources/read` for a `lific://` URI.
    pub(crate) fn read_lific_resource(
        &self,
        uri: &str,
    ) -> Result<ReadResourceResult, rmcp::ErrorData> {
        let Some(parsed) = parse(uri) else {
            return Err(rmcp::ErrorData::resource_not_found(
                format!(
                    "unknown resource {uri}; Lific serves {ISSUE_TEMPLATE}, {PAGE_TEMPLATE} and \
                     {ATTACHMENT_TEMPLATE}"
                ),
                None,
            ));
        };
        let text = match parsed {
            LificUri::Issue(identifier) => self.get_issue(Parameters(GetIssueInput {
                identifier: identifier.to_owned(),
                ..Default::default()
            })),
            LificUri::Page(identifier) => self.get_page(Parameters(GetPageInput {
                identifier: identifier.to_owned(),
                ..Default::default()
            })),
            LificUri::Attachment(id) => return self.read_attachment_resource(id, uri),
        };
        if let Some(error) = text.strip_prefix(super::TOOL_ERROR_PREFIX) {
            return Err(rmcp::ErrorData::resource_not_found(error.to_owned(), None));
        }
        Ok(ReadResourceResult::new(vec![
            ResourceContents::text(text, uri).with_mime_type("text/markdown"),
        ]))
    }
}

impl LificMcp {
    /// An attachment's bytes, behind exactly `get_attachment`'s read gate.
    /// UTF-8 text comes back as text, everything else as a base64 blob.
    fn read_attachment_resource(
        &self,
        id: i64,
        uri: &str,
    ) -> Result<ReadResourceResult, rmcp::ErrorData> {
        let not_found = |error: crate::error::LificError| {
            rmcp::ErrorData::resource_not_found(super::sanitize_error(error), None)
        };
        let attachment = {
            let conn = self.db.read().map_err(not_found)?;
            crate::db::queries::attachments::get_attachment(&conn, id).map_err(not_found)?
        };
        crate::api::attachments::authorize_read(
            &self.db,
            &super::current_identity(&self.db),
            &attachment,
        )
        .map_err(not_found)?;
        let bytes = self.store.read(&attachment.sha256).map_err(not_found)?;
        let contents = match String::from_utf8(bytes) {
            Ok(text) if attachment.mime.starts_with("text/") => ResourceContents::text(text, uri),
            Ok(text) => blob(text.as_bytes(), uri),
            Err(error) => blob(error.as_bytes(), uri),
        };
        Ok(ReadResourceResult::new(vec![
            contents.with_mime_type(&attachment.mime),
        ]))
    }
}

fn blob(bytes: &[u8], uri: &str) -> ResourceContents {
    use base64::Engine;
    ResourceContents::blob(base64::engine::general_purpose::STANDARD.encode(bytes), uri)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn uris_parse_only_when_the_identifier_belongs_to_the_project() {
        assert_eq!(
            parse("lific://APP/issues/APP-42"),
            Some(LificUri::Issue("APP-42"))
        );
        assert_eq!(
            parse("lific://app/pages/APP-DOC-3"),
            Some(LificUri::Page("APP-DOC-3"))
        );
        assert_eq!(parse("lific://APP/issues/OPS-1"), None);
        assert_eq!(parse("lific://APP/issues/APP-42/extra"), None);
        assert_eq!(parse("lific://APP/plans/APP-PLAN-1"), None);
        assert_eq!(parse("lific://APP/issues/"), None);
        assert_eq!(parse("attachment://12"), Some(LificUri::Attachment(12)));
        assert_eq!(parse("attachment://0"), None);
        assert_eq!(parse("attachment://012"), None);
        assert_eq!(parse("attachment://12/x"), None);
    }
}

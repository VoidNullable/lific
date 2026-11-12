//! MCP resources: issues and pages by URI, for hosts that let a person pull a
//! server resource into context (an @-mention) rather than asking the model to
//! call a tool.
//!
//! `resources/read` renders exactly what `get_issue` and `get_page` return,
//! through the same code and the same role checks. `resources/list` is empty
//! on purpose: listing a tracker's contents would dump it, and the templates
//! are enough for a host to build a URI.

use rmcp::model::{ReadResourceResult, ResourceContents, ResourceTemplate};

use super::LificMcp;
use super::schemas::{GetIssueInput, GetPageInput};
use rmcp::handler::server::wrapper::Parameters;

const ISSUE_TEMPLATE: &str = "lific://{project}/issues/{identifier}";
const PAGE_TEMPLATE: &str = "lific://{project}/pages/{identifier}";

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
    ]
}

#[derive(Debug, PartialEq, Eq)]
enum LificUri<'a> {
    Issue(&'a str),
    Page(&'a str),
}

/// Parse `lific://{project}/{issues|pages}/{identifier}`. The identifier must
/// belong to the named project, so a URI cannot claim one project and read
/// another's entity.
fn parse(uri: &str) -> Option<LificUri<'_>> {
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
                    "unknown resource {uri}; Lific serves {ISSUE_TEMPLATE} and {PAGE_TEMPLATE}"
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
        };
        if let Some(error) = text.strip_prefix(super::TOOL_ERROR_PREFIX) {
            return Err(rmcp::ErrorData::resource_not_found(error.to_owned(), None));
        }
        Ok(ReadResourceResult::new(vec![
            ResourceContents::text(text, uri).with_mime_type("text/markdown"),
        ]))
    }
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
        assert_eq!(parse("attachment://12"), None);
    }
}

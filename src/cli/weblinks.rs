//! Turning CLI output objects into links to the web UI (LIF-409).
//!
//! Extracted from [`super::http`], which grew these helpers first and was for a
//! while the only backend that could produce a `web_url`: it always knows the
//! server it is talking to. The direct-SQL backend knows one too whenever
//! `server.public_url` is configured, and there is no reason for the same
//! `lific issue get --json` to carry a link over HTTP and not over SQL. Both
//! backends now enrich through this one module, so the field names, the
//! omission rules and the URL shapes cannot drift apart.
//!
//! Every helper is *additive and total*: an object with no `identifier`, an
//! identifier that does not parse, or an id that is missing is returned
//! untouched rather than given a fabricated URL. There is no default origin
//! here on purpose — a link the caller cannot follow is worse than no link.

use serde_json::Value;

use crate::links::{IssueLinkContext, MarkdownReference, ResourceUrl};

#[derive(Clone, Copy)]
pub(super) enum IssueLinkOutput {
    Url,
    Markdown,
}

#[derive(Clone, Copy)]
pub(super) enum ResourceKind {
    Issue,
    Project,
    Page,
    Search,
}

#[derive(Clone, Copy)]
pub(super) enum CommentLocation {
    Issue,
    Page(i64),
}

impl CommentLocation {
    fn url<'a>(
        self,
        context: &'a IssueLinkContext,
        identifier: &'a str,
        comment_id: i64,
    ) -> Option<ResourceUrl<'a>> {
        match self {
            Self::Issue => context.issue_comment_url(identifier, comment_id),
            Self::Page(page_id) => context.page_comment_url(identifier, page_id, comment_id),
        }
    }

    fn markdown<'a>(
        self,
        context: &'a IssueLinkContext,
        identifier: &'a str,
        comment_id: i64,
    ) -> MarkdownReference<'a> {
        match self {
            Self::Issue => context.issue_comment_markdown(identifier, comment_id),
            Self::Page(page_id) => context.page_comment_markdown(identifier, page_id, comment_id),
        }
    }
}

pub(super) fn linked_resources(
    value: Value,
    context: &IssueLinkContext,
    output: IssueLinkOutput,
    kind: ResourceKind,
) -> Value {
    map_output_objects(value, |object| {
        linked_resource(object, context, output, kind)
    })
}

pub(super) fn map_output_objects(
    value: Value,
    mut map: impl FnMut(serde_json::Map<String, Value>) -> serde_json::Map<String, Value>,
) -> Value {
    fn map_object(
        value: Value,
        map: &mut impl FnMut(serde_json::Map<String, Value>) -> serde_json::Map<String, Value>,
    ) -> Value {
        match value {
            Value::Object(object) => Value::Object(map(object)),
            value => value,
        }
    }

    match value {
        Value::Array(values) => Value::Array(
            values
                .into_iter()
                .map(|value| map_object(value, &mut map))
                .collect(),
        ),
        value => map_object(value, &mut map),
    }
}

fn linked_resource(
    object: serde_json::Map<String, Value>,
    context: &IssueLinkContext,
    output: IssueLinkOutput,
    kind: ResourceKind,
) -> serde_json::Map<String, Value> {
    let linked_field = object
        .get("identifier")
        .and_then(Value::as_str)
        .and_then(|identifier| {
            resource_url(&object, context, identifier, kind).map(|url| match output {
                IssueLinkOutput::Url => ("web_url", url.to_string()),
                IssueLinkOutput::Markdown => (
                    "identifier",
                    MarkdownReference::linked(identifier, url).to_string(),
                ),
            })
        });
    match linked_field {
        Some((field, value)) => with_string_field(object, field, value),
        None => object,
    }
}

pub(super) fn resource_url<'a>(
    object: &serde_json::Map<String, Value>,
    context: &'a IssueLinkContext,
    identifier: &'a str,
    kind: ResourceKind,
) -> Option<ResourceUrl<'a>> {
    let id = object.get("id").and_then(Value::as_i64);
    match kind {
        ResourceKind::Issue => context.issue_url(identifier),
        ResourceKind::Project => context.project_url(identifier),
        ResourceKind::Page => context.page_url(identifier, id?),
        ResourceKind::Search => match object.get("result_type").and_then(Value::as_str)? {
            "issue" => context.issue_url(identifier),
            "page" => context.page_url(identifier, id?),
            "comment" => object
                .get("parent_page_id")
                .and_then(Value::as_i64)
                .map_or(CommentLocation::Issue, CommentLocation::Page)
                .url(context, identifier, id?),
            _ => None,
        },
    }
}

pub(super) fn linked_comments(
    value: Value,
    context: &IssueLinkContext,
    output: IssueLinkOutput,
    identifier: &str,
) -> Value {
    map_output_objects(value, |object| {
        let linked_field = object
            .get("id")
            .and_then(Value::as_i64)
            .and_then(|comment_id| {
                let location = object
                    .get("page_id")
                    .and_then(Value::as_i64)
                    .map_or(CommentLocation::Issue, CommentLocation::Page);
                location
                    .url(context, identifier, comment_id)
                    .map(|url| match output {
                        IssueLinkOutput::Url => ("web_url", url.to_string()),
                        IssueLinkOutput::Markdown => (
                            "comment",
                            location
                                .markdown(context, identifier, comment_id)
                                .to_string(),
                        ),
                    })
            });
        match linked_field {
            Some((field, value)) => with_string_field(object, field, value),
            None => object,
        }
    })
}

pub(super) fn linked_modules(
    value: Value,
    context: &IssueLinkContext,
    output: IssueLinkOutput,
    project: &str,
) -> Value {
    map_output_objects(value, |object| {
        let linked_field = object
            .get("id")
            .and_then(Value::as_i64)
            .zip(object.get("name").and_then(Value::as_str))
            .and_then(|(module_id, name)| {
                context
                    .module_url(project, module_id)
                    .map(|url| match output {
                        IssueLinkOutput::Url => ("web_url", url.to_string()),
                        IssueLinkOutput::Markdown => (
                            "name",
                            context
                                .module_markdown(project, module_id, name)
                                .to_string(),
                        ),
                    })
            });
        match linked_field {
            Some((field, value)) => with_string_field(object, field, value),
            None => object,
        }
    })
}

pub(super) fn with_string_field(
    mut object: serde_json::Map<String, Value>,
    field: &str,
    value: String,
) -> serde_json::Map<String, Value> {
    object.insert(field.into(), Value::String(value));
    object
}

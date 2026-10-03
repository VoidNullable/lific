//! Issue comments and related collaboration panels.

use super::{
    CollaborationProps, DetailEvent, DetailIntent, Intent, PanelAction, RelationKind, WaitInput,
};
use topcoat::{
    context::Cx,
    view::{BoxView, ViewExt, view},
};

pub(crate) const SCRIPT_PATH: &str = "/__topcoat-issue-collaboration.js";
pub(crate) const SCRIPT: &str = include_str!("assets/collaboration.js");
pub(crate) const STYLESHEET_PATH: &str = "/__topcoat-issue-collaboration.css";
pub(crate) const STYLESHEET: &str = include_str!("assets/collaboration.css");

pub(crate) fn panel<'a>(cx: &'a Cx, props: &'a CollaborationProps<'a>) -> BoxView<'a> {
    render(cx, props.route, Some(props.issue), props.capabilities)
}

pub(crate) fn empty_panel<'a>(
    cx: &'a Cx,
    route: super::RouteKey,
    capabilities: super::Capabilities,
) -> BoxView<'a> {
    render(cx, route, None, capabilities)
}

fn render<'a>(
    cx: &'a Cx,
    route_key: super::RouteKey,
    issue: Option<&'a super::super::api::dto::issue::Issue>,
    capabilities: super::Capabilities,
) -> BoxView<'a> {
    let route =
        serde_json::json!({"issue_id": route_key.issue_id, "generation": route_key.generation});
    let route_json = serde_json::to_string(&route).expect("route key serializes");
    let waits_json = serde_json::to_string(
        issue
            .and_then(|issue| issue.waits.as_deref())
            .unwrap_or(&[]),
    )
    .expect("wait snapshot serializes");
    let comments_enabled = capabilities.comment;
    let edit_enabled = capabilities.edit;
    let identifier = issue.map_or("", |issue| issue.identifier.as_str());
    let issue_id = issue.map_or(route_key.issue_id.to_string(), |issue| issue.id.to_string());
    let project_id = issue.map_or(String::new(), |issue| issue.project_id.to_string());
    let blocks = issue
        .and_then(|issue| issue.blocks.as_deref())
        .unwrap_or(&[])
        .join(",");
    let blocked_by = issue
        .and_then(|issue| issue.blocked_by.as_deref())
        .unwrap_or(&[])
        .join(",");
    let relates_to = issue
        .and_then(|issue| issue.relates_to.as_deref())
        .unwrap_or(&[])
        .join(",");
    let duplicates = issue
        .and_then(|issue| issue.duplicates.as_deref())
        .unwrap_or(&[])
        .join(",");
    let duplicated_by = issue
        .and_then(|issue| issue.duplicated_by.as_deref())
        .unwrap_or(&[])
        .join(",");
    view! {cx =>
        <section class="tc-collab" data-topcoat-collaboration="" data-issue-id=(issue_id.as_str())
            data-project-id=(project_id.as_str()) data-identifier=(identifier) data-route=(route_json.as_str())
            data-waits=(waits_json.as_str())
            data-comment-enabled=(comments_enabled.to_string()) data-edit-enabled=(edit_enabled.to_string())
            data-blocks=(blocks.as_str()) data-blocked-by=(blocked_by.as_str()) data-relates-to=(relates_to.as_str())
            data-duplicates=(duplicates.as_str()) data-duplicated-by=(duplicated_by.as_str())>
            <section class="tc-collab__section" aria-labelledby="tc-comments-heading">
                <header class="tc-collab__header"><h2 id="tc-comments-heading">"Comments"</h2><span data-comment-count="">"…"</span></header>
                <div data-comment-thread="" aria-live="polite"><p class="tc-collab__muted">"Loading comments…"</p></div>
                <button class="tc-button" type="button" data-comments-older="" hidden="hidden">"Load older comments"</button>
                <form data-comment-compose="" hidden="hidden">
                    <label for="tc-comment-draft">"Write a comment"</label>
                    <textarea id="tc-comment-draft" data-comment-draft="" rows="4" required="" aria-label="Write a comment"></textarea>
                    <div class="tc-collab__upload-tools"><label>"Attach files"<input type="file" multiple="" data-comment-files="new" /></label><span data-comment-upload-status="new" role="status"></span></div>
                    <div data-mention-list="" role="listbox" hidden="hidden" aria-label="Mention suggestions"></div>
                    <button class="tc-button" type="submit">"Comment"</button>
                </form>
            </section>
            <section class="tc-collab__section" aria-labelledby="tc-relations-heading">
                <header class="tc-collab__header"><h2 id="tc-relations-heading">"Relations"</h2></header>
                <ul data-relation-list=""></ul>
                <form data-relation-create="" hidden="hidden">
                    <label>"Issue identifier"<input name="target" required="" autocomplete="off" /></label>
                    <label>"Relation"<select name="kind"><option value="blocks">"Blocks"</option><option value="relates_to">"Relates to"</option><option value="duplicate">"Duplicate"</option></select></label>
                    <button class="tc-button" type="submit">"Link issue"</button>
                </form>
            </section>
            <section class="tc-collab__section" aria-labelledby="tc-waits-heading">
                <header class="tc-collab__header"><h2 id="tc-waits-heading">"Waits"</h2></header>
                <ul data-wait-list=""></ul>
                <form data-wait-create="" hidden="hidden">
                    <label>"Wait for"<select name="kind"><option value="user">"A person"</option><option value="date">"A date"</option></select></label>
                    <label data-wait-user-field="">"Username"<input name="user" autocomplete="off" /></label>
                    <label data-wait-date-field="" hidden="hidden">"From"<input name="from" type="date" /></label>
                    <label data-wait-until-field="" hidden="hidden">"Until"<input name="until" type="date" /></label>
                    <label>"Note"<input name="note" /></label>
                    <button class="tc-button" type="submit">"Add wait"</button>
                </form>
            </section>
            <section class="tc-collab__section" aria-labelledby="tc-attachments-heading">
                <header class="tc-collab__header"><h2 id="tc-attachments-heading">"Attachments"</h2></header>
                <div data-issue-attachments=""></div>
            </section>
            <section class="tc-collab__section" aria-labelledby="tc-history-heading">
                <header class="tc-collab__header"><h2 id="tc-history-heading">"History"</h2></header>
                <ol data-issue-history=""></ol>
            </section>
            <div class="tc-collab__actions">
                <button class="tc-button tc-button--danger" type="button" data-issue-delete="" hidden="hidden">"Delete issue"</button>
                <button class="tc-button" type="button" data-issue-restore="" hidden="hidden">"Restore issue"</button>
            </div>
            <p class="tc-collab__status" data-collab-status="" role="status" aria-live="polite"></p>
        </section>
    }.boxed()
}

pub(crate) fn event_route(event: &DetailEvent) -> super::RouteKey {
    match event {
        DetailEvent::ScalarApplied { route, .. }
        | DetailEvent::CollaborationApplied { route, .. }
        | DetailEvent::Conflict { route, .. }
        | DetailEvent::Deleted { route }
        | DetailEvent::Restored { route, .. } => *route,
        DetailEvent::EditorApplied { save, .. } => save.route,
    }
}

#[allow(dead_code)]
fn _contract_examples(route: super::RouteKey) {
    let _ = DetailIntent {
        route,
        action: Intent::MutatePanel(PanelAction::CreateComment {
            content: String::new(),
        }),
    };
    let _ = PanelAction::LinkRelation {
        source: String::new(),
        target: String::new(),
        kind: RelationKind::Blocks,
    };
    let _ = WaitInput::Date {
        from: String::new(),
        until: None,
        note: String::new(),
    };
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn empty_panel_keeps_hydration_targets_and_write_forms_stable() {
        let cx = Cx::default();
        let html = empty_panel(
            &cx,
            super::super::RouteKey {
                issue_id: 12,
                generation: 4,
            },
            super::super::Capabilities {
                edit: false,
                comment: false,
            },
        )
        .single()
        .await
        .unwrap()
        .render(&cx);

        for target in [
            "data-topcoat-collaboration",
            "data-comment-thread",
            "data-comment-count",
            "data-comments-older",
            "data-comment-compose",
            "data-comment-files",
            "data-relation-list",
            "data-relation-create",
            "data-wait-list",
            "data-wait-create",
            "data-issue-attachments",
            "data-issue-history",
            "data-issue-delete",
            "data-issue-restore",
        ] {
            assert!(html.contains(target), "missing stable selector {target}");
        }
        assert!(html.contains("data-route="));
        assert!(html.contains("issue_id"));
        assert!(html.contains("generation"));
        assert!(html.contains("data-comment-enabled=\"false\""));
        assert!(html.contains("data-edit-enabled=\"false\""));
        assert!(html.contains("data-comment-compose=\"\" hidden=\"hidden\""));
        assert!(html.contains("data-relation-create=\"\" hidden=\"hidden\""));
        assert!(html.contains("data-wait-create=\"\" hidden=\"hidden\""));
    }
}

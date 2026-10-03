# Plans

`list(cx, project)` mounts the private project plan list. `detail(cx, project, plan_id)` mounts a numeric plan ID. Register `SCRIPT_PATH`/`SCRIPT` and `STYLESHEET_PATH`/`STYLESHEET` on the corresponding shell routes. The script includes the shared Markdown renderer and issue picker.

The list follows the ID cursor until every plan is loaded, then sorts by update time. Active, Done, Archived, and All tabs use the existing project preference key. Creation posts the project ID and trimmed title, then opens the numeric detail route.

The detail renders the returned nested tree, linked issue status, completion provenance, activity, anchor issue, and server step/done counts. Step moves send the existing parent/root and position fields. Moving under the same step or one of its descendants is excluded from the parent picker. Completing a linked step relies on the returned plan and effect; unchecking never sends an issue reopen request. Project events and linked issue events, including links to another project, refresh the plan after active edits or dialogs finish. Pending reads cannot replace drafts or newer saved changes. PlanStepNode contains linked status and completion provenance but no blocked/workable fields, so the tree does not invent blocker labels. Shift-clicking a linked issue opens its preview.

The issue picker resolves bare numbers and identifiers and merges scoped issue search results. Explicit cross-project identifiers remain available for plan links; the server authorizes both sides. The same picker restricts module assignments to that module's project.

Unit tests cover nested traversal, move payloads, server counts, completion provenance, and picker scoping. Headless browser tests cover nested create/edit/move/order/delete, issue attachment/detachment, anchor changes, completion and reopen refresh, cursor pagination, roles, retries, and stale account responses.

Run the focused suite through the repository environment:

```sh
devenv --profile topcoat-e2e shell -- node --test src/topcoat/plans/assets/plans.test.js src/topcoat/plans/assets/plans.browser.test.js
```

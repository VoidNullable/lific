# Issue detail component contract

The route owns one `issue_detail::Coordinator` for each route activation. Its
`RouteKey` contains the issue ID and a generation that changes on navigation,
including a return to the same issue. Components receive borrowed props and emit
typed intents. Only the coordinator replaces the canonical `api::dto::issue::Issue`
and publishes its `seq` as the next `expected_seq`. Backend DTOs stay unchanged.

| Owner | Files it owns | Interface |
| --- | --- | --- |
| Shared contract | `src/topcoat/issue_detail/mod.rs` | Props, intents, completion events, pure revision and draft coordinator |
| Route and scalar fields | `src/topcoat/issue_detail/route.rs`, `fields.rs`, `assets/route.js`, `assets/fields.js`, matching CSS | Compose panels, resolve permissions, own the write queue; `ScalarProps`, `ScalarChange`, `DetailIntent` |
| Markdown editor | `src/topcoat/issue_detail/editor.rs`, `assets/editor.js`, matching CSS | Markdown input, preview, attachments, debounce and queued save requests; `EditorProps`, `EditorSave` |
| Collaboration | `src/topcoat/issue_detail/collaboration.rs`, `assets/collaboration.js`, matching CSS | Comments, relations, waits and their paging/dialog state; `CollaborationProps`, `Panel` |

The route integrator registers modules and assets. Each component exports its
renderer and runtime initializer from its own files. Component workers do not
edit shared registrations or another component's files. JavaScript adapters use
the same event payloads; the Rust coordinator is the reference for transitions.

## Dispatch and completion

`DetailIntent` always carries the originating `RouteKey`. Scalar fields emit
`SetScalar`; editor input emits `EditDescription`, followed by `SaveDescription`.
Collaboration submits panel mutations through the route's queue and emits
`MutatePanel(PanelAction)` for comment create/edit/delete, relation link/unlink/
reverse, or wait add/clear. The adapter emits `PanelMutationCompleted` after its
endpoint succeeds. `WaitInput` makes user and date inputs mutually exclusive;
`RelationKind` maps to the existing `blocks`, `relates_to`, and `duplicate`
values. Delete and restore also belong to the route queue. Permissions come from
the existing session/project
role resolution: `edit` controls scalar/editor/relations/waits and `comment`
controls comment creation; author/admin rules still control comment edits and
deletes. Server authorization remains authoritative.

The route serializes writes for this issue. It coalesces pending description
saves and creates `EditorSave` only when a save reaches the front of the queue,
using the coordinator's current `expected_seq`, text and edit revision. Scalar
updates also read `expected_seq` at dispatch. It never captures that sequence at
the first keystroke. A failed request preserves the draft and does not count as
an acknowledgement. Navigation disposes the queue and its subscriptions; a late
response from another route generation cannot affect the next coordinator.

| Operation | Existing response | Completion published to the coordinator |
| --- | --- | --- |
| Scalar update (`PUT /issues/{id}`) | Full issue | `ScalarApplied` with its issue sequence |
| Description save (same endpoint, `description` and `expected_seq`) | Full issue | `EditorApplied` with the exact `EditorSave` used for that request |
| Comment create/edit/delete | Comment row or deletion boolean | Update the comment window, fetch `GET /issues/{id}`, then `CollaborationApplied(Comments)` |
| Relation link/unlink/reverse | Success boolean | Fetch the issue, then `CollaborationApplied(Relations)` |
| Wait add/clear | Wait row or success boolean | Fetch the issue, then `CollaborationApplied(Waits)` |
| Issue delete | Deletion boolean | `Deleted`; stop queued writes and retain any unsaved draft for undo |
| Issue restore | Full issue | `Restored` with the returned issue and sequence |
| `409 update_conflict` | `current` issue | `Conflict`; publish its sequence without discarding the draft or silently retrying it |

Comment sequences and stream cursors are different from the parent's issue
sequence. Never send a comment's `seq` as the issue's `expected_seq`. A panel
mutation remains at the front of the queue until its issue refresh succeeds;
if the refresh fails, retry the refresh before dispatching another protected
write. Do not add sequence fields to existing endpoint responses. An SSE event
invalidates data; its cursor alone is not an issue snapshot.

## Draft and sequence rules

Canonical issue sequences advance monotonically. Every completion must belong
to the active `RouteKey` and issue ID. An older issue snapshot is ignored.
Scalar, collaboration, conflict and restore snapshots update the saved issue;
they never replace or clear dirty editor text. A clean editor follows refreshed
saved descriptions.

An editor acknowledgement can clear the draft only when its captured edit
revision still matches the current draft and its issue snapshot is current.
Typing during a save leaves the new text dirty even when the earlier save
succeeds. A response from an older save cannot overwrite a newer acknowledged
description or roll back `expected_seq`. Conflict reconciliation shows both the
current server value and the preserved local draft so the user can explicitly
choose what to save next.

The contract tests cover a scalar response advancing the next editor save's
sequence, collaboration preserving dirty text, and an older editor response
leaving newer text and sequence intact.

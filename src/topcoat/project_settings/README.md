# Project setup and administration

`new_project` mounts project creation. `archive_import` mounts complete project transfer.
`administration` composes below the dashboard overview on both `/PROJECT/overview` and
its `/PROJECT/settings` alias. The dashboard owns attention, progress and activity;
this module owns the single inline identity editor through `[data-project-identity]`.

Load `SCRIPT` and `STYLESHEET` once after the shared session runtime. The runtime
mounts `[data-topcoat-project-settings]` with `new`, `archive` or `settings` mode.
`attach(root, {session, win})` also supports explicit mounting and returns
`controller`, `refresh` and `dispose`.

JSON operations use `lificSession.request`; request bodies are serialized only at
that boundary. Binary archive download and progress-reporting multipart upload
use private same-origin endpoints with the current session bearer. Responses are
discarded after account or scope changes. Unknown archive outcomes survive page
reload in account-specific session storage and block repeat imports until the
user checks the project list.

Project creation reports group-assignment failures separately because the project
already exists. Label merges retain the server's target label. Group and project
orders use the server response rather than optimistically changing visible order.
Lead/admin and maintainer affordances follow project role capabilities. An exact
recent-auth refusal preserves the action, refreshes the same account and retries
once after password confirmation.

Node tests cover validation, partial setup, permission boundaries, role endpoint
methods, color normalization, merge results, ordering failures, GitHub preview
snapshots, recent-auth retry, archive progress and unknown outcomes, and account
changes. Chromium tests run headlessly under `topcoat-e2e` and exercise the shared
identity editor, failed group rollback and archive retry acknowledgement.

Project and membership events, catalogue changes, reconnects and window recovery
refresh the authoritative project and role. Refresh waits for active mutations or
archive processing, disables edits until the reply, and retains archive reports.
Identifier changes adopt the server identifier and navigate to its overview.
Member writes retain display metadata from current members or user summaries.
Archive downloads require a project lead or instance admin even when general
project authorization enforcement is off.
A project refresh cancels a pending password confirmation with a visible notice
asking the user to submit the action again. The notice survives later refreshes;
resubmitting uses the refreshed project permissions. Refresh waits while password
confirmation itself is in progress.

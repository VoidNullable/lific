# Issue list and board

`screen(cx, project_identifier, layout)` mounts one shared list/board surface.
A missing project identifier selects the workspace's accessible projects.
Load the session and sync bridges before `SCRIPT_PATH`; include
`STYLESHEET_PATH` in the document head. The component starts itself after the
DOM is ready. Route owners that replace its DOM call the mount's
`_lificIssueList.dispose()` first.

Project and workspace screens consume complete project snapshots from
`lificSync`. Public screens read the published project index directly through
`lificSession` and expose no saved-view, write, or export controls. The server
remains the permission boundary. Selection is ephemeral and retains surviving,
visible issue IDs across sync updates. Pending writes hold replica updates
until their results land; newer replica sequences then replace local results.

Filters, sorting, grouping, density, lanes and hidden columns share the same
saved-view configuration as the existing frontend. Existing local/session
storage keys are reused. Query parameters carry a portable configuration and
preserve unrelated parameters. Layout changes replace the current issue/board
URL. Existing shell issue subtabs send `lific:subtab-change`; the component uses
those tabs when available and supplies its own tabs otherwise.

Bulk writes use individual `PUT /issues/{id}` calls. Partial outcomes retain
failed IDs and name every failed issue. Undo restores each successful target's
own previous field values. Delete is deferred five seconds and survives retries
and route unmount. Link navigation completes pending deletions before leaving.
Reload restores the remaining Undo window from session storage; records use an
account fingerprint without storing credentials. Recovered Undo is removed when
the delete request starts. Queued issue IDs stay hidden through replica updates
until Undo or completion removes their record. Removing a record reconciles
mounted screens with the replica so failed deletions reappear. Account changes cancel
undispatched deletions and clear the prior account's Undo patches and messages.
Completion checks the account again after replica refresh before publishing feedback.
When storage is unavailable, deletion completes before reporting success.
Exports stream the
canonical issue export endpoint, combine documents with Markdown separators,
and enforce the existing 16 MiB aggregate limit.

The browser attachment returns `{controller, dispose}`. Its controller provides
`load`, `change`, `select`, `selectAll`, `clear`, `bulk`, `addLabel`, `move`,
`scheduleDelete`, `undoDelete`, `undoWrite`, and saved-view operations. DOM text
is built with `textContent`, including issue previews and API failures. Native
modal peek restores its trigger's focus when closed.

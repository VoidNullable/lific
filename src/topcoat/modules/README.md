# Modules

`list(cx, project)` mounts the private module list. `detail(cx, project, module_id)` mounts a numeric module ID. Register the embedded script and stylesheet constants with those shell routes. The script includes the shared Markdown renderer and project issue picker.

Module metadata uses the current module REST endpoints. The list takes issue counts from the shell's complete sync read model, including live updates, rather than a capped issue list. Module detail retains the existing project/module-filtered request with a 500-row limit. Done issues form the numerator; every module issue, including cancelled issues, forms the denominator.

The list preserves lifecycle tabs and their project preference keys. Detail supports name, lifecycle status, emoji or `lucide:` icon values, explicit description Save/Cancel, deletion, issue search, project-scoped assignment and removal, and creation links with the numeric module prefill. Blocked/workable labels come from the server’s project/module-scoped `blocked=true` and `workable=true` filters. Plain issue rows have no workable field. Wait-only blockers use the blocked membership even when there are no issue blocker names. Metadata creation, editing, and deletion follow the server’s Lead/admin gate in legacy mode and Maintainer gate with enforcement enabled; issue assignments use their separate issue-write gate.

Unit tests cover lifecycle filtering, count semantics, and authoritative blocker labels. Headless browser tests cover read-model counts beyond 500 rows, metadata writes and rollback, description Save/Cancel, assignment scoping and nullable removal, creation, issue navigation, metadata and assignment permission differences, refresh reads during drafts, form preservation across scalar and assignment acknowledgements, retries, stale account responses, and deletion.

Run the focused suite through the repository environment:

```sh
devenv --profile topcoat-e2e shell -- node --test src/topcoat/modules/assets/modules.test.js src/topcoat/modules/assets/modules.browser.test.js
```

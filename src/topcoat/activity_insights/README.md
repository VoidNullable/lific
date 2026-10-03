# Project activity, insights, and dependency graph

`mod.rs` renders three project route mounts. `assets/routes.js` owns their private-session requests and interaction lifecycle. `assets/model.js` contains the existing layered/grid layout, bounded line diff, and trend curve algorithms, plus pure data mapping.

## Server contracts

- Activity loads `/projects/{id}/activity?limit=50&offset={offset}` and `/projects/{id}/activity/actors`. Actor counts are all time. Actor, text, and date filters apply to loaded history; Load more remains available when no loaded entries match. The actor filter distinguishes the system bucket from Everyone. Filters and unrelated URL parameters survive refresh.
- Activity refreshes on matching project events, resync, focus/visibility, and a 15-second baseline. Fresh entries prepend without losing older loaded entries. Resync discards stale private data and resolves the project again. Account changes clear content and invalidate pending reads.
- Insights requests `/projects/{id}/insights?weeks={4|12|26|52}`. Default window is 12 weeks. Current status/priority/module distributions remain independent of the weekly trend window; top actors use the server's selected window. Closure counts are rendered as supplied by the server. Missing series observations display as unavailable and break chart lines; zero remains zero. Insights has no realtime or focus refresh subscription.
- Graph loads `/issues?project_id={id}&limit=1000`, project relations, and a fresh project role. Only relations with both visible endpoints define Linked membership. Closed issues are excluded by default. Blocking edges control layering; other relation types control clustering. Cycles retain every node and edge. Unlinked issues use the existing grid layout.
- Graph writes use `/issues/link`, `/issues/unlink`, and the atomic `/issues/reverse` endpoint. Server rejection retains the dialog for retry. If a write succeeds but the subsequent read fails, Retry only reloads data, avoiding a second reversal. Graph coordinates are temporary and recomputed after refresh, mutation, or filter changes.

## Reachable interactions

Graph nodes navigate to issue detail. Hover/focus previews fetch full descriptions; touch long press exposes the same preview. Connect handles open a relation chooser after dragging to another node. Source/target/type selectors provide the same create action from the keyboard. Edge management and endpoint links are available in the text alternative. Viewers receive links and exploration controls; relation editing requires the project edit capability. Arrow keys pan the focused canvas, plus/minus zoom, and Home fits the graph. Node positions can be moved without writing coordinates to the server.

Charts expose per-week SVG labels and a complete data table. Activity rows use native details/summary controls and show local/UTC timestamps, transport, actor standing, entity links, old/new values, and a bounded multiline diff.

## Integration

Expose this module in `src/topcoat/mod.rs`. Mount `activity(cx, project)`, `insights(cx, project)`, and `graph(cx, project)` for the private Activity, Insights, and Graph pages. Serve `SCRIPT` at `SCRIPT_PATH` and `STYLESHEET` at `STYLESHEET_PATH`, and load both assets with the private shell. The script mounts only `[data-topcoat-analytics]` roots.

## Tests

`assets/model.test.js` proves actor/system/date/query/project filtering, entity destinations, missing-versus-zero weekly inputs, linkage under closed visibility, cycle-safe layout/clustering, and changed-line retention through context folding.

`assets/routes.browser.test.js` uses headless Chromium. It proves URL/filter persistence and pagination, activity refresh and resync, all insight windows and metric values, insight isolation from realtime events, graph layout/closed filters/keyboard navigation/previews/links, relation payloads and retry, atomic reverse behavior after a failed graph reload, viewer affordances, and rejection of results from a prior account.

Run with:

```sh
devenv --profile topcoat-e2e shell -- node --test src/topcoat/activity_insights/assets/model.test.js src/topcoat/activity_insights/assets/routes.browser.test.js
```

# Dashboard integration

Mount `home(cx)` at `/` and `overview(cx, identifier)` at a private project's
overview URL. Both return the same accessible loading frame and bind through
`data-topcoat-dashboard`. Load the module's stylesheet and script after the
shared session and sync scripts. Project creation and settings belong to their
own route owners.

The browser controller uses `lificSession.request` and the existing REST
endpoints. Home reads projects, personal project groups, active and todo issues,
all pages, and activity for at most three projects. Overview resolves its own
identifier and reads issue counts, project issues, activity, and the caller's
effective role. Neither screen writes project administration data.
The role read uses the shared session's forced refresh so its cache and other
controls receive the same permissions as the dashboard.

Home keeps the project's canonical ordering within each personal group. Work
cards retain the existing count/name group ranking, active/priority/update issue
ranking, six-row cap, eight pinned/recent rows, and ten activity rows. Overview
keeps the existing priority/age/staleness ranking and six-row attention cap;
completion uses server counts rather than the capped issue list.

Requests are owned by a session generation. Account/public transitions abort
pending requests and clear private data. Project access denial clears overview
content. Secondary service failures name the affected section; temporary role
lookup failure preserves the shared session's legacy display policy. Definitive
role denial removes actions or the inaccessible overview.

The controller consumes `lific:realtime` for project/issue/page/comment changes.
It debounces refreshes for 750 ms with a five-second maximum wait. The shared
sync client reports activity baselines through `lific:sync-change` and forwards
resync as a realtime invalidation. Resync immediately clears dashboard data and
starts a fresh request even before a baseline exists. Focus, online, visible-page, catalog,
recent-storage, and cached-history changes refresh through the same controller.

`dashboard.test.js` covers the derived dashboard data and request lifecycle.
`dashboard.browser.test.js` runs Chromium headlessly to cover rendered links,
safe titles, narrow layouts, keyboard focus, live role/count changes, access
recovery, shared resync, cached history, and authentication loading states.
`dashboard.session.browser.test.js` runs the real session bridge to cover role
cache replacement and loads the bundled mascot PNG through its Topcoat asset
path. The server serves `MASCOT` at `MASCOT_PATH` as `image/png`.

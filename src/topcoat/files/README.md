# Topcoat Files browser

`screen(cx, project_identifier)` renders the private `/{PROJECT}/files` mount. Register `SCRIPT_PATH` and `STYLESHEET_PATH` with the Topcoat asset handlers and mount the screen from the existing project route. The bundle embeds the shared attachment client before the Files controller; `lificSession` owns authenticated and scoped API requests.

The browser loads the project, role, attachment inventory, and pending orphan inventory. It preserves the legacy MIME and uploader filters, created/size/filename sorting, 50-row pagination, project byte/count totals, expandable where-used links and duplicate matches, orphan countdown, and uploader/admin/edit delete gates. Deletion asks for confirmation and reports backend failures. It does not add bulk or relink controls: the current legacy Files route exposes per-file delete and navigation, and linking occurs through issue/page/comment editors.

Selecting a file opens its preview. Images use the shared thumbnail stream and fall back to original bytes; text and structured formats use the shared preview client, and audio/video retain browser range playback through scoped attachment URLs. Unsupported or failed previews keep a direct original-download link. The screen clears loaded private rows and closes previews on an account or scope change.

Run tests with:

```sh
node --test src/topcoat/files/assets/files.test.js
devenv --profile topcoat-e2e shell -- node --test src/topcoat/files/assets/files.browser.test.js
```

The browser test launches Playwright headlessly.

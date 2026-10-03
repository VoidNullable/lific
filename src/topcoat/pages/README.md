# Topcoat pages

The pages route family has server-rendered project list and detail mount points and a browser controller driven by `lificSession.request`. `list` and `detail` take a project identifier and public-scope flag. A resolved page with `project_id: null` follows the existing workspace-admin permission path even when opened from the supported project page-detail route.

Register `SCRIPT_PATH` and `STYLESHEET_PATH` as the Topcoat page assets. The script embeds the shared attachment client before the page controller so markdown attachment references use the session's scoped, authenticated stream reader. Private and public route resolution stays in `lificSession` and the attachment client.

The list loads project pages and folders, filters by title/status/folder, creates pages, and pins rows.

The detail keeps body edits local until Save and restores the server body on Cancel. Title, lifecycle, pin, and labels save on change. Comments have their own capability gate, use markdown rendering, and support create/edit/delete plus keyset paging. Public pages render only with the public session and expose no write controls. Markdown output escapes HTML and only embeds in-scope attachment references through the existing attachment client.

Run focused tests with:

```sh
node --test src/topcoat/pages/assets/pages.test.js
devenv --profile topcoat-e2e shell -- node --test src/topcoat/pages/assets/pages.browser.test.js
```

The browser test uses the repository's configured Playwright executable and runs headlessly.

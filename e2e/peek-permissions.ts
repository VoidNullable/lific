#!/usr/bin/env bun
// Exercise the shipped preview and role derivation with bounded synthetic HTTP responses.
import { strict as assert } from "node:assert";
import { resolve } from "node:path";
import { chromium } from "playwright";
import { createServer } from "../web/node_modules/vite/dist/node/index.js";

const root = resolve(import.meta.dir, "../web");
const fixtureId = root + "/src/PeekPermissionsFixture.svelte";
const fixture = `<script>
  import PeekPanel from './lib/issues/PeekPanel.svelte';
  import { openPeek } from './lib/issues/peek.svelte';
  import { projectRole } from './lib/projectRole.svelte';
  // The route behind the preview belongs to a different, editable project.
  projectRole.projectId = 1;
  projectRole.role = 'maintainer';
  projectRole.enforced = true;
  projectRole.loaded = true;
  window.openPreview = openPeek;
</script>
<button onclick={() => openPeek('VIEW-1')}>Preview issue</button>
<PeekPanel navigate={() => {}} />`;

const server = await createServer({
  root,
  logLevel: "error",
  server: { host: "127.0.0.1", port: 0, strictPort: false },
  plugins: [{
    name: "peek-permissions-fixture", enforce: "pre",
    resolveId(id) { if (id === "/src/PeekPermissionsFixture.svelte") return fixtureId; },
    load(id) { if (id === fixtureId) return fixture; },
    configureServer(vite) {
      vite.middlewares.use(async (req, res, next) => {
        if (req.url?.split("?")[0] !== "/") return next();
        res.setHeader("Content-Type", "text/html");
        res.end(await vite.transformIndexHtml("/", `<!doctype html><div id="app"></div><script type="module">import {mount} from 'svelte'; import Fixture from '/src/PeekPermissionsFixture.svelte'; import '/src/app.css'; mount(Fixture,{target:document.getElementById('app')});</script>`));
      });
    },
  }],
});
const deadline = setTimeout(() => { console.error("Peek permission test deadline exceeded"); process.exit(1); }, 90_000);
let browser;
try {
  await server.listen();
  browser = await chromium.launch({ headless: true, channel: "chromium" });
  const cases = [
    { name: "viewer", role: "viewer", enforced: true, is_admin: false, editable: false },
    { name: "maintainer", role: "maintainer", enforced: true, is_admin: false, editable: true },
    { name: "admin", role: null, enforced: true, is_admin: true, editable: true },
    { name: "legacy", role: "viewer", enforced: false, is_admin: false, editable: true },
    { name: "unavailable", role: null, enforced: true, is_admin: false, editable: false },
  ];
  for (const policy of cases) {
    const context = await browser.newContext({ viewport: { width: 1280, height: 900 }, reducedMotion: "reduce" });
    const page = await context.newPage();
    page.setDefaultTimeout(8_000);
    const errors: string[] = [];
    const writes: unknown[] = [];
    const roleProjects: string[] = [];
    let release!: () => void;
    const gate = new Promise<void>(resolve => release = resolve);
    const issue = { id: 22, project_id: 2, identifier: "VIEW-1", title: "Viewer preview fixture", description: "Synthetic issue", status: "todo", priority: "high", module_id: 3, labels: [], blocks: [], blocked_by: [], relates_to: [], duplicates: [], created_at: "2026-09-18 10:00:00", updated_at: "2026-09-18 10:00:00" };
    page.on("pageerror", error => errors.push(error.message));
    await page.route("**/api/**", async route => {
      const request = route.request();
      const path = new URL(request.url()).pathname;
      if (request.method() !== "GET") {
        writes.push(request.postDataJSON());
        Object.assign(issue, request.postDataJSON());
        return route.fulfill({ json: issue });
      }
      if (path.endsWith("/my-role")) {
        roleProjects.push(path);
        await gate;
        return route.fulfill(policy.name === "unavailable" ? { status: 503, json: { error: "Unavailable" } } : { json: policy });
      }
      if (path.includes("/issues/resolve/")) return route.fulfill({ json: issue });
      if (path === "/api/modules") return route.fulfill({ json: [{ id: 3, project_id: 2, name: "Tracking", emoji: null }] });
      return route.fulfill({ json: [] });
    });
    await page.goto(server.resolvedUrls!.local[0]);
    await page.getByRole("button", { name: "Preview issue", exact: true }).click();
    const panel = page.getByRole("dialog", { name: "VIEW-1 preview" });
    await panel.waitFor();
    // Role resolution is deliberately held. An unrelated maintainer role must
    // not leak editing affordances into a viewer preview during that interval.
    assert.equal(await panel.getByRole("button", { name: issue.title, exact: true }).count(), 0, `${policy.name}: title editable before issue-project permissions resolve`);
    for (const name of ["Todo", "High", "Tracking"]) assert.equal(await panel.getByRole("button", { name, exact: true }).count(), 0);
    release();
    if (policy.editable) {
      await panel.getByRole("button", { name: issue.title, exact: true }).waitFor();
      for (const name of ["Todo", "High", "Tracking"]) await panel.getByRole("button", { name, exact: true }).waitFor();
      await panel.getByRole("button", { name: issue.title, exact: true }).click();
      await panel.getByRole("textbox").fill("Changed by an editor");
      await panel.getByRole("textbox").press("Tab");
      await panel.getByRole("button", { name: "Changed by an editor", exact: true }).waitFor();
      assert.deepEqual(writes, [{ title: "Changed by an editor" }]);
    } else {
      await panel.getByText(policy.name === "unavailable" ? "Editing permissions could not be checked." : "Read-only access", { exact: true }).waitFor();
      await panel.getByRole("heading", { name: issue.title, exact: true }).waitFor();
      for (const name of ["Todo", "High", "Tracking"]) assert.equal(await panel.getByRole("button", { name, exact: true }).count(), 0);
      assert.equal(await panel.getByRole("textbox").count(), 0);
      for (const text of ["Todo", "High", "Tracking"]) await panel.getByText(text, { exact: true }).waitFor();
      assert.deepEqual(writes, []);
    }
    assert.deepEqual(roleProjects, ["/api/projects/2/my-role"]);
    assert.deepEqual(errors, []);
    await context.close();
    console.log(`PASS ${policy.name}: issue-project permissions govern peek`);
  }
} finally {
  await browser?.close();
  await server.close();
  clearTimeout(deadline);
}

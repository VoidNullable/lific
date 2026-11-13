#!/usr/bin/env bun
// The Attachments panel. Insert: before the body was ever focused, Insert on
// an image puts the image's Markdown (alt text included) at the end of the
// body editor with the caret after it, and the image renders in place. Details: the alt
// text can be edited and persists across a reload, and "Used in" links every
// issue that references the file.
import { strict as assert } from "node:assert";
import { resolve } from "node:path";
import { chromium } from "playwright";
import { createServer } from "../web/node_modules/vite/dist/node/index.js";

const root = resolve(import.meta.dir, "../web");
const fixtureId = root + "/src/AttachmentInsertFixture.svelte";
const fixture = `<script>
  import EditableMarkdown from './lib/EditableMarkdown.svelte';
  import AttachmentSection from './lib/AttachmentSection.svelte';
  import { EditorView } from '@codemirror/view';
  let body = $state('Intro paragraph.');
  let editor = $state(null);
  window.saved = [];
  window.view = () => EditorView.findFromDOM(document.querySelector('.cm-editor'));
</script>
<main style="max-width: 700px">
  <EditableMarkdown bind:this={editor} value={body}
    attachTo={{ entity_type: 'page', entity_id: 1 }}
    onSave={async (next) => { window.saved.push(next); body = next; return { ok: true, body: next, seq: null }; }} />
  <AttachmentSection entityType="page" entityId={1}
    onInsert={(snippet) => editor?.insertMarkdown(snippet)} />
</main>`;
const originalHtml = await Bun.file(root + "/index.html").text();
const html = originalHtml.replace(
  '<script type="module" src="/src/main.ts"></script>',
  `<script type="module">import {mount} from 'svelte'; import Fixture from '/src/AttachmentInsertFixture.svelte'; import '/src/app.css'; mount(Fixture,{target:document.getElementById('app')});</script>`,
);
const server = await createServer({
  root,
  logLevel: "error",
  server: { host: "127.0.0.1", port: 0, strictPort: false },
  plugins: [
    {
      name: "attachment-insert-fixture",
      enforce: "pre",
      resolveId(id) {
        if (id === "/src/AttachmentInsertFixture.svelte") return fixtureId;
      },
      load(id) {
        if (id === fixtureId) return fixture;
      },
      configureServer(vite) {
        vite.middlewares.use(async (req, res, next) => {
          if (req.url?.split("?")[0] !== "/") return next();
          res.setHeader("Content-Type", "text/html");
          res.end(await vite.transformIndexHtml("/", html));
        });
      },
    },
  ],
});
const deadline = setTimeout(() => {
  console.error("Attachment panel test deadline exceeded");
  process.exit(1);
}, 90_000);

// A 1x1 PNG.
const PNG = Buffer.from(
  "iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAIAAACQd1PeAAAADElEQVR4nGP4z8AAAAMBAQDJ/pLvAAAAAElFTkSuQmCC",
  "base64",
);
const attachment: Record<string, unknown> = {
  id: 7,
  filename: "login.png",
  mime: "image/png",
  size_bytes: PNG.length,
  uploader_id: 1,
  created_at: "2026-11-12T00:00:00Z",
  alt_text: "The login dialog",
  has_thumbnail: false,
};

let browser;
try {
  await server.listen();
  const executablePath = process.env.PLAYWRIGHT_EXECUTABLE_PATH;
  browser = await chromium.launch(
    executablePath ? { headless: true, executablePath } : { headless: true, channel: "chromium" },
  );
  const page = await browser.newPage({ viewport: { width: 1000, height: 900 } });
  page.setDefaultTimeout(8_000);
  const errors: string[] = [];
  page.on("pageerror", (error) => errors.push(error.message));
  // Playwright tries the most recently added route first.
  await page.route("**/api/**", (route) => {
    const path = new URL(route.request().url()).pathname;
    if (path === "/api/auth/me") return route.fulfill({ json: { id: 1, username: "fixture" } });
    if (path === "/api/attachments") return route.fulfill({ json: [attachment] });
    return route.fulfill({ json: [] });
  });
  const patches: unknown[] = [];
  await page.route(/\/api\/attachments\/7/, (route) => {
    const request = route.request();
    const path = new URL(request.url()).pathname;
    if (request.method() === "PATCH") {
      const body = request.postDataJSON();
      patches.push(body);
      attachment.alt_text = body.alt_text;
      return route.fulfill({ json: attachment });
    }
    if (path.endsWith("/links")) {
      return route.fulfill({
        json: {
          entities: [
            { entity_type: "issue", entity_id: 1, identifier: "APP-1", title: "Login fails", page_id: null },
            { entity_type: "issue", entity_id: 2, identifier: "OPS-9", title: "Audit screens", page_id: null },
          ],
          duplicates: [],
        },
      });
    }
    if (path.endsWith("/thumbnail")) return route.fulfill({ status: 404, body: "" });
    return route.fulfill({ contentType: "image/png", body: PNG });
  });

  await page.goto(server.resolvedUrls!.local[0]);
  await page.locator(".cm-content").getByText("Intro paragraph.").waitFor();

  const insert = page.getByRole("button", { name: "Insert login.png into the body" });
  await insert.hover();
  await insert.click();

  // eslint-disable-next-line @typescript-eslint/no-explicit-any
  const text = () => page.evaluate(() => (window as any).view().state.doc.toString() as string);
  const snippet = "![The login dialog](/api/attachments/7)";
  await page.waitForFunction(
    // eslint-disable-next-line @typescript-eslint/no-explicit-any
    (s) => (window as any).view()?.state.doc.toString().includes(s),
    snippet,
  );
  const state = await page.evaluate(() => {
    // eslint-disable-next-line @typescript-eslint/no-explicit-any
    const view = (window as any).view();
    return {
      value: view.state.doc.toString() as string,
      caret: view.state.selection.main.head as number,
      focused: view.hasFocus as boolean,
    };
  });
  assert.equal(state.value, `Intro paragraph.\n\n${snippet}\n`, JSON.stringify(state));
  assert.equal(state.caret, state.value.length, JSON.stringify(state));
  assert.ok(state.focused, "the editor has focus");

  // A second insert while editing goes at the caret, joining the first image.
  await insert.hover();
  await insert.click();
  await page.waitForFunction(
    // eslint-disable-next-line @typescript-eslint/no-explicit-any
    (s) => (window as any).view().state.doc.toString().split(s).length === 3,
    snippet,
  );
  assert.equal(await text(), `Intro paragraph.\n\n${snippet}\n${snippet}\n`);

  await page.keyboard.press("Control+s");
  // eslint-disable-next-line @typescript-eslint/no-explicit-any
  await page.waitForFunction(() => (window as any).saved.length === 1);
  // eslint-disable-next-line @typescript-eslint/no-explicit-any
  assert.equal(await page.evaluate(() => (window as any).saved[0]), `Intro paragraph.\n\n${snippet}\n${snippet}\n`);
  await page.locator(".cm-md-image img[alt='The login dialog']").first().waitFor();
  assert.equal(await page.locator(".cm-md-image img").count(), 2);
  console.log("attachment insert: ok");

  // Details: edit the alt text, then reload and find it persisted.
  const details = page.getByRole("button", { name: "Details for login.png" });
  await details.hover();
  await details.click();
  const region = page.getByRole("region", { name: "Details for login.png" });
  const altInput = region.getByRole("textbox", { name: "Alt text for login.png" });
  assert.equal(await altInput.inputValue(), "The login dialog");
  await altInput.fill("Login dialog showing the lockout error");
  await region.getByRole("button", { name: "Save" }).click();
  // Saved: the field now matches what is stored, so Save turns itself off.
  await region.getByRole("button", { name: "Save" }).and(page.locator(":disabled")).waitFor();
  assert.deepEqual(patches, [{ alt_text: "Login dialog showing the lockout error" }]);

  const uses = region.locator(".att__detail-uses a");
  await uses.first().waitFor();
  assert.deepEqual(
    await uses.evaluateAll((links) => links.map((a) => [a.textContent, a.getAttribute("href")])),
    [
      ["APP-1", "#/APP/issues/APP-1"],
      ["OPS-9", "#/OPS/issues/OPS-9"],
    ],
  );

  if (process.env.SHOT) await page.screenshot({ path: process.env.SHOT, fullPage: true });
  await page.reload();
  await page.locator(".att__thumb img").waitFor();
  assert.equal(
    await page.locator(".att__thumb img").getAttribute("alt"),
    "Login dialog showing the lockout error",
  );
  assert.deepEqual(errors, []);
  console.log("attachment details: ok");
} finally {
  clearTimeout(deadline);
  await browser?.close();
  await server.close();
}
process.exit(0);

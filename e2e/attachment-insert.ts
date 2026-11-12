#!/usr/bin/env bun
// Insert from the Attachments panel: from read mode, Insert on an image opens
// the body editor with the image's Markdown (alt text included) at the end and
// the caret after it, and saving renders the image in the body.
import { strict as assert } from "node:assert";
import { resolve } from "node:path";
import { chromium } from "playwright";
import { createServer } from "../web/node_modules/vite/dist/node/index.js";

const root = resolve(import.meta.dir, "../web");
const fixtureId = root + "/src/AttachmentInsertFixture.svelte";
const fixture = `<script>
  import EditableMarkdown from './lib/EditableMarkdown.svelte';
  import AttachmentSection from './lib/AttachmentSection.svelte';
  let body = $state('Intro paragraph.');
  let editor = $state(null);
  let mode = $state('read');
  window.saved = [];
</script>
<main style="max-width: 700px">
  <p data-mode>{mode}</p>
  <EditableMarkdown bind:this={editor} bind:mode value={body} floatingToggle={false}
    attachTo={{ entity_type: 'page', entity_id: 1 }}
    onSave={async (next) => { window.saved.push(next); body = next; }} />
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
  console.error("Attachment insert test deadline exceeded");
  process.exit(1);
}, 90_000);

// A 1x1 PNG.
const PNG = Buffer.from(
  "iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAIAAACQd1PeAAAADElEQVR4nGP4z8AAAAMBAQDJ/pLvAAAAAElFTkSuQmCC",
  "base64",
);
const attachment = {
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
  await page.route(/\/api\/attachments\/7/, (route) =>
    new URL(route.request().url()).pathname.endsWith("/thumbnail")
      ? route.fulfill({ status: 404, body: "" })
      : route.fulfill({ contentType: "image/png", body: PNG }),
  );

  await page.goto(server.resolvedUrls!.local[0]);
  await page.getByText("Intro paragraph.").waitFor();
  assert.equal(await page.locator("[data-mode]").textContent(), "read");

  const insert = page.getByRole("button", { name: "Insert login.png into the body" });
  await insert.hover();
  await insert.click();

  const textarea = page.locator("textarea");
  await textarea.waitFor();
  const snippet = "![The login dialog](/api/attachments/7)";
  await page.waitForFunction(
    (s) => document.querySelector("textarea")?.value.includes(s),
    snippet,
  );
  const state = await textarea.evaluate((el: HTMLTextAreaElement) => ({
    value: el.value,
    caret: el.selectionStart,
    focused: document.activeElement === el,
  }));
  assert.equal(state.value, `Intro paragraph.\n\n${snippet}\n`, JSON.stringify(state));
  assert.equal(state.caret, state.value.length, JSON.stringify(state));
  assert.ok(state.focused, "the editor has focus");

  // A second insert while editing goes at the caret, joining the first image.
  await insert.hover();
  await insert.click();
  await page.waitForFunction(
    (s) => (document.querySelector("textarea")?.value.split(s).length ?? 0) === 3,
    snippet,
  );
  assert.equal(await textarea.inputValue(), `Intro paragraph.\n\n${snippet}\n${snippet}\n`);

  await textarea.press("Control+s");
  await page.waitForFunction(() => (window as any).saved.length === 1);
  await page.locator(".prose img[alt='The login dialog']").first().waitFor();
  assert.equal(await page.locator(".prose .md-image-row img").count(), 2);
  assert.deepEqual(errors, []);
  console.log("attachment insert: ok");
} finally {
  clearTimeout(deadline);
  await browser?.close();
  await server.close();
}
process.exit(0);

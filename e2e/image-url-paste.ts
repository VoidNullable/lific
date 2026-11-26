#!/usr/bin/env bun
// Browser interaction coverage; server fetch/storage guards have Rust tests.
import { strict as assert } from "node:assert";
import { resolve } from "node:path";
import { chromium } from "playwright";
import { createServer } from "../web/node_modules/vite/dist/node/index.js";
import { png } from "./png";

const root = resolve(import.meta.dir, "../web");
const fixtureId = root + "/src/ImageUrlPasteFixture.svelte";
const fixture = `<script>
  import { onDestroy } from 'svelte';
  import EditableMarkdown from './lib/EditableMarkdown.svelte';
  import AttachComposer from './lib/attachments/AttachComposer.svelte';
  import { createComposerAttachments } from './lib/attachments/composer.svelte';
  import { EditorView } from '@codemirror/view';
  let body = $state('Before.\\n\\n');
  let draft = $state('');
  let el = $state(null);
  window.view = () => EditorView.findFromDOM(document.querySelector('.cm-editor'));
  window.saved = [];
  const attach = createComposerAttachments({
    el: () => el,
    text: { read: () => draft, write: (next, caret) => {
      draft = next;
      if (el) { el.value = next; if (caret !== undefined) el.setSelectionRange(caret, caret); }
    }},
  });
  onDestroy(() => attach.destroy());
</script>
<main style="max-width: 700px">
  <EditableMarkdown value={body} attachTo={{entity_type:'page',entity_id:1}}
    onSave={async (next) => { window.saved.push(next); body = next; return {ok:true,body:next,seq:null}; }} />
  <AttachComposer composer={attach}>
    <textarea aria-label="Comment draft" bind:this={el} bind:value={draft}
      onpaste={(e) => attach.handlePaste(e)} onkeydown={(e) => attach.handleKeydown(e)}></textarea>
    <button disabled={attach.uploads.pending}>Submit comment</button>
  </AttachComposer>
</main>`;
const originalHtml = await Bun.file(root + "/index.html").text();
const html = originalHtml.replace(
  '<script type="module" src="/src/main.ts"></script>',
  `<script type="module">import {mount} from 'svelte'; import Fixture from '/src/ImageUrlPasteFixture.svelte'; import '/src/app.css'; mount(Fixture,{target:document.getElementById('app')});</script>`,
);
const server = await createServer({
  root, logLevel: "error",
  server: { host: "127.0.0.1", port: 0, strictPort: false },
  plugins: [{
    name: "image-url-paste-fixture", enforce: "pre",
    resolveId(id) { if (id === "/src/ImageUrlPasteFixture.svelte") return fixtureId; },
    load(id) { if (id === fixtureId) return fixture; },
    configureServer(vite) {
      vite.middlewares.use(async (req, res, next) => {
        if (req.url !== "/") return next();
        res.setHeader("Content-Type", "text/html");
        res.end(await vite.transformIndexHtml("/", html));
      });
    },
  }],
});
const deadline = setTimeout(() => { console.error("image URL paste deadline exceeded"); process.exit(1); }, 90_000);
let browser;
try {
  await server.listen();
  browser = await chromium.launch(process.env.PLAYWRIGHT_EXECUTABLE_PATH
    ? { headless: true, executablePath: process.env.PLAYWRIGHT_EXECUTABLE_PATH }
    : { headless: true, channel: "chromium" });
  const context = await browser.newContext({ permissions: ["clipboard-read", "clipboard-write"] });
  const page = await context.newPage();
  page.setDefaultTimeout(8000);
  page.setDefaultNavigationTimeout(30_000);
  const errors: string[] = [];
  page.on("pageerror", e => errors.push(e.message));
  const requests: Record<string, unknown>[] = [];
  let fail = false;
  let release: (() => void) | null = null;
  let delay = false;
  await page.route("**/api/**", async route => {
    const path = new URL(route.request().url()).pathname;
    if (path === "/api/attachments/import") {
      requests.push(route.request().postDataJSON());
      if (delay) await new Promise<void>(r => { release = r; });
      return route.fulfill(fail
        ? { status: 400, json: {error: "image host must resolve only to public addresses"} }
        : { json: {id: 7, url: "/api/attachments/7", filename: "screen.png", mime: "image/png", size: 80} });
    }
    if (path === "/api/attachments/7") return route.fulfill({ contentType: "image/png", body: png(2, 3) });
    return route.fulfill({ json: [] });
  });
  await page.goto(server.resolvedUrls!.local[0]);
  await page.locator(".cm-content").waitFor();
  const url = "https://images.example/screen.png?size=large";
  const text = () => page.evaluate(() => (window as any).view().state.doc.toString() as string);
  async function paste(value: string) {
    await page.evaluate(value => navigator.clipboard.writeText(value), value);
    await page.keyboard.press("Control+v");
  }
  await page.locator(".cm-line").first().click();
  await page.keyboard.press("Control+End");
  await paste(url);
  const accept = page.getByRole("button", {name: "Import as attachment", exact: true});
  await accept.waitFor();
  assert.equal(requests.length, 0, "pasting does not fetch");
  await page.getByRole("button", {name: "Keep URL", exact: true}).click();
  assert.equal(await text(), `Before.\n\n${url}`);
  assert.equal(requests.length, 0);
  console.log("image URL decline: ok");

  await page.locator(".cm-line").first().click();
  await page.keyboard.press("Control+End");
  await page.keyboard.press("Enter");
  await paste(url);
  delay = true;
  const importing = page.waitForRequest("**/api/attachments/import");
  await accept.click();
  await importing;
  // Changes after the pasted URL must survive a completed import.
  await page.locator(".cm-line").first().click();
  await page.keyboard.press("Control+End");
  await page.keyboard.type(" after");
  assert.ok(release);
  release!();
  delay = false;
  await page.waitForFunction(() => (window as any).view().state.doc.toString().includes("![screen.png](/api/attachments/7)"));
  assert.equal(await text(), `Before.\n\n${url}\n![screen.png](/api/attachments/7) after`);
  assert.deepEqual(requests[0], {url, entity_type: "page", entity_id: 1});
  await page.keyboard.press("Control+s");
  await page.waitForFunction(() => (window as any).saved.some((s: string) => s.includes("/api/attachments/7")));
  console.log("image URL accept, edits during fetch, saved Markdown: ok");

  const comment = page.getByRole("textbox", {name: "Comment draft"});
  await comment.fill("Comment ");
  await comment.press("End");
  await paste("http://127.0.0.1/private.png");
  fail = true;
  await accept.click();
  await page.getByText("image host must resolve only to public addresses", {exact: true}).waitFor();
  assert.equal(await comment.inputValue(), "Comment http://127.0.0.1/private.png");
  assert.deepEqual(requests[1], {url: "http://127.0.0.1/private.png"});
  fail = false;
  await page.getByRole("button", {name: "Retry import", exact: true}).click();
  await page.waitForFunction(() => (document.querySelector("textarea") as HTMLTextAreaElement).value.includes("/api/attachments/7"));
  assert.equal(await comment.inputValue(), "Comment ![screen.png](/api/attachments/7)");
  console.log("image URL failure retains URL and permits retry: ok");

  await comment.fill("");
  await paste(url);
  await comment.press("Escape");
  assert.equal(await accept.count(), 0);
  assert.equal(await comment.inputValue(), url);
  await comment.fill("");
  await paste(url);
  delay = true;
  const commentImport = page.waitForRequest("**/api/attachments/import");
  await comment.press("Enter");
  await commentImport;
  assert.ok(await page.getByRole("button", {name: "Submit comment"}).isDisabled());
  release!();
  delay = false;
  await page.waitForFunction(() => (document.querySelector("textarea") as HTMLTextAreaElement).value.includes("/api/attachments/7"));
  assert.equal(await comment.inputValue(), "![screen.png](/api/attachments/7)");
  assert.ok(await page.getByRole("button", {name: "Submit comment"}).isEnabled());
  console.log("image URL keyboard accept, decline and pending save gate: ok");
  assert.deepEqual(errors, []);
} finally {
  clearTimeout(deadline);
  await browser?.close();
  await server.close();
}
process.exit(0);

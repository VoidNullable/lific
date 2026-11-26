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
  import EditableMarkdown from './lib/EditableMarkdown.svelte';
  import Comments from './lib/Comments.svelte';
  import Toaster from './lib/toast/Toaster.svelte';
  import { EditorView } from '@codemirror/view';
  let body = $state('Before.\\n\\n');
  window.view = () => EditorView.findFromDOM(document.querySelector('.cm-editor'));
  window.saved = [];
  window.submitted = [];
</script>
<main style="max-width: 700px">
  <EditableMarkdown value={body} attachTo={{entity_type:'page',entity_id:1}}
    onSave={async (next) => { window.saved.push(next); body = next; return {ok:true,body:next,seq:null}; }} />
  <Comments comments={[]} placeholder="Comment draft"
    onSubmit={async (content) => { window.submitted.push(content); return {id: 1, content}; }} />
</main>
<Toaster />`;
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
        ? { status: 400, json: {error: "image host is unavailable"} }
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

  const comment = page.getByPlaceholder("Comment draft");
  await comment.fill("Comment ");
  await comment.press("End");
  await paste("http://127.0.0.1/private.png");
  fail = true;
  await accept.click();
  await page.getByText("image host is unavailable", {exact: true}).waitFor();
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
  assert.ok(await page.getByRole("button", {name: "Comment", exact: true}).isDisabled());
  release!();
  delay = false;
  await page.waitForFunction(() => (document.querySelector("textarea") as HTMLTextAreaElement).value.includes("/api/attachments/7"));
  assert.equal(await comment.inputValue(), "![screen.png](/api/attachments/7)");
  assert.ok(await page.getByRole("button", {name: "Comment", exact: true}).isEnabled());
  console.log("image URL keyboard accept, decline and pending save gate: ok");

  const importsBeforeEditing = requests.length;
  await comment.fill("");
  await paste(url);
  await accept.waitFor();
  await comment.pressSequentially(" note");
  await accept.waitFor({state: "hidden"});
  await comment.press("Enter");
  assert.equal(await comment.inputValue(), `${url} note\n`);
  await comment.press("Control+Enter");
  await page.waitForFunction(() => (window as any).submitted.length === 1);
  assert.equal(await page.evaluate(() => (window as any).submitted[0]), `${url} note`);
  assert.equal(requests.length, importsBeforeEditing);
  // Modified Enter also submits when the offer is still visible.
  await comment.fill("");
  await paste(url);
  await accept.waitFor();
  await comment.press("Control+Enter");
  await page.waitForFunction(() => (window as any).submitted.length === 2);
  assert.equal(await page.evaluate(() => (window as any).submitted[1]), url);
  assert.equal(requests.length, importsBeforeEditing);
  console.log("image URL offer dismisses on edit; newline and Ctrl+Enter submit: ok");

  await comment.fill("Before ");
  await comment.press("End");
  await paste(url);
  await accept.waitFor();
  await comment.press("Home");
  await accept.waitFor({state: "hidden"});
  assert.equal(requests.length, importsBeforeEditing);

  await page.locator(".cm-line").first().click();
  await page.keyboard.press("Control+End");
  await page.keyboard.press("Enter");
  await paste(url);
  await accept.waitFor();
  await page.keyboard.type(" note");
  await accept.waitFor({state: "hidden"});
  await page.keyboard.press("Enter");
  assert.ok((await text()).endsWith(`${url} note\n`));
  await paste(url);
  await accept.waitFor();
  await page.keyboard.press("Control+Home");
  await accept.waitFor({state: "hidden"});
  await page.keyboard.press("Escape");
  assert.equal(await page.evaluate(() => (window as any).view().hasFocus), false);
  assert.equal(requests.length, importsBeforeEditing);
  console.log("image URL caret movement dismisses offer; editor Escape still blurs: ok");

  await comment.fill("");
  await paste("https://images.example/picture.svg");
  assert.equal(await comment.inputValue(), "https://images.example/picture.svg");
  assert.equal(await accept.count(), 0);
  assert.equal(requests.length, importsBeforeEditing);
  console.log("SVG URL stays plain text without an import offer: ok");

  // A successful fetch must explain why a deleted URL was not replaced.
  await page.locator(".cm-line").first().click();
  await page.keyboard.press("Control+End");
  await page.keyboard.press("Enter");
  await paste("https://images.example/removed.png");
  delay = true;
  const removedImport = page.waitForRequest("**/api/attachments/import");
  await accept.click();
  await removedImport;
  await page.locator(".cm-line").first().click();
  await page.keyboard.press("Control+a");
  await page.keyboard.type("Replacement text.");
  release!();
  delay = false;
  await page.getByText("Image imported but not inserted. Find it in Attachments.", {exact: true}).waitFor();
  assert.equal(await text(), "Replacement text.");
  console.log("deleted URL during import preserves edits and shows attachment notice: ok");
  assert.deepEqual(errors, []);
} finally {
  clearTimeout(deadline);
  await browser?.close();
  await server.close();
}
process.exit(0);

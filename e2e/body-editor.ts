#!/usr/bin/env bun
// The live body editor (EditableMarkdown), mounted alone in Chromium:
//
//   - the body reads like the rendered page until the caret reaches an
//     element, and the stored Markdown is untouched by the decorations;
//   - typing saves on its own after a pause with the seq it last synced with,
//     and a body opened and saved without changes round-trips byte-for-byte;
//   - a remote edit elsewhere in the body applies in place (caret kept), and
//     one that overlaps unsaved typing raises the conflict notice;
//   - images render in place and reveal their Markdown under the caret;
//   - task checkboxes toggle the marker; the slash menu inserts blocks;
//   - the selection toolbar formats text;
//   - a phone-width viewport does not overflow.
//
// Run:  cd e2e && bun body-editor.ts   (SHOTS=dir saves screenshots)
import { strict as assert } from "node:assert";
import { resolve } from "node:path";
import { chromium, type Page } from "playwright";
import { png } from "./png";
import { createServer } from "../web/node_modules/vite/dist/node/index.js";

const root = resolve(import.meta.dir, "../web");
const fixtureId = root + "/src/BodyEditorFixture.svelte";
const SHOTS = process.env.SHOTS;

const BODY = [
  "# Release checklist",
  "",
  "Ship **2.12** once the *editor* lands. See LIF-12 and [the docs](https://lific.dev/docs).",
  "",
  "- [ ] Write the changelog",
  "- [x] Run `cargo test`",
  "- Plain bullet",
  "",
  "> Quotes stay quotes.",
  "",
  "![screenshot|240](/api/attachments/1)",
  "",
  "| Area | Owner |",
  "| --- | --- |",
  "| Web | Blake |",
  "",
  "```rust",
  "fn main() {}",
  "```",
  "",
  "---",
  "",
  "Last line.",
].join("\n");

const fixture = `<script>
  import EditableMarkdown from './lib/EditableMarkdown.svelte';
  import { EditorView } from '@codemirror/view';
  window.__view = () => EditorView.findFromDOM(document.querySelector('.cm-editor'));
  let value = $state(window.__initial);
  let seq = $state(1);
  let ref = $state(null);
  window.__saves = [];
  window.__pending = [];
  window.__remote = (text, s) => { value = text; seq = s; };
  window.__ref = () => ref;
  function onSave(next, expected) {
    window.__saves.push({ next, expected });
    if (window.__manual) return new Promise((r) => window.__pending.push(r));
    seq += 1;
    value = next;
    return Promise.resolve({ ok: true, body: next, seq });
  }
</script>
<main style="max-width: 700px; margin: 40px auto; padding: 0 16px;">
  <EditableMarkdown bind:this={ref} {value} {seq} docId={7} noun="The description" {onSave} />
  <p id="after">After the body</p>
</main>`;

const originalHtml = await Bun.file(root + "/index.html").text();
const html = originalHtml.replace(
  '<script type="module" src="/src/main.ts"></script>',
  `<script type="module">window.__initial = ${JSON.stringify(BODY)}; import {mount} from 'svelte'; import Fixture from '/src/BodyEditorFixture.svelte'; import '/src/app.css'; mount(Fixture,{target:document.getElementById('app')});</script>`,
);
const server = await createServer({
  root,
  logLevel: "error",
  server: { host: "127.0.0.1", port: 0, strictPort: false },
  plugins: [
    {
      name: "body-editor-fixture",
      enforce: "pre",
      resolveId(id) {
        if (id === "/src/BodyEditorFixture.svelte") return fixtureId;
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
  console.error("Body editor test deadline exceeded");
  process.exit(1);
}, 120_000);

const doc = (page: Page) =>
  page.evaluate(() => {
    const view = (window as unknown as { __view(): { state: { doc: { toString(): string } } } }).__view();
    return view.state.doc.toString();
  });
const saves = (page: Page) =>
  page.evaluate(() => (window as unknown as { __saves: { next: string; expected: number }[] }).__saves);
const shot = async (page: Page, name: string) => {
  if (SHOTS) await page.screenshot({ path: `${SHOTS}/${name}.png`, fullPage: true });
};
/** Put the caret at `offset` in the document. */
const caretAt = (page: Page, offset: number) =>
  page.evaluate((pos) => {
    const view = (window as unknown as { __view(): { dispatch(t: unknown): void; focus(): void } }).__view();
    view.focus();
    view.dispatch({ selection: { anchor: pos } });
  }, offset);

let browser;
try {
  await server.listen();
  const executablePath = process.env.PLAYWRIGHT_EXECUTABLE_PATH;
  browser = await chromium.launch(
    executablePath ? { headless: true, executablePath } : { headless: true, channel: "chromium" },
  );
  const page = await browser.newPage({ viewport: { width: 1000, height: 1400 } });
  page.setDefaultTimeout(8_000);
  const errors: string[] = [];
  page.on("pageerror", (error) => errors.push(error.message));
  await page.route("**/api/**", (route) => route.fulfill({ json: [] }));
  await page.route(/\/api\/.*attachments\/\d+/, (route) => {
    const thumb = new URL(route.request().url()).pathname.endsWith("/thumbnail");
    return route.fulfill({ contentType: "image/png", body: thumb ? png(480, 270) : png(1600, 900) });
  });

  await page.goto(server.resolvedUrls!.local[0]);
  await page.locator(".cm-content").waitFor();

  // ── reads like the page ────────────────────────────────
  const view = await page.evaluate(() => {
    const content = document.querySelector(".cm-content")!;
    return {
      text: content.textContent,
      h1: !!content.querySelector(".cm-md-h1"),
      strong: content.querySelector(".cm-md-strong")?.textContent,
      checkboxes: content.querySelectorAll("input.cm-md-task").length,
      image: !!content.querySelector(".cm-md-image img"),
      table: !!content.querySelector(".cm-md-block table"),
      ref: content.querySelector(".cm-md-ref")?.textContent,
    };
  });
  await shot(page, "1-reading");
  assert.ok(!view.text?.includes("**"), "bold markers are hidden away from the caret");
  assert.ok(!view.text?.includes("# Release"), "the heading marker is hidden");
  assert.ok(view.h1, "the heading is styled");
  assert.equal(view.strong, "2.12");
  assert.equal(view.checkboxes, 2);
  assert.ok(view.image, "the image renders");
  assert.ok(view.table, "the table renders");
  assert.equal(view.ref, "LIF-12");
  assert.equal(await doc(page), BODY, "decorations never touch the text");

  // ── caret reveals syntax ───────────────────────────────
  await caretAt(page, BODY.indexOf("2.12") + 1);
  const revealed = await page.locator(".cm-line").nth(2).textContent();
  assert.ok(revealed?.includes("**2.12**"), `bold markers show under the caret: ${revealed}`);
  await caretAt(page, BODY.indexOf("![screenshot") + 3);
  assert.ok(
    (await page.locator(".cm-content").textContent())?.includes("![screenshot|240](/api/attachments/1)"),
    "the image source shows under the caret",
  );
  assert.equal(await page.locator(".cm-md-image img").count(), 1, "and the image still shows");
  await shot(page, "2-caret-in-image");

  // ── autosave ───────────────────────────────────────────
  await caretAt(page, BODY.length);
  await page.keyboard.type(" Typed.");
  assert.equal((await saves(page)).length, 0, "nothing saves mid-typing");
  await page.waitForFunction(() => (window as unknown as { __saves: unknown[] }).__saves.length === 1, null, {
    timeout: 5_000,
  });
  const first = (await saves(page))[0];
  assert.equal(first.next, BODY + " Typed.");
  assert.equal(first.expected, 1);

  // ── a task checkbox toggles its marker ─────────────────
  await page.locator(".cm-content").blur();
  await page.locator("input.cm-md-task").first().click();
  assert.ok((await doc(page)).includes("- [x] Write the changelog"));

  // ── remote edit applies in place ───────────────────────
  await caretAt(page, BODY.indexOf("Quotes") + 2);
  await page.evaluate(() => (window as unknown as { __ref(): { flush(): Promise<void> } }).__ref().flush());
  const before = await doc(page);
  const remote = before.replace("Last line.", "Last line, edited by an agent.");
  await page.evaluate((t) => (window as unknown as { __remote(t: string, s: number): void }).__remote(t, 99), remote);
  await page.waitForFunction((t) => {
    const v = (window as unknown as { __view(): { state: { doc: { toString(): string } } } }).__view();
    return v.state.doc.toString() === t;
  }, remote);
  const caret = await page.evaluate(() => {
    const v = (window as unknown as { __view(): { state: { selection: { main: { head: number } } } } }).__view();
    return v.state.selection.main.head;
  });
  assert.equal(caret, remote.indexOf("Quotes") + 2, "the caret stays put");
  await page.keyboard.type("zz");
  await page.keyboard.press("ControlOrMeta+z");
  assert.equal(await doc(page), remote, "undo takes back the person's typing");
  // The step before that in the person's history is the task tick: the
  // remote edit in between is not theirs to undo.
  await page.keyboard.press("ControlOrMeta+z");
  assert.equal(
    await doc(page),
    remote.replace("- [x] Write the changelog", "- [ ] Write the changelog"),
    "undo skips the remote edit",
  );
  // A real keyboard reports Shift+Z as "Z"; a lowercase "z" would read as undo.
  await page.keyboard.press("ControlOrMeta+Shift+Z");
  assert.equal(await doc(page), remote, "redo brings the tick back");

  // ── overlapping edit raises a conflict ─────────────────
  await page.evaluate(() => ((window as unknown as { __manual: boolean }).__manual = true));
  await caretAt(page, remote.indexOf("Quotes stay") + "Quotes stay".length);
  await page.keyboard.type(" mostly");
  await page.evaluate(
    (t) => (window as unknown as { __remote(t: string, s: number): void }).__remote(t, 120),
    remote.replace("Quotes stay quotes.", "Quotes are gone."),
  );
  await page.locator(".em-conflict").waitFor();
  await shot(page, "3-conflict");
  await page.getByRole("button", { name: "Keep mine" }).click();
  await page.waitForFunction(() => (window as unknown as { __pending: unknown[] }).__pending.length === 1);
  const kept = (await saves(page)).at(-1)!;
  assert.ok(kept.next.includes("Quotes stay mostly quotes."));
  assert.equal(kept.expected, 120);
  await page.evaluate(() => {
    const w = window as unknown as { __pending: ((r: unknown) => void)[]; __manual: boolean; __saves: { next: string }[] };
    w.__manual = false;
    w.__pending[0]({ ok: true, body: w.__saves.at(-1)!.next, seq: 121 });
  });
  await page.locator(".em-conflict").waitFor({ state: "detached" });

  // ── slash menu ─────────────────────────────────────────
  const end = (await doc(page)).length;
  await caretAt(page, end);
  await page.keyboard.press("Enter");
  await page.keyboard.press("Enter");
  await page.keyboard.type("/check");
  await page.locator(".cm-tooltip-autocomplete").waitFor();
  // CodeMirror ignores Enter for the first 75 ms a completion list is open.
  await page.waitForTimeout(120);
  await shot(page, "4-slash");
  await page.keyboard.press("Enter");
  await page.keyboard.type("New task");
  assert.ok((await doc(page)).endsWith("\n\n- [ ] New task"), JSON.stringify((await doc(page)).slice(-40)));

  // ── selection toolbar ──────────────────────────────────
  await page.keyboard.down("Shift");
  for (let i = 0; i < 4; i++) await page.keyboard.press("ArrowLeft");
  await page.keyboard.up("Shift");
  await page.locator(".fmt").waitFor();
  await shot(page, "5-toolbar");
  await page.getByRole("button", { name: "Bold" }).click();
  assert.ok((await doc(page)).endsWith("- [ ] New **task**"));

  // ── phone width ────────────────────────────────────────
  await page.setViewportSize({ width: 390, height: 1400 });
  await page.locator(".cm-content").blur();
  await shot(page, "6-phone");
  assert.equal(await page.evaluate(() => document.documentElement.scrollWidth), 390);

  assert.deepEqual(errors, []);
  console.log("body editor: ok");
} finally {
  clearTimeout(deadline);
  await browser?.close();
  await server.close();
}
process.exit(0);

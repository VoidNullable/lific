#!/usr/bin/env bun
/**
 * LIF-524: a project's Conventions tab in the web UI.
 *
 * Spawns the debug binary against a scratch config + DB, signs in, and at a
 * desktop viewport (1440x900) and a phone viewport (390x844) checks that:
 *
 *   - the sidebar (desktop) or the mobile nav (phone) lists Conventions;
 *   - a project without a conventions page shows the template choice on
 *     /<PROJECT>/conventions: four templates, Blank among them, each one's
 *     full text readable before choosing;
 *   - "Use <template>" creates the page and the tab becomes that page, with
 *     its state in briefings shown and no switch to flip;
 *   - the Pages list shows neither a conventions block nor the page, and the
 *     page's old address and a palette lookup both lead to the tab;
 *   - Change template asks first, then replaces the text, and the old text
 *     is in the page history;
 *   - turning it off and back on each ask first, survive a reload, and keep
 *     the text;
 *   - a project whose conventions page is empty offers the templates and
 *     fills in that same page;
 *   - the create-project picker shares the component and sends the choice.
 *
 * Also checks that a viewer, and a signed-out reader of a published
 * project, get the tab read-only.
 *
 * Run:  cd e2e && bun conventions.ts
 * Binary: $LIFIC_BIN, else target/debug/lific (debug rust-embed reads
 * web/dist from disk, so build the web UI first).
 * Screenshots: $SHOTS, else /tmp/opencode/conventions-tab-shots.
 */
import { strict as assert } from "node:assert";
import { chromium, type Browser, type BrowserContext, type Page } from "playwright";
import { spawn, execFileSync, type ChildProcess } from "node:child_process";
import { mkdtempSync, mkdirSync, rmSync, existsSync } from "node:fs";
import { createServer } from "node:net";
import { tmpdir } from "node:os";
import { join, resolve } from "node:path";
import { Database } from "bun:sqlite";

const ROOT = resolve(import.meta.dir, "..");
const BIN = process.env.LIFIC_BIN ?? join(ROOT, "target", "debug", "lific");
const SHOTS = process.env.SHOTS ?? "/tmp/opencode/conventions-tab-shots/v2";
const ADMIN_PASSWORD = "conventions-admin-password-123";
const PASSWORD = "conventions-user-password-123";

const VIEWPORTS = [
  { name: "desktop", width: 1440, height: 900, mobile: false },
  { name: "phone", width: 390, height: 844, mobile: true },
] as const;
type Viewport = (typeof VIEWPORTS)[number];

interface Template {
  key: string;
  name: string;
  summary: string;
  content: string;
}

function cli(config: string, db: string, args: string[]): string {
  return execFileSync(BIN, ["--config", config, "--db", db, ...args], {
    encoding: "utf8",
    stdio: ["ignore", "pipe", "pipe"],
  });
}

function freePort(): Promise<number> {
  return new Promise((res, rej) => {
    const srv = createServer();
    srv.listen(0, "127.0.0.1", () => {
      const addr = srv.address();
      if (addr && typeof addr === "object") {
        const port = addr.port;
        srv.close(() => res(port));
      } else srv.close(() => rej(new Error("could not allocate a port")));
    });
  });
}

async function waitForServer(url: string, timeoutMs: number): Promise<void> {
  const deadline = Date.now() + timeoutMs;
  while (Date.now() < deadline) {
    try {
      if ((await fetch(url)).ok) return;
    } catch {
      // not up yet
    }
    await new Promise((r) => setTimeout(r, 150));
  }
  throw new Error(`server did not answer at ${url} within ${timeoutMs}ms`);
}

async function api(page: Page, method: string, path: string, body?: unknown): Promise<any> {
  return page.evaluate(
    async ([method, path, body]) => {
      const res = await fetch(`/api${path}`, {
        method,
        headers: {
          "Content-Type": "application/json",
        },
        body: body === undefined ? undefined : JSON.stringify(body),
      });
      if (!res.ok) throw new Error(`${method} ${path}: ${res.status} ${await res.text()}`);
      return res.json();
    },
    [method, path, body] as const,
  );
}

async function signIn(context: BrowserContext, base: string, identity: string, password: string) {
  const page = await context.newPage();
  await page.goto(`${base}/login`, { waitUntil: "load" });
  await page.fill("#login-identity", identity);
  await page.fill("#login-password", password);
  await page.click("button[type=submit]");
  // The app leaves the login route once the session cookie is set.
  await page.waitForURL((url) => !url.href.includes("login"), { timeout: 15_000 });
  return page;
}

async function newContext(browser: Browser, vp: Viewport) {
  return browser.newContext({
    viewport: { width: vp.width, height: vp.height },
    ...(vp.mobile ? { isMobile: true, hasTouch: true, deviceScaleFactor: 2 } : {}),
  });
}

async function shot(page: Page, vp: Viewport, name: string) {
  // Let route fades and colour transitions finish.
  await page.waitForTimeout(400);
  await page.screenshot({ path: join(SHOTS, `${vp.name}-${name}.png`) });
}

async function projectByIdent(page: Page, ident: string): Promise<{ id: number; identifier: string }> {
  const projects = await api(page, "GET", "/projects");
  const found = projects.find((p: { identifier: string }) => p.identifier === ident);
  assert.ok(found, `project ${ident} exists`);
  return found;
}

/** The project's live conventions page, or null. */
async function conventionsPage(page: Page, projectId: number): Promise<any | null> {
  const pages = await api(page, "GET", `/pages?project_id=${projectId}`);
  return pages.find((p: { conventions?: boolean }) => p.conventions === true) ?? null;
}

/** The page's width must not overflow the viewport (no sideways scroll). */
async function assertNoOverflow(page: Page, what: string) {
  const over = await page.evaluate(() => document.documentElement.scrollWidth - window.innerWidth);
  assert.ok(over <= 1, `${what}: the page scrolls sideways by ${over}px`);
}

async function waitFor<T>(page: Page, read: () => Promise<T>, ok: (v: T) => boolean, what: string): Promise<T> {
  const deadline = Date.now() + 8_000;
  let last = await read();
  while (!ok(last) && Date.now() < deadline) {
    await page.waitForTimeout(100);
    last = await read();
  }
  assert.ok(ok(last), `${what} (last: ${JSON.stringify(last)})`);
  return last;
}

/** A distinctive line of a template's text, to find it in a preview. */
function sampleLine(t: Template): string {
  const line = t.content
    .split("\n")
    .map((l) => l.replace(/^[#>*\-\d.\s]+/, "").replace(/[*_`]/g, "").trim())
    .find((l) => l.length > 30);
  assert.ok(line, `${t.key} has a line to look for`);
  return line.slice(0, 30);
}

/** Undo a stored page diff (unified, no file header) on the text it
 *  produced, giving back the text before the edit. */
function undoDiff(after: string, diff: string): string {
  const newLines = after.split("\n");
  const old: string[] = [];
  let next = 0; // index into newLines
  for (const line of diff.split("\n")) {
    const hunk = line.match(/^@@ -\d+(?:,\d+)? \+(\d+)(?:,\d+)? @@/);
    if (hunk) {
      // An empty side is numbered one line earlier in unified diffs.
      const start = Math.max(0, Number(hunk[1]) - 1);
      while (next < start) old.push(newLines[next++]);
      continue;
    }
    if (line.startsWith("\\") || line === "") continue;
    if (line.startsWith(" ")) {
      old.push(line.slice(1));
      next++;
    } else if (line.startsWith("-")) {
      old.push(line.slice(1));
    } else if (line.startsWith("+")) {
      next++;
    }
  }
  while (next < newLines.length) old.push(newLines[next++]);
  return old.join("\n");
}

/** Open the document's ⋯ menu and pick an item. */
async function menu(page: Page, item: string) {
  await page.getByRole("button", { name: "More actions" }).first().click();
  await page.getByRole("menuitem", { name: item, exact: true }).click();
}

async function scenario(
  browser: Browser,
  base: string,
  vp: Viewport,
  ids: { bare: string; emptyPage: string; created: string },
  templates: Template[],
  errors: string[],
) {
  const context = await newContext(browser, vp);
  const page = await signIn(context, base, "ada", ADMIN_PASSWORD);
  page.setDefaultTimeout(10_000);
  page.on("pageerror", (err) => errors.push(`${vp.name}: ${err}`));
  const byKey = (key: string) => templates.find((t) => t.key === key)!;
  const tab = `${base}/#/${ids.bare}/conventions`;

  // ── A project with no conventions page ──────────────────────────────
  const bare = await projectByIdent(page, ids.bare);
  const existing = await conventionsPage(page, bare.id);
  if (existing) await api(page, "DELETE", `/pages/${existing.id}`);
  await api(page, "POST", "/pages", {
    project_id: bare.id,
    title: "Release checklist",
    content: "Steps before a release",
    status: "active",
  });

  // The tab is in the navigation.
  await page.goto(`${base}/#/${ids.bare}/pages`);
  if (vp.mobile) {
    await page.getByRole("button", { name: "Open navigation" }).click();
    await page.getByRole("button", { name: `Open Project ${ids.bare} navigation` }).click();
    const link = page.locator(`[data-mobile-nav] a[href="#/${ids.bare}/conventions"]`);
    await link.waitFor({ state: "visible" });
    await link.click();
  } else {
    const link = page.locator(`a.sidebar-destination[href="#/${ids.bare}/conventions"]`);
    await link.waitFor({ state: "visible" });
    await link.click();
  }
  await page.waitForURL(new RegExp(`#/${ids.bare}/conventions$`));
  console.log(`ok   ${vp.name}: the navigation lists Conventions`);

  const chooser = page.getByTestId("conventions-chooser");
  await chooser.waitFor({ state: "visible", timeout: 15_000 });
  assert.equal(await chooser.getAttribute("data-state"), "none", "no page: the template choice shows");
  const radios = chooser.getByRole("radiogroup", { name: "Conventions template" });
  for (const t of templates) {
    const text = await radios.locator(`[role=radio][data-template="${t.key}"]`).innerText();
    assert.ok(text.includes(t.key === "empty" ? "Blank" : t.name), `${t.key} is named`);
  }
  const preview = chooser.getByTestId("template-preview");
  const first = byKey("ai_managed");
  await waitFor(page, () => preview.getAttribute("data-template"), (v) => v === "ai_managed", "the first template is previewed");
  assert.ok((await preview.innerText()).includes(sampleLine(first)), "the preview shows the template's text");
  await radios.locator('[role=radio][data-template="human_managed"]').click();
  await waitFor(page, () => preview.getAttribute("data-template"), (v) => v === "human_managed", "picking previews it");
  assert.ok((await preview.innerText()).includes(sampleLine(byKey("human_managed"))), "the preview follows the choice");
  await radios.locator('[role=radio][data-template="empty"]').click();
  await preview.getByTestId("template-blank").waitFor({ state: "visible" });
  assert.ok((await preview.innerText()).includes("An empty page"), "Blank says what it is");
  const use = chooser.getByTestId("conventions-use-template");
  assert.equal((await use.innerText()).trim(), "Start blank", "the action names the blank start");
  await assertNoOverflow(page, `${vp.name} conventions, Blank`);
  await shot(page, vp, "01-tab-blank");

  await radios.locator('[role=radio][data-template="human_managed"]').click();
  assert.equal((await use.innerText()).trim(), "Use Human managed", "one action names the choice");
  // The longest template, so there is a middle to scroll to.
  const longest = templates.reduce((a, b) => (b.content.length > a.content.length ? b : a));
  await radios.locator(`[role=radio][data-template="${longest.key}"]`).click();
  await waitFor(page, () => preview.getAttribute("data-template"), (v) => v === longest.key, "the longest is previewed");
  // The text is laid out at full length (no box of its own to scroll), and
  // its header with the action stays in view as the page scrolls.
  const text = preview.getByTestId("template-text");
  assert.equal(
    await text.evaluate((el) => el.scrollHeight - el.clientHeight),
    0,
    "the text is not cut off in a scroll box",
  );
  await chooser.evaluate((el) => {
    const scroller = el.parentElement!;
    scroller.scrollTop = (scroller.scrollHeight - scroller.clientHeight) / 2;
  });
  await page.waitForTimeout(200);
  const scrolled = await chooser.evaluate((el) => el.parentElement!.scrollTop);
  assert.ok(scrolled > 0, "the page scrolls the text");
  const useBox = await use.boundingBox();
  assert.ok(useBox && useBox.y >= 0 && useBox.y < vp.height, "the action stays in view mid-text");
  await assertNoOverflow(page, `${vp.name} conventions, no page`);
  await shot(page, vp, "02-tab-template-mid-text");
  await chooser.evaluate((el) => (el.parentElement!.scrollTop = 0));
  await radios.locator('[role=radio][data-template="human_managed"]').click();
  console.log(`ok   ${vp.name}: without a page the tab offers four templates with their full text`);

  // ── Use it: the tab becomes the page ────────────────────────────────
  await use.click();
  const status = page.getByTestId("conventions-status");
  await status.waitFor({ state: "visible", timeout: 15_000 });
  assert.ok(page.url().endsWith(`#/${ids.bare}/conventions`), "the tab keeps its address");
  assert.equal(await status.getAttribute("data-enabled"), "true", "a new page is in briefings");
  assert.ok((await status.innerText()).includes("In agents' briefings"), "the state is stated");
  assert.equal(await page.getByRole("switch").count(), 0, "there is no switch to flip");
  const created = await waitFor(page, () => conventionsPage(page, bare.id), (p) => p !== null, "the page exists");
  assert.equal(created.content, byKey("human_managed").content, "the page holds the template");
  assert.equal(created.status, "active", "the page is created active");
  await page.getByText(sampleLine(byKey("human_managed"))).first().waitFor({ state: "visible" });
  await assertNoOverflow(page, `${vp.name} conventions page`);
  await shot(page, vp, "03-tab-with-page");
  console.log(`ok   ${vp.name}: a template creates the page and the tab shows it`);

  // ── Pages leaves it out; old links lead to the tab ──────────────────
  await page.goto(`${base}/#/${ids.bare}/pages`);
  await page.getByText("Release checklist").filter({ visible: true }).first().waitFor({ state: "visible" });
  assert.equal(await page.getByTestId("conventions-section").count(), 0, "no conventions block on Pages");
  const copies = await page.evaluate((ident) => {
    const main = document.querySelector("main") ?? document.body;
    return [...main.querySelectorAll("button, a")].filter((el) => el.textContent?.includes(ident)).length;
  }, created.identifier);
  assert.equal(copies, 0, "the conventions page is not listed in Pages");
  await assertNoOverflow(page, `${vp.name} pages`);
  await shot(page, vp, "04-pages-list");

  await page.goto(`${base}/#/${ids.bare}/pages/${created.id}`);
  await page.waitForURL(new RegExp(`#/${ids.bare}/conventions$`), { timeout: 15_000 });
  await status.waitFor({ state: "visible" });

  // A link to one of its comments, in either form, keeps the target.
  const comment = await api(page, "POST", `/pages/${created.id}/comments`, { content: `Linked comment ${vp.name}` });
  for (const link of [
    `${base}/#/${ids.bare}/pages/${created.id}?comment=${comment.id}`,
    `${base}/${ids.bare}/pages/${created.id}#comment-${comment.id}`,
  ]) {
    await page.goto(link);
    await page.waitForURL(`${base}/#/${ids.bare}/conventions?comment=${comment.id}`, { timeout: 15_000 });
    await page.locator(`#comment-${comment.id}`).waitFor({ state: "visible", timeout: 15_000 });
    const inView = await page.locator(`#comment-${comment.id}`).evaluate((el) => {
      const r = el.getBoundingClientRect();
      return r.bottom > 0 && r.top < window.innerHeight;
    });
    assert.ok(inView, `${link}: the linked comment is scrolled into view`);
  }
  if (!vp.mobile) {
    await page.goto(`${base}/#/${ids.bare}/pages`);
    await page.getByText("Release checklist").filter({ visible: true }).first().waitFor();
    await page.keyboard.press("Control+k");
    const input = page.getByPlaceholder("Search or jump to…");
    await input.waitFor({ state: "visible" });
    await input.fill(created.identifier);
    await page.locator("[data-flat-idx]").filter({ hasText: created.identifier }).first().waitFor({ state: "visible" });
    await page.keyboard.press("Enter");
    await page.waitForURL(new RegExp(`#/${ids.bare}/conventions$`), { timeout: 15_000 });
  }
  console.log(`ok   ${vp.name}: Pages leaves the page out, and its old address${vp.mobile ? "" : " and a palette lookup"} open the tab`);

  // ── Change template, behind a confirmation ──────────────────────────
  await page.goto(tab);
  await status.waitFor({ state: "visible", timeout: 15_000 });
  await menu(page, "Change template…");
  const dialog = page.getByTestId("conventions-dialog");
  await dialog.waitFor({ state: "visible" });
  assert.equal(await dialog.getAttribute("data-mode"), "template");
  assert.ok((await dialog.innerText()).includes("stays in the page's history"), "the dialog says the old text is kept");
  await dialog.locator('[role=radio][data-template="human_owned"]').click();
  await waitFor(page, () => dialog.getByTestId("template-preview").getAttribute("data-template"), (v) => v === "human_owned", "the dialog previews the choice");
  const confirm = dialog.getByTestId("conventions-dialog-confirm");
  assert.equal((await confirm.innerText()).trim(), "Replace with Human owned");
  await assertNoOverflow(page, `${vp.name} change template`);
  await shot(page, vp, "05-change-template-dialog");
  // Focus stays in the dialog, and nothing behind it reacts to keys.
  for (let i = 0; i < 30; i++) {
    await page.keyboard.press("Tab");
    const inside = await dialog.evaluate((el) => el.contains(document.activeElement));
    assert.ok(inside, `Tab ${i + 1} kept focus in the dialog`);
  }
  const backgroundTookFocus = await page.evaluate(() => {
    const btn = document.querySelector<HTMLElement>('button[aria-label="More actions"]')!;
    const before = document.activeElement;
    btn.focus();
    const took = document.activeElement === btn;
    (before as HTMLElement | null)?.focus();
    return took;
  });
  assert.equal(backgroundTookFocus, false, "the page behind the dialog can't take focus");
  // Esc closes it and focus goes back to the menu button.
  await page.keyboard.press("Escape");
  await dialog.waitFor({ state: "hidden" });
  assert.ok(page.url().endsWith(`#/${ids.bare}/conventions`), "Esc closed the dialog, not the page");
  // The dialog's close event runs a task after it hides.
  await waitFor(
    page,
    () => page.evaluate(() => document.activeElement?.getAttribute("aria-label") ?? document.activeElement?.tagName),
    (v) => v === "More actions",
    "focus returns to the menu button",
  );
  // Cancel changes nothing.
  await menu(page, "Change template…");
  await dialog.waitFor({ state: "visible" });
  await dialog.getByRole("button", { name: "Cancel" }).click();
  await dialog.waitFor({ state: "hidden" });
  assert.equal((await api(page, "GET", `/pages/${created.id}`)).content, created.content, "cancel keeps the text");
  await menu(page, "Change template…");
  await dialog.locator('[role=radio][data-template="human_owned"]').click();
  await confirm.click();
  await dialog.waitFor({ state: "hidden" });
  await waitFor(
    page,
    async () => (await api(page, "GET", `/pages/${created.id}`)).content,
    (c) => c === byKey("human_owned").content,
    "the text is the new template",
  );
  // The history entry holds the old text: undoing its diff on the new text
  // gives back exactly what the page said before.
  const history = (await api(page, "GET", `/pages/${created.id}/activity`)).items;
  const entry = history.find((a: { field: string | null }) => a.field === "content_diff" || a.field === "content");
  assert.ok(entry, "the replacement is in the page history");
  const previous =
    entry.field === "content" ? entry.old_value : undoDiff(byKey("human_owned").content, entry.new_value);
  assert.equal(previous.trimEnd(), created.content.trimEnd(), "the history entry holds the old text");
  console.log(`ok   ${vp.name}: Change template asks first, then replaces the text, with history`);

  // ── Turn off, then back on ──────────────────────────────────────────
  await menu(page, "Leave out of briefings…");
  await dialog.waitFor({ state: "visible" });
  assert.equal(await dialog.getAttribute("data-mode"), "off");
  assert.ok((await dialog.innerText()).includes("stay here"), "turning off says the page stays");
  await confirm.click();
  await dialog.waitFor({ state: "hidden" });
  await waitFor(
    page,
    async () => (await api(page, "GET", `/pages/${created.id}`)).conventions_enabled,
    (v) => v === false,
    "conventions_enabled=false is stored",
  );
  await page.reload();
  await status.waitFor({ state: "visible", timeout: 15_000 });
  assert.equal(await status.getAttribute("data-enabled"), "false", "off survives a reload");
  assert.ok((await status.innerText()).includes("Left out of agents' briefings"));
  assert.equal((await api(page, "GET", `/pages/${created.id}`)).content, byKey("human_owned").content, "off keeps the text");
  await shot(page, vp, "06-tab-off");
  await menu(page, "Put back in briefings…");
  await dialog.waitFor({ state: "visible" });
  assert.equal(await dialog.getAttribute("data-mode"), "on");
  await confirm.click();
  await dialog.waitFor({ state: "hidden" });
  await waitFor(page, () => status.getAttribute("data-enabled"), (v) => v === "true", "back in briefings");
  assert.equal((await api(page, "GET", `/pages/${created.id}`)).conventions_enabled, true);
  console.log(`ok   ${vp.name}: off and on each ask first, survive a reload and keep the text`);

  // ── A conventions page with no text yet ─────────────────────────────
  const emptyProject = await projectByIdent(page, ids.emptyPage);
  const emptyPage = await conventionsPage(page, emptyProject.id);
  assert.ok(emptyPage, "an API-created project has a conventions page");
  assert.equal(emptyPage.content.trim(), "", "and the empty template leaves it blank");
  await page.goto(`${base}/#/${ids.emptyPage}/conventions`);
  await chooser.waitFor({ state: "visible", timeout: 15_000 });
  assert.equal(await chooser.getAttribute("data-state"), "empty", "an empty page offers the templates");
  await chooser.getByTestId("conventions-use-template").click();
  await status.waitFor({ state: "visible", timeout: 15_000 });
  const filled = await api(page, "GET", `/pages/${emptyPage.id}`);
  assert.equal(filled.content, byKey("ai_managed").content, "the template filled in the same page");
  assert.equal(await conventionsPage(page, emptyProject.id).then((p) => p.id), emptyPage.id, "no second page");
  console.log(`ok   ${vp.name}: an empty conventions page takes a template in place`);

  // ── The create-project picker ───────────────────────────────────────
  await page.goto(`${base}/#/projects/new`);
  await page.locator("#project-name").waitFor({ state: "visible", timeout: 15_000 });
  const picker = page.getByRole("radiogroup", { name: "Conventions template" });
  await picker.locator('[role=radio][data-template="human_owned"]').waitFor({ state: "visible" });
  assert.equal(await picker.locator('[role=radio][data-template="empty"]').getAttribute("aria-checked"), "true", "Blank is the default");
  await page.fill("#project-name", `Picked ${vp.name}`);
  await page.fill("#project-id", ids.created);
  await picker.locator('[role=radio][data-template="human_owned"]').click();
  assert.equal(await picker.locator('[role=radio][data-template="human_owned"]').getAttribute("aria-checked"), "true");
  assert.ok(
    (await page.getByTestId("template-preview").innerText()).includes(sampleLine(byKey("human_owned"))),
    "the form previews the choice",
  );
  // The form shows the opening of the text and a way to read the rest.
  const more = page.getByRole("button", { name: "Show the full text" });
  await more.waitFor({ state: "visible" });
  await page.getByRole("heading", { name: "Conventions", exact: true }).scrollIntoViewIfNeeded();
  await page.getByRole("heading", { name: "Conventions", exact: true }).evaluate((el) =>
    el.scrollIntoView({ block: "start" }),
  );
  await assertNoOverflow(page, `${vp.name} new project`);
  await shot(page, vp, "07-new-project-conventions");
  await more.click();
  await page.getByRole("button", { name: "Show less" }).waitFor({ state: "visible" });
  await page.getByRole("button", { name: "Create project" }).click();
  await page.waitForURL(new RegExp(`/${ids.created}/overview$`), { timeout: 15_000 });
  const picked = await projectByIdent(page, ids.created);
  const pickedPage = await conventionsPage(page, picked.id);
  assert.equal(pickedPage?.content, byKey("human_owned").content, "the new project holds the chosen template");
  await page.goto(`${base}/#/${ids.created}/conventions`);
  await status.waitFor({ state: "visible", timeout: 15_000 });
  console.log(`ok   ${vp.name}: the create-project picker sends the template and the tab shows it`);

  await context.close();
}

/** A viewer reads the tab: no templates, no changes. */
async function viewerScenario(browser: Browser, base: string, ident: string, errors: string[]) {
  const vp = VIEWPORTS[0];
  const admin = await newContext(browser, vp);
  const adminPage = await signIn(admin, base, "ada", ADMIN_PASSWORD);
  const project = await projectByIdent(adminPage, ident);

  const context = await newContext(browser, vp);
  const page = await signIn(context, base, "mia", PASSWORD);
  page.on("pageerror", (err) => errors.push(`viewer: ${err}`));
  await page.goto(`${base}/#/${ident}/conventions`);
  const chooser = page.getByTestId("conventions-chooser");
  await chooser.waitFor({ state: "visible", timeout: 15_000 });
  assert.equal(await chooser.getAttribute("data-state"), "none");
  await page.waitForTimeout(500);
  assert.equal(await chooser.locator("[data-template]").count(), 0, "a viewer gets no templates");
  assert.ok((await chooser.innerText()).includes("no conventions yet"), "a viewer is told there are none");
  await shot(page, vp, "08-viewer-no-conventions");

  await api(adminPage, "POST", "/pages", {
    project_id: project.id,
    title: "Conventions",
    status: "active",
    conventions: true,
    content: "# Conventions\n\nViewers read this.",
  });
  await page.reload();
  const status = page.getByTestId("conventions-status");
  await status.waitFor({ state: "visible", timeout: 15_000 });
  await page.getByText("Viewers read this.").waitFor({ state: "visible" });
  await page.getByRole("button", { name: "More actions" }).first().click();
  await page.getByRole("menuitem", { name: "Copy link" }).waitFor({ state: "visible" });
  assert.equal(await page.getByRole("menuitem", { name: "Change template…" }).count(), 0, "a viewer can't change the template");
  assert.equal(await page.getByRole("menuitem", { name: "Leave out of briefings…" }).count(), 0, "or turn it off");
  console.log("ok   viewer: the tab is read-only, with and without a page");
  await context.close();
  await admin.close();
}

/** A signed-out reader of a published project reads the tab. */
async function signedOutScenario(browser: Browser, base: string, ident: string, errors: string[]) {
  const vp = VIEWPORTS[1];
  const context = await newContext(browser, vp);
  const page = await context.newPage();
  page.on("pageerror", (err) => errors.push(`signed out: ${err}`));
  await page.goto(`${base}/#/${ident}/conventions`);
  const status = page.getByTestId("conventions-status");
  await status.waitFor({ state: "visible", timeout: 15_000 });
  await page.getByText("Readers read this.").waitFor({ state: "visible" });
  assert.ok(page.url().endsWith(`#/${ident}/conventions`), "a reader stays on the tab");
  // Open the menu, then check it holds only reading actions.
  await page.getByRole("button", { name: "More actions" }).first().click();
  const items = page.getByRole("menu").getByRole("menuitem");
  await items.first().waitFor({ state: "visible" });
  const labels = (await items.allInnerTexts()).map((s) => s.trim());
  assert.deepEqual(labels, ["Copy link"], `a reader's menu reads only (got ${JSON.stringify(labels)})`);
  await page.keyboard.press("Escape");
  await assertNoOverflow(page, "signed out conventions");
  console.log("ok   signed out: a published project's tab is readable, with no changes on offer");
  await context.close();
}

/** The project index fails to load: the tab says so and can try again. */
async function indexFailureScenario(browser: Browser, base: string, ident: string, errors: string[]) {
  const vp = VIEWPORTS[0];
  // A fresh context, so no cached copy of the project stands in.
  const context = await newContext(browser, vp);
  const page = await signIn(context, base, "ada", ADMIN_PASSWORD);
  page.on("pageerror", (err) => errors.push(`index failure: ${err}`));
  const project = await projectByIdent(page, ident);
  let failing = true;
  await page.route(`**/api/projects/${project.id}/index*`, (route) =>
    failing
      ? route.fulfill({ status: 500, contentType: "application/json", body: JSON.stringify({ error: "index unavailable" }) })
      : route.continue(),
  );
  await page.goto(`${base}/#/${ident}/conventions`);
  await page.getByText("Couldn't load conventions").waitFor({ state: "visible", timeout: 20_000 });
  await shot(page, vp, "09-index-failure");
  failing = false;
  await page.getByRole("button", { name: "Try again" }).click();
  await page.getByTestId("conventions-status").waitFor({ state: "visible", timeout: 15_000 });
  console.log("ok   index failure: the tab shows the error and recovers on Try again");
  await context.close();
}

async function main(): Promise<number> {
  if (!existsSync(BIN)) {
    console.error(`no binary at ${BIN}: run \`mkdir -p web/dist && cargo build\` (or set LIFIC_BIN)`);
    return 1;
  }
  if (!existsSync(join(ROOT, "web", "dist", "index.html"))) {
    console.error("web/dist/index.html missing: run `cd web && bun run build` first");
    return 1;
  }
  mkdirSync(SHOTS, { recursive: true });

  const scratch = mkdtempSync(join(tmpdir(), "lific-conventions-"));
  const config = join(scratch, "lific.toml");
  const db = join(scratch, "conventions.db");
  let server: ChildProcess | null = null;
  let browser: Browser | null = null;
  let serverLog = "";
  const deadline = setTimeout(() => {
    console.error("conventions test deadline exceeded");
    server?.kill("SIGKILL");
    process.exit(1);
  }, 300_000);

  try {
    cli(config, db, [
      "init", "--no-service", "--json",
      "--name", "Conventions Admin",
      "--auth-mode", "passwords",
      "--password", ADMIN_PASSWORD,
    ]);
    cli(config, db, [
      "user", "create", "--username", "ada",
      "--email", "ada@example.test", "--password", ADMIN_PASSWORD, "--admin",
    ]);
    cli(config, db, [
      "user", "create", "--username", "mia",
      "--email", "mia@example.test", "--password", PASSWORD,
    ]);
    for (const ident of ["BAREA", "BAREB", "VIEW", "PUB"]) {
      cli(config, db, ["project", "create", "--name", `Project ${ident}`, "--identifier", ident, "--json"]);
    }
    cli(config, db, ["member", "add", "--project", "VIEW", "--user", "mia", "--role", "viewer"]);
    // Publishing has no CLI by design (see public-projects.ts).
    const seed = new Database(db);
    seed.run("UPDATE projects SET is_public = 1 WHERE identifier = 'PUB'");
    seed.close();

    const port = await freePort();
    const base = `http://127.0.0.1:${port}`;
    server = spawn(BIN, ["--config", config, "--db", db, "start", "--port", String(port), "--host", "127.0.0.1"], {
      stdio: ["ignore", "pipe", "pipe"],
    });
    server.stdout?.on("data", (d: Buffer) => (serverLog += d.toString()));
    server.stderr?.on("data", (d: Buffer) => (serverLog += d.toString()));
    await waitForServer(`${base}/`, 30_000);

    browser = await chromium.launch({ executablePath: process.env.PLAYWRIGHT_EXECUTABLE_PATH });
    const errors: string[] = [];

    // Setup through the API: the templates, projects whose conventions page
    // is empty, a viewer project with no conventions page at all, and a
    // published project with one.
    const probe = await browser.newContext();
    const probePage = await signIn(probe, base, "ada", ADMIN_PASSWORD);
    const templates: Template[] = await api(probePage, "GET", "/conventions/templates");
    assert.deepEqual(
      templates.map((t) => t.key),
      ["empty", "ai_managed", "human_managed", "human_owned"],
      "templates arrive in order",
    );
    for (const ident of ["EMPTA", "EMPTB"]) {
      await api(probePage, "POST", "/projects", { name: `Empty ${ident}`, identifier: ident });
    }
    const view = await projectByIdent(probePage, "VIEW");
    const viewPage = await conventionsPage(probePage, view.id);
    if (viewPage) await api(probePage, "DELETE", `/pages/${viewPage.id}`);
    const pub = await projectByIdent(probePage, "PUB");
    const pubPage = await conventionsPage(probePage, pub.id);
    const readerText = "# Conventions\n\nReaders read this.";
    if (pubPage) await api(probePage, "PUT", `/pages/${pubPage.id}`, { content: readerText });
    else {
      await api(probePage, "POST", "/pages", {
        project_id: pub.id, title: "Conventions", status: "active", conventions: true, content: readerText,
      });
    }
    await probe.close();

    await scenario(browser, base, VIEWPORTS[0], { bare: "BAREA", emptyPage: "EMPTA", created: "PICKA" }, templates, errors);
    await scenario(browser, base, VIEWPORTS[1], { bare: "BAREB", emptyPage: "EMPTB", created: "PICKB" }, templates, errors);
    await viewerScenario(browser, base, "VIEW", errors);
    await signedOutScenario(browser, base, "PUB", errors);
    await indexFailureScenario(browser, base, "PUB", errors);

    assert.deepEqual(errors, [], "no page errors");
    console.log(`conventions: all checks passed (screenshots in ${SHOTS})`);
    return 0;
  } catch (err) {
    console.error(err);
    if (serverLog) console.error("--- server log ---\n" + serverLog.slice(-4000));
    return 1;
  } finally {
    clearTimeout(deadline);
    await browser?.close().catch(() => {});
    server?.kill("SIGTERM");
    rmSync(scratch, { recursive: true, force: true });
  }
}

process.exit(await main());

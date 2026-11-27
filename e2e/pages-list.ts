#!/usr/bin/env bun
/**
 * The redesigned Pages tab: folder pane, list, reading pane, keyboard,
 * filters, bulk actions and the context menu.
 *
 * Spawns the debug binary against a scratch config + DB, seeds one project
 * through the API (nested folders, pages in every status, a pinned page,
 * labels, and an issue that links one page), signs in, and checks at
 * 1440x900 that:
 *
 *   1. All pages lists every live page once, leaves out archived pages and
 *      shows the pinned page in the strip instead of as a row, with a group
 *      header per folder, and the folder pane offers Conventions;
 *   2. scoping to a folder shows its pages and its subfolder's, and an empty
 *      folder shows the empty state;
 *   3. clicking a row previews it, J moves the preview, Enter opens it;
 *   4. P pins the focused row and it moves to the strip;
 *   5. C opens the composer, Enter creates the page in the scoped folder;
 *   6. F opens the filter, Draft narrows the list and shows a chip, and
 *      removing the chip restores the list;
 *   7. X and the checkbox select two rows, and the bulk bar moves them;
 *   8. the context menu archives a row, which moves it to Archived in the
 *      folder pane;
 *   9. Archived's Restore brings the page back, and Group by Day edited
 *      puts every page under a day header;
 *  10. the linked page's row shows a link count of 1;
 *
 * and at 390x844 (touch) that the tab does not scroll sideways and the
 * folder drawer opens from the topbar and closes when a folder is picked.
 *
 * Run:  cd e2e && bun pages-list.ts
 * Binary: $LIFIC_BIN, else target/debug/lific (debug rust-embed reads
 * web/dist from disk, so build the web UI first).
 * Screenshots: $SHOTS, else /tmp/opencode/pages-list-e2e.
 */
import { strict as assert } from "node:assert";
import { chromium, type Browser, type BrowserContext, type Page } from "playwright";
import { spawn, execFileSync, type ChildProcess } from "node:child_process";
import { mkdtempSync, mkdirSync, rmSync, existsSync } from "node:fs";
import { createServer } from "node:net";
import { tmpdir } from "node:os";
import { join, resolve } from "node:path";

const ROOT = resolve(import.meta.dir, "..");
const BIN = process.env.LIFIC_BIN ?? join(ROOT, "target", "debug", "lific");
const SHOTS = process.env.SHOTS ?? "/tmp/opencode/pages-list-e2e";
const PASSWORD = "pages-list-admin-password-123";
const PROJ = "PRJ";

interface ApiPage {
  id: number;
  identifier: string;
  title: string;
  status: string;
  folder_id: number | null;
  pinned: boolean;
  conventions: boolean;
  labels: string[];
}

interface Seed {
  projectId: number;
  folders: { guides: number; advanced: number; notes: number; later: number };
  pages: Record<string, ApiPage>;
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
        headers: { "Content-Type": "application/json" },
        body: body === undefined ? undefined : JSON.stringify(body),
      });
      if (!res.ok) throw new Error(`${method} ${path}: ${res.status} ${await res.text()}`);
      return res.json();
    },
    [method, path, body] as const,
  );
}

async function signIn(context: BrowserContext, base: string) {
  const page = await context.newPage();
  await page.goto(`${base}/login`, { waitUntil: "load" });
  await page.fill("#login-identity", "ada");
  await page.fill("#login-password", PASSWORD);
  await page.click("button[type=submit]");
  await page.waitForURL((url) => !url.href.includes("login"), { timeout: 15_000 });
  return page;
}

async function shot(page: Page, name: string) {
  // Let fades and transitions settle.
  await page.waitForTimeout(350);
  await page.screenshot({ path: join(SHOTS, `${name}.png`) });
}

async function waitFor<T>(page: Page, read: () => Promise<T>, ok: (v: T) => boolean, what: string): Promise<T> {
  const deadline = Date.now() + 10_000;
  let last = await read();
  while (!ok(last) && Date.now() < deadline) {
    await page.waitForTimeout(100);
    last = await read();
  }
  assert.ok(ok(last), `${what} (last: ${JSON.stringify(last)})`);
  return last;
}

const sorted = (ids: number[]) => [...ids].sort((a, b) => a - b);
const sameSet = (a: number[], b: number[]) => JSON.stringify(sorted(a)) === JSON.stringify(sorted(b));

/** Row ids in the list, in DOM order. */
function rowIds(page: Page): Promise<number[]> {
  return page.$$eval('[data-testid="pages-list"] [data-testid="page-row"]', (els) =>
    els.map((e) => Number(e.getAttribute("data-page-id"))),
  );
}

async function waitForRows(page: Page, expected: number[], what: string) {
  await waitFor(page, () => rowIds(page), (ids) => ids.length === expected.length && sameSet(ids, expected), what);
}

const row = (page: Page, id: number) => page.locator(`[data-testid="pages-list"] [data-testid="page-row"][data-page-id="${id}"]`);

/** A plain click on the row, away from its link and buttons. */
async function clickRow(page: Page, id: number, button: "left" | "right" = "left") {
  await row(page, id).locator("time").first().click({ button });
}

async function projectPages(page: Page, projectId: number): Promise<ApiPage[]> {
  const all: ApiPage[] = await api(page, "GET", `/pages?project_id=${projectId}`);
  return all.filter((p) => !p.conventions);
}

/** What All pages with no filters should list as rows. */
async function expectedBrowse(page: Page, projectId: number): Promise<number[]> {
  return (await projectPages(page, projectId)).filter((p) => p.status !== "archived" && !p.pinned).map((p) => p.id);
}

/** The Pages tab, scoped to All pages. */
async function openBrowse(page: Page, base: string, desktop = true) {
  await page.goto(`${base}/#/${PROJ}/pages`);
  const tree = page.getByTestId("folder-tree");
  await tree.waitFor({ state: desktop ? "visible" : "attached", timeout: 15_000 });
  if (desktop) await tree.getByRole("button", { name: /^All pages/ }).click();
}

function folderButton(page: Page, folderId: number) {
  // The second button in a folder entry is its name, which scopes the list.
  return page.getByTestId("folder-tree").locator(`[data-folder-id="${folderId}"] > button`).nth(1);
}

async function paneTitle(page: Page): Promise<string> {
  const pane = page.getByTestId("page-pane");
  if (!(await pane.isVisible())) return "";
  return (await pane.locator("h2").innerText()).trim();
}

async function seed(page: Page): Promise<Seed> {
  const projects = await api(page, "GET", "/projects");
  const project = projects.find((p: { identifier: string }) => p.identifier === PROJ);
  assert.ok(project, `project ${PROJ} exists`);
  const pid: number = project.id;

  await api(page, "POST", "/labels", { project_id: pid, name: "spec", color: "#3B82F6" });
  await api(page, "POST", "/labels", { project_id: pid, name: "ops", color: "#EF4444" });

  const folder = async (name: string, parent_id?: number) =>
    (await api(page, "POST", "/folders", { project_id: pid, name, ...(parent_id ? { parent_id } : {}) })).id as number;
  const guides = await folder("Guides");
  const advanced = await folder("Advanced", guides);
  const notes = await folder("Notes");
  const later = await folder("Later");

  const make = async (
    key: string,
    title: string,
    folder_id: number | null,
    status: string,
    extra: { labels?: string[]; pinned?: boolean } = {},
  ): Promise<[string, ApiPage]> => {
    const created: ApiPage = await api(page, "POST", "/pages", {
      project_id: pid,
      title,
      content: `${title} body text for the excerpt.`,
      status: status === "archived" ? "active" : status,
      labels: extra.labels ?? [],
      ...(folder_id != null ? { folder_id } : {}),
    });
    const patch: Record<string, unknown> = {};
    if (status === "archived") patch.status = "archived";
    if (extra.pinned) patch.pinned = true;
    const final = Object.keys(patch).length ? await api(page, "PUT", `/pages/${created.id}`, patch) : created;
    return [key, final];
  };

  const pages = Object.fromEntries([
    await make("started", "Getting started", guides, "active", { labels: ["spec"] }),
    await make("internals", "Deep internals", advanced, "active"),
    await make("outline", "Draft outline", guides, "draft"),
    await make("release", "Release notes", notes, "complete", { labels: ["ops"] }),
    await make("meeting", "Meeting log", null, "draft"),
    await make("old", "Old design", notes, "archived"),
    await make("handbook", "Team handbook", null, "active", { pinned: true }),
    await make("linked", "Linked spec", null, "active", { labels: ["spec"] }),
    await make("scratch", "Scratch ideas", notes, "draft"),
  ]) as Record<string, ApiPage>;
  assert.equal(pages.old.status, "archived");
  assert.equal(pages.handbook.pinned, true);

  await api(page, "POST", "/issues", {
    project_id: pid,
    title: "Follow the spec",
    description: `The rules live in ${pages.linked.identifier}.`,
  });

  return { projectId: pid, folders: { guides, advanced, notes, later }, pages };
}

async function desktop(browser: Browser, base: string, errors: string[]): Promise<Seed> {
  const context = await browser.newContext({ viewport: { width: 1440, height: 900 } });
  const page = await signIn(context, base);
  page.setDefaultTimeout(10_000);
  page.on("pageerror", (err) => errors.push(`desktop: ${err}`));
  const s = await seed(page);
  const P = s.pages;
  const F = s.folders;

  // ── 1. All pages ─────────────────────────────────────────────────────
  await openBrowse(page, base);
  const live = await expectedBrowse(page, s.projectId);
  assert.ok(!live.includes(P.old.id) && !live.includes(P.handbook.id), "the expectation leaves out archived and pinned");
  await waitForRows(page, live, "Browse lists every live, unpinned page");
  const ids = await rowIds(page);
  assert.equal(new Set(ids).size, ids.length, "no page is listed twice");
  assert.equal(await row(page, P.old.id).count(), 0, "the archived page is not a row");
  assert.equal(await row(page, P.handbook.id).count(), 0, "the pinned page is not a row");
  await page.getByTestId("pinned-strip").locator(`[data-page-id="${P.handbook.id}"]`).waitFor({ state: "visible" });
  for (const key of ["f:root", `f:${F.guides}`, `f:${F.advanced}`, `f:${F.notes}`]) {
    await page.locator(`[data-testid="pages-list"] [data-group="${key}"]`).waitFor({ state: "visible" });
  }
  assert.equal(await page.locator(`[data-group="f:${F.later}"]`).count(), 0, "an empty folder gets no group outside its scope");
  await page.getByTestId("pages-pulse").waitFor({ state: "visible" });
  await page.getByTestId("conventions-entry").waitFor({ state: "visible" });
  assert.equal(await page.getByRole("tab").count(), 0, "there are no sub-tabs");
  await shot(page, "01-browse");
  console.log("ok   All pages lists each live page once, pins in the strip, a header per folder");

  // ── 10. Link count (checked here, while every page is in place) ──────
  const links = row(page, P.linked.id).getByTestId("page-links");
  await links.waitFor({ state: "visible" });
  assert.equal((await links.innerText()).trim(), "1", "the linked page shows one incoming link");
  assert.equal(await row(page, P.meeting.id).getByTestId("page-links").count(), 0, "an unlinked page shows no count");
  console.log("ok   the linked page's row shows a link count of 1");

  // ── 2. Folder scope ──────────────────────────────────────────────────
  await folderButton(page, F.guides).click();
  await waitForRows(page, [P.started.id, P.outline.id, P.internals.id], "Guides shows its pages and Advanced's");
  assert.equal(await page.getByTestId("pinned-strip").count(), 0, "the pinned strip is only on All pages");
  await shot(page, "02-scope-guides");
  await folderButton(page, F.later).click();
  const empty = page.getByTestId("pages-empty");
  await empty.waitFor({ state: "visible" });
  assert.ok((await empty.innerText()).includes("No pages in Later yet"), "the empty folder says so");
  assert.equal((await rowIds(page)).length, 0, "the empty folder lists nothing");
  await shot(page, "03-scope-empty");
  await page.getByTestId("folder-tree").getByRole("button", { name: /^All pages/ }).click();
  await waitForRows(page, live, "All pages restores the list");
  console.log("ok   a folder scope shows its pages and its subfolder's; an empty folder shows the empty state");

  // ── 3. Reading pane, J, Enter ────────────────────────────────────────
  const titleOf = new Map(Object.values(P).map((p) => [p.id, p.title]));
  const order = await rowIds(page);
  await clickRow(page, order[0]);
  await waitFor(page, () => paneTitle(page), (t) => t === titleOf.get(order[0]), "the pane shows the clicked page");
  assert.ok(page.url().endsWith(`#/${PROJ}/pages`), "a click with the pane open previews instead of navigating");
  await page.keyboard.press("j");
  await waitFor(page, () => paneTitle(page), (t) => t === titleOf.get(order[1]), "J moves the pane to the next row");
  await shot(page, "04-reading-pane");
  await page.keyboard.press("Enter");
  await page.waitForURL(new RegExp(`#/${PROJ}/pages/${order[1]}$`), { timeout: 15_000 });
  console.log("ok   a click previews, J moves the preview, Enter opens the page");

  // ── 4. P pins ────────────────────────────────────────────────────────
  await openBrowse(page, base);
  await waitForRows(page, live, "back on Browse");
  await clickRow(page, P.meeting.id);
  await waitFor(page, () => paneTitle(page), (t) => t === P.meeting.title, "the pane shows Meeting log");
  await page.keyboard.press("p");
  await waitFor(
    page,
    async () => (await api(page, "GET", `/pages/${P.meeting.id}`)).pinned,
    (v) => v === true,
    "P stored pinned=true",
  );
  await page.getByTestId("pinned-strip").locator(`[data-page-id="${P.meeting.id}"]`).waitFor({ state: "visible" });
  await waitFor(page, () => row(page, P.meeting.id).count(), (n) => n === 0, "the pinned page leaves the rows");
  await shot(page, "05-pinned");
  console.log("ok   P pins the focused row and it moves to the strip");

  // ── 5. Composer in a folder scope ────────────────────────────────────
  await page.keyboard.press("Escape");
  await folderButton(page, F.notes).click();
  await waitForRows(page, [P.release.id, P.scratch.id], "Notes lists its live pages");
  await page.evaluate(() => (document.activeElement as HTMLElement | null)?.blur());
  await page.keyboard.press("c");
  const composer = page.getByTestId("page-composer");
  await composer.waitFor({ state: "visible" });
  const input = composer.getByLabel("New page title");
  await waitFor(page, () => input.evaluate((el) => el === document.activeElement), (v) => v, "the composer title has focus");
  await input.pressSequentially("Composer page");
  await shot(page, "06-composer");
  await input.press("Enter");
  await page.waitForURL(new RegExp(`#/${PROJ}/pages/\\d+$`), { timeout: 15_000 });
  const newId = Number(page.url().match(/\/pages\/(\d+)$/)![1]);
  const made = await api(page, "GET", `/pages/${newId}`);
  assert.equal(made.title, "Composer page", "the page has the typed title");
  assert.equal(made.folder_id, F.notes, "the page lands in the scoped folder");
  assert.equal(made.status, "draft", "the composer starts a draft");
  console.log("ok   C opens the composer and Enter creates the page in the scoped folder");

  // ── 6. Filter ────────────────────────────────────────────────────────
  await openBrowse(page, base);
  const unfiltered = await expectedBrowse(page, s.projectId);
  await waitForRows(page, unfiltered, "Browse before filtering");
  await page.keyboard.press("f");
  const filter = page.getByTestId("pages-filter");
  await filter.waitFor({ state: "visible" });
  await filter.getByRole("button", { name: "Draft", exact: true }).click();
  const drafts = (await projectPages(page, s.projectId)).filter((p) => p.status === "draft").map((p) => p.id);
  assert.ok(drafts.includes(P.meeting.id), "the pinned draft counts as a draft");
  await waitForRows(page, drafts, "Draft lists only drafts, pinned ones included");
  const chips = page.getByTestId("pages-filter-chips");
  await chips.waitFor({ state: "visible" });
  assert.ok((await chips.innerText()).includes("Draft"), "a chip names the filter");
  await page.keyboard.press("Escape");
  await filter.waitFor({ state: "hidden" });
  await shot(page, "07-filter-draft");
  await chips.getByRole("button", { name: "Remove filter: Draft" }).click();
  await chips.waitFor({ state: "hidden" });
  await waitForRows(page, unfiltered, "removing the chip restores the list");
  console.log("ok   F opens the filter, Draft narrows the list with a chip, removing it restores the list");

  // ── 7. Bulk move ─────────────────────────────────────────────────────
  await clickRow(page, P.started.id);
  await waitFor(page, () => paneTitle(page), (t) => t === P.started.title, "the pane shows Getting started");
  await page.keyboard.press("x");
  const bulkBar = page.getByTestId("pages-bulk-bar");
  await bulkBar.waitFor({ state: "visible" });
  const box = row(page, P.internals.id).getByRole("checkbox");
  await box.click();
  await waitFor(page, async () => (await bulkBar.innerText()).includes("2 selected"), (v) => v, "two pages are selected");
  for (const name of ["Move", "Status", "Label", "Pin", "Unpin", "Export"]) {
    await bulkBar.getByRole("button", { name, exact: true }).waitFor({ state: "visible" });
  }
  await bulkBar.getByRole("button", { name: "Move", exact: true }).click();
  await bulkBar.getByRole("button", { name: "Later", exact: true }).click();
  for (const id of [P.started.id, P.internals.id]) {
    await waitFor(
      page,
      async () => (await api(page, "GET", `/pages/${id}`)).folder_id,
      (v) => v === F.later,
      `page ${id} moved to Later`,
    );
  }
  await bulkBar.waitFor({ state: "hidden" });
  await page.locator(`[data-testid="pages-list"] [data-group="f:${F.later}"]`).waitFor({ state: "visible" });
  await shot(page, "08-bulk-moved");
  console.log("ok   X and the checkbox select two rows, and the bulk bar moves them");

  // ── 8. Context menu archive ──────────────────────────────────────────
  await page.keyboard.press("Escape");
  await clickRow(page, P.release.id, "right");
  const menu = page.locator("[data-context-menu]");
  await menu.waitFor({ state: "visible" });
  await menu.getByRole("menuitem", { name: "Pin", exact: true }).waitFor({ state: "visible" });
  await menu.getByRole("menuitem", { name: /^Move to folder/ }).waitFor({ state: "visible" });
  await shot(page, "09-context-menu");
  await menu.getByRole("menuitem", { name: "Archive", exact: true }).click();
  await waitFor(page, () => row(page, P.release.id).count(), (n) => n === 0, "the archived page leaves All pages");
  assert.equal((await api(page, "GET", `/pages/${P.release.id}`)).status, "archived", "the API has it archived");
  const archived = page.getByTestId("archived-entry");
  await waitFor(page, async () => (await archived.innerText()).replace(/\s+/g, " ").trim(), (t) => t === "Archived 2", "the pane counts two archived pages");
  await archived.click();
  await waitForRows(page, [P.release.id, P.old.id], "Archived lists the archived pages only");
  await shot(page, "10-archived");
  console.log("ok   the context menu archives a row and it shows under Archived");

  // ── 9. Restore, and Group by Day edited ──────────────────────────────
  await row(page, P.release.id).getByRole("button", { name: "Restore", exact: true }).click();
  await waitFor(
    page,
    async () => (await api(page, "GET", `/pages/${P.release.id}`)).status,
    (v) => v === "active",
    "Restore stored status=active",
  );
  await waitForRows(page, [P.old.id], "the restored page leaves Archived");
  await page.getByTestId("folder-tree").getByRole("button", { name: /^All pages/ }).click();
  await row(page, P.release.id).waitFor({ state: "visible" });
  await page.getByRole("button", { name: /^Display/ }).click();
  await page.getByTestId("pages-display").getByRole("button", { name: "Day edited", exact: true }).click();
  await page.locator('[data-testid="pages-list"] [data-group="d:Today"]').waitFor({ state: "visible" });
  assert.equal(await page.locator('[data-testid="pages-list"] [data-group^="f:"]').count(), 0, "no folder headers remain");
  await waitForRows(page, await expectedBrowse(page, s.projectId), "day groups list the same pages");
  await page.getByTestId("pages-display").getByRole("button", { name: "Folder", exact: true }).click();
  console.log("ok   Restore brings the page back; Group by Day edited puts pages under day headers");

  await context.close();
  return s;
}

async function phone(browser: Browser, base: string, s: Seed, errors: string[]) {
  const context = await browser.newContext({
    viewport: { width: 390, height: 844 },
    isMobile: true,
    hasTouch: true,
    deviceScaleFactor: 2,
  });
  const page = await signIn(context, base);
  page.setDefaultTimeout(10_000);
  page.on("pageerror", (err) => errors.push(`phone: ${err}`));

  await openBrowse(page, base, false);
  await waitForRows(page, await expectedBrowse(page, s.projectId), "the phone lists the live pages");
  const overflow = () =>
    page.evaluate(() => document.scrollingElement!.scrollWidth - window.innerWidth);
  assert.ok((await overflow()) <= 0, `the phone does not scroll sideways (by ${await overflow()}px)`);
  await shot(page, "11-phone-browse");

  const tree = page.getByTestId("folder-tree");
  const treeRight = async () => {
    const b = await tree.boundingBox();
    return b ? b.x + b.width : -1;
  };
  const treeLeft = async () => (await tree.boundingBox())?.x ?? -1000;
  assert.ok((await treeRight()) <= 0, "the folder drawer starts closed");
  const opener = page.getByRole("button", { name: /^Folders:/ });
  assert.equal(await opener.getAttribute("aria-label"), "Folders: All pages");
  await opener.tap();
  await waitFor(page, treeLeft, (x) => x >= 0, "the folder drawer slides in");
  assert.ok((await overflow()) <= 0, "the open drawer does not make the page scroll sideways");
  await shot(page, "12-phone-drawer");

  await folderButton(page, s.folders.notes).tap();
  await waitFor(page, treeRight, (x) => x <= 0, "picking a folder closes the drawer");
  await waitFor(page, () => opener.getAttribute("aria-label"), (v) => v === "Folders: Notes", "the button names the folder");
  const inNotes = (await projectPages(page, s.projectId))
    .filter((p) => p.folder_id === s.folders.notes && p.status !== "archived")
    .map((p) => p.id);
  await waitForRows(page, inNotes, "the phone list is scoped to Notes");
  assert.ok((await overflow()) <= 0, "the scoped list does not scroll sideways");
  await shot(page, "13-phone-scoped");
  console.log("ok   phone: no sideways scroll, the folder drawer opens from the topbar and closes on a pick");

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

  const scratch = mkdtempSync(join(tmpdir(), "lific-pages-list-"));
  const config = join(scratch, "lific.toml");
  const db = join(scratch, "pages.db");
  let server: ChildProcess | null = null;
  let browser: Browser | null = null;
  let serverLog = "";
  const deadline = setTimeout(() => {
    console.error("pages-list test deadline exceeded");
    server?.kill("SIGKILL");
    process.exit(1);
  }, 300_000);

  try {
    cli(config, db, [
      "init", "--no-service", "--json",
      "--name", "Pages Admin",
      "--auth-mode", "passwords",
      "--password", PASSWORD,
    ]);
    cli(config, db, [
      "user", "create", "--username", "ada",
      "--email", "ada@example.test", "--password", PASSWORD, "--admin",
    ]);
    cli(config, db, ["project", "create", "--name", "Pages Project", "--identifier", PROJ, "--json"]);

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

    const s = await desktop(browser, base, errors);
    await phone(browser, base, s, errors);

    assert.deepEqual(errors, [], "no page errors");
    console.log(`pages-list: all checks passed (screenshots in ${SHOTS})`);
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

#!/usr/bin/env bun
/**
 * LIF-524: the per-project conventions page in the web UI.
 *
 * Spawns the debug binary against a scratch config + DB, signs in, and at a
 * desktop viewport (1440x900) and a phone viewport (390x844) checks that:
 *
 *   - a project without a conventions page marks the folder pane's
 *     Conventions entry as missing, and the entry shows the three templates
 *     and a blank start;
 *   - picking a template creates the page and lands on it, labelled as the
 *     project's conventions;
 *   - the page view's switch turns it off, the off state survives a reload,
 *     and the text is unchanged; the entry and the block then show "Off"
 *     and the page stays out of the ordinary list;
 *   - a project whose conventions page is empty offers the templates (no
 *     blank start), and a template fills in that same page;
 *   - the create-project picker sends the chosen template, and the new
 *     project's Pages block shows the page.
 *
 * Also checks once that a viewer gets no Conventions entry when none is
 * written.
 *
 * Run:  cd e2e && bun conventions.ts
 * Binary: $LIFIC_BIN, else target/debug/lific (debug rust-embed reads
 * web/dist from disk, so build the web UI first).
 * Screenshots: $SHOTS, else /tmp/opencode/conventions-e2e.
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
const SHOTS = process.env.SHOTS ?? "/tmp/opencode/conventions-e2e";
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

/** Pick an entry in the Pages folder pane, opening the drawer below lg. */
async function pickInFolderPane(page: Page, testid: string) {
  const entry = page.getByTestId("folder-tree").getByTestId(testid);
  await entry.waitFor({ state: "attached", timeout: 15_000 });
  if ((page.viewportSize()?.width ?? 1440) < 1024) await page.getByRole("button", { name: /^Folders:/ }).click();
  await entry.click();
}

/** The Pages folder pane's Conventions entry, which holds the conventions block. */
async function openConventions(page: Page, base: string, ident: string) {
  if (!page.url().endsWith(`/${ident}/pages`)) await page.goto(`${base}/${ident}/pages`);
  await pickInFolderPane(page, "conventions-entry");
}

async function openAllPages(page: Page) {
  const entry = page.getByTestId("folder-tree").getByRole("button", { name: /^All pages/ });
  await entry.waitFor({ state: "attached", timeout: 15_000 });
  if ((page.viewportSize()?.width ?? 1440) < 1024) await page.getByRole("button", { name: /^Folders:/ }).click();
  await entry.click();
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

  await page.goto(`${base}/${ids.bare}/pages`);
  // The folder pane lists Conventions above the folders and says when it
  // is missing or off.
  const entry = page.getByTestId("conventions-entry");
  await entry.waitFor({ state: "attached", timeout: 15_000 });
  assert.equal(await entry.getAttribute("data-state"), "missing", "the entry says there is none");
  await openConventions(page, base, ids.bare);
  const section = page.getByTestId("conventions-section");
  await section.waitFor({ state: "visible", timeout: 15_000 });
  assert.equal(await section.getAttribute("data-state"), "none", "no page: the empty state shows");
  for (const key of ["ai_managed", "human_managed", "human_owned", "blank"]) {
    await section.locator(`[data-template="${key}"]`).waitFor({ state: "visible" });
  }
  for (const key of ["ai_managed", "human_managed", "human_owned"]) {
    const t = byKey(key);
    const text = await section.locator(`[data-template="${key}"]`).innerText();
    assert.ok(text.includes(t.name), `${key} button names the template`);
    if (t.summary) assert.ok(text.includes(t.summary), `${key} button shows the summary`);
  }
  assert.equal(await section.locator('[data-template="empty"]').count(), 0, "the empty template is the blank start");
  await assertNoOverflow(page, `${vp.name} pages, no conventions`);
  await shot(page, vp, "01-pages-no-conventions");
  console.log(`ok   ${vp.name}: a project without a conventions page offers the templates and a blank start`);

  // ── Pick a template ─────────────────────────────────────────────────
  await section.locator('[data-template="human_managed"]').click();
  await page.waitForURL(/\/pages\/\d+$/, { timeout: 15_000 });
  const createdId = Number(page.url().match(/\/pages\/(\d+)$/)![1]);
  const banner = page.getByTestId("conventions-banner");
  await banner.waitFor({ state: "visible", timeout: 15_000 });
  assert.ok((await banner.innerText()).includes("This project's conventions"), "the page is labelled");
  const created = await api(page, "GET", `/pages/${createdId}`);
  assert.equal(created.conventions, true, "the new page is the conventions page");
  assert.equal(created.conventions_enabled, true, "a new conventions page starts on");
  assert.equal(created.content, byKey("human_managed").content, "the page holds the template");
  assert.equal(created.status, "active", "the page is created active");
  const toggle = page.getByRole("switch", { name: "Include in agents' briefings" });
  assert.equal(await toggle.getAttribute("aria-checked"), "true", "the switch starts on");
  await assertNoOverflow(page, `${vp.name} conventions page`);
  await shot(page, vp, "02-conventions-page-on");
  console.log(`ok   ${vp.name}: a template creates the page and lands on it`);

  // ── Switch it off; it stays off after a reload, text intact ─────────
  await toggle.click();
  await waitFor(
    page,
    async () => (await api(page, "GET", `/pages/${createdId}`)).conventions_enabled,
    (v) => v === false,
    "the switch stored conventions_enabled=false",
  );
  await page.reload();
  await banner.waitFor({ state: "visible", timeout: 15_000 });
  assert.equal(await toggle.getAttribute("aria-checked"), "false", "the switch is still off after a reload");
  assert.ok((await banner.innerText()).includes("leave it out"), "the off state is explained");
  const afterOff = await api(page, "GET", `/pages/${createdId}`);
  assert.equal(afterOff.content, created.content, "turning it off keeps the text");
  await shot(page, vp, "03-conventions-page-off");
  console.log(`ok   ${vp.name}: the switch turns it off, it survives a reload, the text stays`);

  // ── The Pages block shows it, off, and the tree leaves it out ───────
  await openConventions(page, base, ids.bare);
  await section.waitFor({ state: "visible", timeout: 15_000 });
  await waitFor(page, () => section.getAttribute("data-state"), (v) => v === "page", "the block shows the page");
  await section.getByTestId("conventions-off").waitFor({ state: "visible" });
  assert.ok((await section.innerText()).includes(created.title), "the block names the page");
  await assertNoOverflow(page, `${vp.name} pages, with conventions`);
  await shot(page, vp, "04-pages-conventions-off");
  await openAllPages(page);
  await page.getByTestId("page-row").filter({ hasText: "Release checklist" }).waitFor({ state: "visible", timeout: 15_000 });
  assert.equal(
    await page.locator(`[data-testid="page-row"][data-page-id="${createdId}"]`).count(),
    0,
    "the conventions page is not repeated in the list",
  );
  assert.equal(await entry.getAttribute("data-state"), "off", "the entry says it is off");
  await pickInFolderPane(page, "conventions-entry");
  await section.locator("button").first().click();
  await page.waitForURL(new RegExp(`/pages/${createdId}$`));
  console.log(`ok   ${vp.name}: the Pages block shows the page with Off, and the tree leaves it out`);

  // ── A conventions page with no text yet ─────────────────────────────
  const emptyProject = await projectByIdent(page, ids.emptyPage);
  const emptyPage = await conventionsPage(page, emptyProject.id);
  assert.ok(emptyPage, "an API-created project has a conventions page");
  assert.equal(emptyPage.content.trim(), "", "and the empty template leaves it blank");
  await openConventions(page, base, ids.emptyPage);
  await section.waitFor({ state: "visible", timeout: 15_000 });
  assert.equal(await section.getAttribute("data-state"), "empty", "an empty page offers the templates");
  await section.locator('[data-template="ai_managed"]').waitFor({ state: "visible" });
  assert.equal(await section.locator('[data-template="blank"]').count(), 0, "no blank start: the page exists");
  await assertNoOverflow(page, `${vp.name} pages, empty conventions`);
  await shot(page, vp, "05-pages-empty-conventions");
  await section.locator('[data-template="ai_managed"]').click();
  await page.waitForURL(new RegExp(`/pages/${emptyPage.id}$`), { timeout: 15_000 });
  await page.getByTestId("conventions-banner").waitFor({ state: "visible" });
  const filled = await api(page, "GET", `/pages/${emptyPage.id}`);
  assert.equal(filled.content, byKey("ai_managed").content, "the template filled in the same page");
  console.log(`ok   ${vp.name}: an empty conventions page takes a template in place`);

  // ── The create-project picker ───────────────────────────────────────
  await page.goto(`${base}/projects/new`);
  await page.locator("#project-name").waitFor({ state: "visible", timeout: 15_000 });
  const radios = page.getByRole("radiogroup", { name: "Conventions template" });
  await radios.locator('[data-template="human_owned"]').waitFor({ state: "visible" });
  assert.equal(
    await radios.locator('[data-template="empty"]').getAttribute("aria-checked"),
    "true",
    "Empty is the default",
  );
  for (const t of templates) {
    const text = await radios.locator(`[data-template="${t.key}"]`).innerText();
    assert.ok(text.includes(t.name), `${t.key} option is named`);
    if (t.summary) assert.ok(text.includes(t.summary), `${t.key} option shows its summary`);
  }
  await page.fill("#project-name", `Picked ${vp.name}`);
  await page.fill("#project-id", ids.created);
  await radios.locator('[data-template="human_owned"]').click();
  assert.equal(await radios.locator('[data-template="human_owned"]').getAttribute("aria-checked"), "true");
  await radios.scrollIntoViewIfNeeded();
  await assertNoOverflow(page, `${vp.name} new project`);
  await shot(page, vp, "06-new-project-picker");
  await page.getByRole("button", { name: "Create project" }).click();
  await page.waitForURL(new RegExp(`/${ids.created}/overview$`), { timeout: 15_000 });
  const picked = await projectByIdent(page, ids.created);
  const pickedPage = await conventionsPage(page, picked.id);
  assert.ok(pickedPage, "the new project has a conventions page");
  assert.equal(pickedPage.content, byKey("human_owned").content, "it holds the chosen template");
  await openConventions(page, base, ids.created);
  await section.waitFor({ state: "visible", timeout: 15_000 });
  assert.equal(await section.getAttribute("data-state"), "page", "the new project's Pages block shows the page");
  assert.equal(await section.getByTestId("conventions-off").count(), 0, "and it is on");
  await assertNoOverflow(page, `${vp.name} new project pages`);
  await shot(page, vp, "07-new-project-pages");
  console.log(`ok   ${vp.name}: the create-project picker sends the template and Pages shows it`);

  await context.close();
}

/** A viewer can't write conventions, so with none written there is no entry. */
async function viewerScenario(browser: Browser, base: string, ident: string, errors: string[]) {
  const context = await newContext(browser, VIEWPORTS[0]);
  const page = await signIn(context, base, "mia", PASSWORD);
  page.on("pageerror", (err) => errors.push(`viewer: ${err}`));
  await page.goto(`${base}/${ident}/pages`);
  const tree = page.getByTestId("folder-tree");
  await tree.getByRole("button", { name: /^All pages/ }).waitFor({ state: "visible", timeout: 15_000 });
  assert.equal(await tree.getByTestId("conventions-entry").count(), 0, "a viewer gets no Conventions entry");
  await shot(page, VIEWPORTS[0], "08-viewer-no-conventions");
  console.log("ok   viewer: no Conventions entry when none is written");
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
  }, 240_000);

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
    for (const ident of ["BAREA", "BAREB", "VIEW"]) {
      cli(config, db, ["project", "create", "--name", `Project ${ident}`, "--identifier", ident, "--json"]);
    }
    cli(config, db, ["member", "add", "--project", "VIEW", "--user", "mia", "--role", "viewer"]);

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
    // is empty, and a viewer project with no conventions page at all.
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
    await probe.close();

    await scenario(browser, base, VIEWPORTS[0], { bare: "BAREA", emptyPage: "EMPTA", created: "PICKA" }, templates, errors);
    await scenario(browser, base, VIEWPORTS[1], { bare: "BAREB", emptyPage: "EMPTB", created: "PICKB" }, templates, errors);
    await viewerScenario(browser, base, "VIEW", errors);

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

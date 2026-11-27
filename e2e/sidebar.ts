#!/usr/bin/env bun
// Real Layout, Settings, palette and MobileNav. Only HTTP is mocked; no copied
// sidebar state or handlers. Vite and Chromium are owned by this finite process.
// Run inside `devenv --profile e2e shell`: bun run sidebar. Screenshots:
// $E2E_SCREENSHOT_DIR or the OS temp directory.
// This checks the client's per-session ordering contract, not server isolation.
import { strict as assert } from "node:assert";
import { resolve } from "node:path";
import { tmpdir } from "node:os";
import { chromium, type BrowserContext, type Locator, type Page } from "playwright";
import { createServer } from "../web/node_modules/vite/dist/node/index.js";
import { accents, appearance, sidebarContrast, checkSidebarContrast, desktopScreenshots, mobileVisualChecks } from "./sidebar-visual";
import { withNativeLinkDiagnostics } from "./native-links";

const root = resolve(import.meta.dir, "../web");
const fixtureId = root + "/src/SidebarFixture.svelte";
const fixture = `<script>
  import Layout from './lib/Layout.svelte';
  import Settings from './routes/Settings.svelte';
  import { init } from './lib/theme';
  import { commandPaletteState } from './lib/commandPaletteState.svelte';
  import { mobileNavState } from './lib/mobileNavState.svelte';
  import Toaster from './lib/toast/Toaster.svelte';
  init();
  let route = $state(location.hash.slice(1) || '/');
  let refresh = $state();
  const navigate = path => { location.hash = path; route = path; };
  window.fixture = { navigate, refresh: () => refresh?.(), palette: () => commandPaletteState.open,
    mobile: () => mobileNavState.open };
</script>
<svelte:window onhashchange={() => route = location.hash.slice(1) || '/'} />
<svelte:boundary>
  <Layout {route} {navigate} bind:onProjectChange={refresh}>
    {#if route === '/settings'}<Settings {navigate} />
    {:else}<div class="p-8"><h1 id="route" class="text-heading">{route}</h1></div>{/if}
  </Layout>
  {#snippet failed(error)}<pre id="boundary-error">{error.message}</pre>{/snippet}
</svelte:boundary>
<Toaster />`;

const server = await createServer({
  root, server: { host: "127.0.0.1", port: 0, strictPort: false },
  plugins: [{
    name: "sidebar-fixture", enforce: "pre",
    resolveId(id) { if (id === "/src/SidebarFixture.svelte") return fixtureId; },
    load(id) { if (id === fixtureId) return fixture; },
    configureServer(server) {
      server.middlewares.use(async (req, res, next) => {
        if (req.url?.split("?")[0] !== "/") return next();
        res.setHeader("Content-Type", "text/html");
        res.end(await server.transformIndexHtml("/", `<div id="app"></div><script type="module">import {mount} from 'svelte'; import Fixture from '/src/SidebarFixture.svelte'; import '/src/app.css'; mount(Fixture,{target:document.getElementById('app')});</script>`));
      });
    },
  }],
});

const stamp = "2026-09-10T12:00:00Z";
const longName = "The extraordinarily long project name that must not hide its actions";
const longTitle = "Investigate the extraordinarily long sidebar regression title with enough detail to wrap across several lines";
const destinations = ["Overview", "Issues", "Board", "Modules", "Pages", "Plans", "Graph", "Files", "Activity", "Insights"];
type Call = { method: string; path: string; query: string; body: any };
type Gate = { method: string; path: string; status: number; release: () => void; promise: Promise<void> };
type Item = { kind: "folder" | "project"; id: number };
const project = (id: number): Item => ({ kind: "project", id });
const folder = (id: number): Item => ({ kind: "folder", id });
const same = (a: Item | null, b: Item | null) => !!a && !!b && a.kind === b.kind && a.id === b.id;
class API {
  user = { id: 1, username: "sidebar", display_name: "Sidebar Operator", email: "sidebar@example.test", is_admin: false };
  projects = ["One", "Two", longName, "Four"].map((name, i) => ({
    id: i + 1, name, identifier: ["ONE", "TWO", "LONG", "FOUR"][i], emoji: "", description: "",
    sort_order: i, created_at: stamp, updated_at: stamp, is_public: false,
  }));
  // The sidebar as the server stores it: each container's items in order.
  // `null` is the top level. Mirrors src/db/queries/project_groups.rs closely
  // enough that a move the client should have refused fails the test.
  folders = new Map<number, { name: string; items: Item[] }>([
    [11, { name: "Work", items: [project(1), project(2)] }],
    [12, { name: "Personal", items: [project(3)] }],
  ]);
  root: Item[] = [folder(11), folder(12), project(4)];
  nextFolder = 13;
  seenMoves = 0;
  calls: Call[] = [];
  unexpected: string[] = [];
  expectedConsole = new Set<string>();
  failingGets = new Set<string>();
  gates: Gate[] = [];
  pending = 0;
  hold(method: string, path: string, status = 200) {
    let release!: () => void;
    const promise = new Promise<void>(r => release = r);
    const gate = { method, path, status, release, promise };
    this.gates.push(gate);
    return gate;
  }
  fail(method: string, path: string) { const gate = this.hold(method, path, 409); gate.release(); }
  /** Replace the tree: `spec` nests as [name, id, children] for a folder, a
   *  number for a project. */
  arrange(spec: (number | [string, number, any[]])[]) {
    this.folders.clear();
    const build = (list: (number | [string, number, any[]])[]): Item[] => list.map(entry => {
      if (typeof entry === "number") return project(entry);
      const [name, id, children] = entry;
      this.folders.set(id, { name, items: build(children) });
      this.nextFolder = Math.max(this.nextFolder, id + 1);
      return folder(id);
    });
    this.root = build(spec);
  }
  /** The tree as `Work[One,Two],Four`, the same shape `domTree` reads. */
  describe(items: Item[] = this.root): string {
    return items.map(i => i.kind === "project" ? this.projects.find(p => p.id === i.id)!.name
      : `${this.folders.get(i.id)!.name}[${this.describe(this.folders.get(i.id)!.items)}]`).join(",");
  }
  list(parent: number | null) { return parent === null ? this.root : this.folders.get(parent)!.items; }
  parentOf(item: Item): number | null | undefined {
    if (this.root.some(i => same(i, item))) return null;
    for (const [id, f] of this.folders) if (f.items.some(i => same(i, item))) return id;
    return undefined;
  }
  depth(parent: number | null): number {
    let n = 0;
    for (let at = parent; at !== null && at !== undefined; at = this.parentOf(folder(at))!) n++;
    return n;
  }
  height(id: number): number {
    return 1 + Math.max(0, ...this.folders.get(id)!.items.filter(i => i.kind === "folder").map(i => this.height(i.id)));
  }
  /** Folder names top-down to `item`, for assertions. */
  path(item: Item): string[] {
    const out: string[] = [];
    for (let at = this.parentOf(item); at !== null && at !== undefined; at = this.parentOf(folder(at))) out.unshift(this.folders.get(at)!.name);
    return out;
  }
  place(item: Item, parent: number | null, before: Item | null) {
    const from = this.parentOf(item);
    if (from !== undefined) { const list = this.list(from); list.splice(list.findIndex(i => same(i, item)), 1); }
    const list = this.list(parent);
    const at = before ? list.findIndex(i => same(i, before)) : -1;
    list.splice(at >= 0 ? at : list.length, 0, item);
  }
  group(id: number, parent: number | null, depth: number) {
    const f = this.folders.get(id)!;
    return { id, user_id: this.user.id, name: f.name, parent_id: parent, depth, sort_order: 0, items: [...f.items],
      project_ids: f.items.filter(i => i.kind === "project").map(i => i.id), created_at: stamp, updated_at: stamp };
  }
  layout() {
    const groups: ReturnType<API["group"]>[] = [];
    const walk = (items: Item[], parent: number | null, depth: number) => {
      for (const i of items) if (i.kind === "folder") { groups.push(this.group(i.id, parent, depth)); walk(this.folders.get(i.id)!.items, i.id, depth + 1); }
    };
    walk(this.root, null, 1);
    return { root: [...this.root], groups, max_depth: 4 };
  }
  /** The server's refusals. The client checks first, so reaching one is a bug. */
  refusal(item: Item, parent: number | null): string | null {
    if (parent !== null && !this.folders.has(parent)) return "No such folder.";
    if (item.kind !== "folder") return null;
    for (let at: number | null | undefined = parent; at !== null && at !== undefined; at = this.parentOf(folder(at)))
      if (at === item.id) return "cycle";
    return this.depth(parent) + this.height(item.id) > 4 ? "depth" : null;
  }
  async attach(context: BrowserContext) {
    await context.route("**/api/**", async route => {
      const req = route.request();
      const url = new URL(req.url());
      const path = url.pathname.slice(4);
      const method = req.method();
      const body = req.postData() ? req.postDataJSON() : null;
      this.calls.push({ method, path, query: url.search, body });
      this.pending++;
      try {
        // A bounded fixture fails rather than feeding an asynchronous effect loop forever.
        if (this.calls.length > 250) {
          if (!this.unexpected.includes("Runaway API requests")) this.unexpected.push("Runaway API requests");
          return await route.abort();
        }
        const index = this.gates.findIndex(g => g.method === method && g.path === path);
        if (index >= 0) {
          const gate = this.gates.splice(index, 1)[0];
          await gate.promise;
          if (gate.status !== 200) {
            this.expectedConsole.add(req.url());
            return await route.fulfill({ status: gate.status, json: { error: "Fixture rejected this save." } });
          }
        }
        if (method === "GET" && this.failingGets.has(path)) {
          this.expectedConsole.add(req.url());
          return await route.fulfill({ status: 409, json: { error: "Fixture read temporarily unavailable." } });
        }
        let data: unknown;
        if (path === "/auth/me" && ["GET", "PATCH"].includes(method)) {
          if (method === "PATCH") Object.assign(this.user, body);
          data = this.user;
        } else if (path === "/projects" && method === "GET") data = this.projects;
        else if (path === "/project-groups/layout" && method === "GET") data = this.layout();
        else if (path === "/project-groups" && method === "GET") data = this.layout().groups;
        else if (path === "/project-groups/move" && method === "PUT") {
          const refused = this.refusal(body.item, body.parent_id);
          if (refused) {
            this.unexpected.push(`Client sent a refused move (${refused}): ${JSON.stringify(body)}`);
            return await route.fulfill({ status: 400, json: { error: refused } });
          }
          this.place(body.item, body.parent_id, body.before);
          data = this.layout();
        } else if (path === "/project-groups" && method === "POST") {
          const id = this.nextFolder++;
          this.folders.set(id, { name: body.name, items: [] });
          const list = this.list(body.parent_id ?? null);
          const lastFolder = list.findLastIndex(i => i.kind === "folder");
          list.splice(lastFolder + 1, 0, folder(id));
          if (body.contains) this.place(body.contains, id, null);
          data = this.group(id, body.parent_id ?? null, this.depth(body.parent_id ?? null) + 1);
        } else if (path === "/project-groups/assign" && method === "PUT") {
          this.place(project(body.project_id), body.group_id, null);
          data = { ok: true };
        } else if (/^\/project-groups\/\d+$/.test(path) && method === "PATCH") {
          const id = Number(path.split("/").at(-1));
          this.folders.get(id)!.name = body.name;
          data = this.group(id, this.parentOf(folder(id)) ?? null, this.depth(this.parentOf(folder(id)) ?? null) + 1);
        } else if (/^\/project-groups\/\d+$/.test(path) && method === "DELETE") {
          // Contents take the folder's place in its parent; nothing is lost.
          const id = Number(path.split("/").at(-1));
          const list = this.list(this.parentOf(folder(id))!);
          list.splice(list.findIndex(i => same(i, folder(id))), 1, ...this.folders.get(id)!.items);
          this.folders.delete(id);
          data = { deleted: true };
        } else if (/^\/projects\/\d+\/my-role$/.test(path) && method === "GET") {
          data = { role: "owner", is_admin: false };
        } else if (["/issues", "/modules", "/pages", "/plans"].includes(path) && method === "GET") {
          const projectId = Number(url.searchParams.get("project_id"));
          const project = this.projects.find(p => p.id === projectId);
          data = !project || (path === "/pages" && url.searchParams.get("status") !== "active") ? [] : [{
            id: projectId * 10, project_id: projectId, identifier: `${project.identifier}-1`,
            title: projectId === 1 ? longTitle : "Other project item", name: "Recent module",
            status: "active", updated_at: stamp, created_at: stamp,
          }];
        } else if (path === "/instance" && method === "GET") data = { web_auto_login: false, auth_mode: "passwords" };
        else if (["/auth/bots", "/folders"].includes(path) && method === "GET") data = [];
        else if (path === "/issues/attention" && method === "GET") {
          const issue = (id: number) => ({ id, project_id: 1, identifier: `ONE-${id}`, title: "Needs you", status: "todo", priority: "medium", updated_at: stamp, created_at: stamp, labels: [] });
          data = { assigned: [issue(1)], human: [issue(2)], waiting: [] };
        } else if (path === "/agents/activity" && method === "GET") {
          data = { window: "1h", active_count: 3, buckets: [], agents: [], recent: [] };
        }
        else {
          this.unexpected.push(`${method} ${path}${url.search}`);
          return await route.fulfill({ status: 501, json: { error: "Unmocked API request" } });
        }
        await route.fulfill({ json: data });
      } catch (error) {
        if (!context.pages().every(p => p.isClosed())) this.unexpected.push(String(error));
      } finally { this.pending--; }
    });
  }
}

const deadline = setTimeout(() => { console.error("Sidebar suite deadline exceeded"); process.exit(1); }, 180_000);
const failures: string[] = [];
let browser: Awaited<ReturnType<typeof chromium.launch>>;
let base: string;
let passed = 0;
const shotDir = process.env.E2E_SCREENSHOT_DIR ?? tmpdir();
const allGates: Gate[] = [];
type Session = { page: Page; api: API; aside: Locator; context: BrowserContext };

async function session(options: { route?: string; mobile?: boolean; collapsed?: number[]; api?: API; observeReveal?: boolean } = {}): Promise<Session> {
  const api = options.api ?? new API();
  const context = await browser.newContext({ viewport: options.mobile ? { width: 390, height: 844 } : { width: 1280, height: 1000 }, reducedMotion: "reduce", colorScheme: "light" });
  await api.attach(context);
  if (options.observeReveal) await context.addInitScript(() => {
    (window as any).sidebarScrollCalls = [];
    const original = Element.prototype.scrollIntoView;
    Element.prototype.scrollIntoView = function(options) {
      const id = this.getAttribute("data-sidebar-project");
      if (id) (window as any).sidebarScrollCalls.push({ id, options });
      // Observe the real browser scroll, never replace it with a no-op.
      return original.call(this, options);
    };
  });
  if (options.collapsed) await context.addInitScript(ids => {
    if (location.protocol !== "http:") return;
    if (localStorage.getItem("lific:sidebar:collapsed-groups") === null)
      localStorage.setItem("lific:sidebar:collapsed-groups", JSON.stringify(ids));
  }, options.collapsed);
  const errors: string[] = [];
  context.on("page", page => {
    page.setDefaultTimeout(5_000);
    page.on("pageerror", error => errors.push(error.message));
    page.on("console", message => {
      if (message.type() !== "error") return;
      // Chromium emits a console resource error for the deliberately failed HTTP
      // responses. Never suppress application errors or unrelated failed resources.
      if (api.expectedConsole.has(message.location().url) && /^Failed to load resource: the server responded with a status of 409\b/.test(message.text())) return;
      errors.push(message.text());
    });
  });
  const page = await context.newPage();
  sessions.push({ page, api, aside: page.locator("aside"), context, errors });
  await page.goto(base + "#" + (options.route ?? "/"));
  await page.locator('aside a[title="Account settings"]').waitFor({ state: "attached" });
  await healthy(page, api);
  return { page, api, aside: page.locator("aside"), context };
}
const sessions: (Session & { errors: string[] })[] = [];
async function healthy(page: Page, api: API) {
  assert.equal(await page.locator("#boundary-error").count(), 0, "Svelte boundary rendered");
  assert.deepEqual(api.unexpected, [], "Unexpected or runaway HTTP requests");
}
async function settle(s: Session) {
  await s.page.waitForFunction(() => !!(window as any).fixture);
  // An idle observation window detects async effect loops that evade the boundary.
  await s.page.waitForTimeout(150);
  const calls = s.api.calls.length;
  await s.page.waitForTimeout(150);
  assert.equal(s.api.calls.length, calls, "HTTP requests continued after the UI settled");
  assert.equal(s.api.pending, 0, "Requests never completed");
  await healthy(s.page, s.api);
}
async function routeTo(page: Page, path: string) {
  await page.evaluate(path => (window as any).fixture.navigate(path), path);
  await page.waitForFunction(path => location.hash === "#" + path, path);
}
async function attr(el: Locator, name: string, value: string | null) {
  await el.evaluate((el, args) => new Promise<void>((resolve, reject) => {
    const end = performance.now() + 4000;
    function check() {
      if (el.getAttribute(args.name) === args.value) resolve();
      else if (performance.now() > end) reject(new Error(`${args.name}: expected ${args.value}, got ${el.getAttribute(args.name)}`));
      else requestAnimationFrame(check);
    }
    check();
  }), { name, value });
}
const menuItem = (page: Page, name: string) => page.getByRole("menuitem", { name, exact: true });

// The rendered sidebar tree as `Work[One,Two],Four`, read from the DOM.
function domTree(page: Page): Promise<string> {
  return page.evaluate(() => {
    const walk = (el: Element | null): string[] => el ? [...el.children].flatMap(c => {
      if (c.matches("[data-drop-entry]")) return [c.querySelector("a[data-sidebar-project]")!.getAttribute("title")!];
      if (c.matches(".sidebar-group-heading")) {
        const id = (c as HTMLElement).dataset.dropId;
        return [`${c.querySelector("button[title]")!.getAttribute("title")}[${walk(document.getElementById(`group-${id}`)).join(",")}]`];
      }
      return [];
    }) : [];
    return walk(document.querySelector("aside [data-tree-root]")).join(",");
  });
}
const row = (s: Session, kind: "project" | "folder", id: number) =>
  s.aside.locator(`[data-drop-row][data-drop-kind="${kind}"][data-drop-id="${id}"]`);
type Where = "before" | "after" | "inside" | "below";
async function pointAt(target: Locator, where: Where) {
  const b = (await target.boundingBox())!;
  const y = where === "before" ? b.y + 2 : where === "after" ? b.y + b.height - 2 : where === "below" ? b.y + b.height + 24 : b.y + b.height / 2;
  return { x: b.x + Math.min(b.width / 2, 80), y };
}
/** Press on `source` and move past the drag threshold, with a real mouse. */
async function grab(page: Page, source: Locator) {
  await source.scrollIntoViewIfNeeded();
  const { x, y } = await pointAt(source, "inside");
  await page.mouse.move(x, y); await page.mouse.down();
  await page.mouse.move(x + 3, y + 7, { steps: 3 });
  await page.locator("[data-sidebar-drag-ghost]").waitFor();
}
async function hoverAt(page: Page, target: Locator, where: Where) {
  const { x, y } = await pointAt(target, where);
  await page.mouse.move(x, y, { steps: 6 });
}
async function dragTo(page: Page, source: Locator, target: Locator, where: Where) {
  await grab(page, source); await hoverAt(page, target, where); await page.mouse.up();
  await page.locator("[data-sidebar-drag-ghost]").waitFor({ state: "detached" });
}
const moves = (api: API) => api.calls.filter(c => c.method === "PUT" && c.path === "/project-groups/move").map(c => c.body);
/** The next move the client sends, once it arrives. */
async function moved(api: API) {
  return (await called(api, ++api.seenMoves, "PUT", "/project-groups/move")).body;
}
async function action(s: Session, name: string, choice: string) {
  const trigger = s.aside.getByRole("button", { name, exact: true });
  await trigger.focus(); await trigger.click();
  await menuItem(s.page, choice).click();
}
function held(api: API, method: string, path: string, status = 200) {
  const gate = api.hold(method, path, status); allGates.push(gate); return gate;
}
async function called(api: API, count: number, method: string, path: string) {
  for (let i = 0; i < 100; i++) {
    const calls = api.calls.filter(c => c.method === method && c.path === path);
    if (calls.length >= count) return calls.at(-1)!;
    await Bun.sleep(20);
  }
  throw new Error(`Missing request ${method} ${path} (#${count})`);
}
async function test(name: string, run: () => Promise<void>) {
  const start = sessions.length;
  try {
    await run();
    for (const s of sessions.slice(start)) {
      await settle(s);
      assert.deepEqual(s.errors, [], "Browser runtime/console errors");
    }
    passed++; console.log(`PASS ${name}`);
  } catch (error) {
    failures.push(`${name}: ${error}`);
    console.error(`FAIL ${name}:`, error);
    for (const s of sessions.slice(start)) {
      console.error("Diagnostics:", { errors: s.errors, unexpected: s.api.unexpected, requests: s.api.calls.length,
        boundary: await s.page.locator("#boundary-error").textContent({ timeout: 200 }).catch(() => null) });
      await s.page.screenshot({ path: resolve(shotDir, `sidebar-failed-${name.replace(/[^a-z0-9]+/gi, "-")}.png`) }).catch(() => {});
    }
  } finally {
    for (const gate of allGates.splice(0)) gate.release();
    for (const s of sessions.slice(start)) await s.context.close();
  }
}

try {
  await server.listen(); base = server.resolvedUrls!.local[0];
  const executablePath = process.env.PLAYWRIGHT_EXECUTABLE_PATH;
  browser = await chromium.launch(
    executablePath
      ? { headless: true, executablePath }
      : { headless: true, channel: "chromium" },
  );

  await test("separate disclosure and Overview links", async () => {
    const s = await session(); const { page, aside } = s;
    await aside.getByRole("button", { name: "Expand One", exact: true }).click();
    assert.equal(new URL(page.url()).hash, "#/");
    await aside.getByRole("button", { name: "Expand Two", exact: true }).click();
    await aside.locator('a[title="One"]').click();
    await page.waitForURL("**/#/ONE/overview");
    await attr(aside.locator('a[title="One"]'), "aria-current", "page");
    await attr(aside.locator("#project-nav-1"), "hidden", null);
    await attr(aside.locator("#project-nav-2"), "hidden", null);
    await aside.getByRole("button", { name: "Collapse One", exact: true }).click();
    assert.equal(new URL(page.url()).hash, "#/ONE/overview");
    await attr(aside.locator("#project-nav-1"), "hidden", "");
    await attr(aside.locator("#project-nav-2"), "hidden", null);
  });

  await test("sidebar selection and readable accents in both themes", async () => {
    const s = await session({ route: "/ONE/issues/ONE-1" });
    await s.aside.getByRole("button", { name: "Recent issues", exact: true }).click();
    await checkSidebarContrast(s.page, shotDir);
  });

  await test("selected account metadata remains readable across accents", async () => {
    const s = await session({ route: "/settings" });
    const measurements = [];
    for (const theme of ["light", "dark"]) for (const accent of accents) {
      await appearance(s.page, theme, accent);
      const result = { theme, accent, ...await sidebarContrast(s.page) };
      measurements.push(result);
      assert.ok(result.minimumText >= 4.5, JSON.stringify(result));
    }
    await Bun.write(resolve(shotDir, "sidebar-account-contrast.json"), JSON.stringify(measurements, null, 2));
  });

  if (process.env.E2E_VISUAL === "1") await test("desktop visual matrix", async () => {
    const api = new API();
    api.projects[0].emoji = "🦎";
    api.projects[1].emoji = "lucide:Terminal";
    const s = await session({ api, route: "/ONE/issues" });
    await desktopScreenshots(s.page, shotDir);
  });

  await test("mobile visual grammar contrast and touch targets", async () => {
    const api = new API();
    api.projects[0].emoji = "🦎";
    api.projects[1].emoji = "lucide:Terminal";
    const s = await session({ api, mobile: true, route: "/ONE/issues" });
    await mobileVisualChecks(s.page, shotDir, process.env.E2E_VISUAL === "1");
  });

  await test("direct link reveal and deliberate collapse survives revalidation", async () => {
    const s = await session({ route: "/ONE/issues/ONE-1", collapsed: [11, 12] });
    const { page, aside } = s;
    await attr(aside.locator("#group-11"), "hidden", null);
    await attr(aside.locator("#group-12"), "hidden", "");
    await attr(aside.locator("#project-nav-1"), "hidden", null);
    await aside.getByRole("button", { name: "Collapse One", exact: true }).click();
    await aside.getByRole("button", { name: "Work", exact: true }).click();
    await routeTo(page, "/ONE/pages");
    await page.evaluate(() => (window as any).fixture.refresh());
    await settle(s);
    await attr(aside.locator("#group-11"), "hidden", "");
    await attr(aside.locator("#project-nav-1"), "hidden", "");
    assert.ok(await aside.getByRole("button", { name: "Current: One", exact: true }).isVisible());
    assert.ok((await page.evaluate(() => JSON.parse(localStorage.getItem("lific:sidebar:collapsed-groups")!))).includes(11));
    // A document reload is a new direct entry, unlike data revalidation.
    await page.reload();
    await attr(aside.locator("#group-11"), "hidden", null);
    await attr(aside.locator("#project-nav-1"), "hidden", null);
  });

  await test("failed initial groups recover and reveal the active project exactly once", async () => {
    const api = new API();
    // Every startup read fails until explicitly recovered. Layout currently has
    // multiple initial refresh callers; do not depend on their count or order.
    api.failingGets.add("/project-groups/layout");
    const s = await session({ api, route: "/ONE/issues/ONE-1", collapsed: [11, 12], observeReveal: true });
    const { page, aside } = s;
    const scrollCalls = () => page.evaluate(() => (window as any).sidebarScrollCalls);
    await settle(s);
    assert.ok(await aside.locator('a[title="One"]').isVisible(), "Projects loaded despite failed groups");
    assert.equal(await aside.locator("#group-11").count(), 0);
    await attr(aside.locator("#project-nav-1"), "hidden", "");
    assert.deepEqual(await scrollCalls(), [], "Failed membership read must not consume route reveal");
    assert.deepEqual(await page.evaluate(() => JSON.parse(localStorage.getItem("lific:sidebar:collapsed-groups")!)), [11, 12]);

    api.failingGets.delete("/project-groups/layout");
    const gate = held(api, "GET", "/project-groups/layout");
    const count = api.calls.filter(c => c.path === "/project-groups/layout").length;
    const recovering = page.evaluate(() => (window as any).fixture.refresh());
    await called(api, count + 1, "GET", "/project-groups/layout");
    assert.deepEqual(await scrollCalls(), [], "Wait for recovered membership, not just project success");
    gate.release(); await recovering;
    await attr(aside.locator("#group-11"), "hidden", null);
    await attr(aside.locator("#project-nav-1"), "hidden", null);
    await attr(aside.locator("#group-12"), "hidden", "");
    await settle(s);
    assert.deepEqual(await scrollCalls(), [{ id: "1", options: { block: "nearest" } }]);
    assert.deepEqual(await page.evaluate(() => JSON.parse(localStorage.getItem("lific:sidebar:collapsed-groups")!)), [12]);

    await aside.getByRole("button", { name: "Collapse One", exact: true }).click();
    await aside.getByRole("button", { name: "Work", exact: true }).click();
    for (const path of ["/ONE/pages", "/ONE/board"]) {
      await routeTo(page, path);
      await page.evaluate(() => (window as any).fixture.refresh());
      await settle(s);
      await attr(aside.locator("#group-11"), "hidden", "");
      await attr(aside.locator("#project-nav-1"), "hidden", "");
      assert.deepEqual(await scrollCalls(), [{ id: "1", options: { block: "nearest" } }], "Revalidation must not reveal twice");
    }
    assert.ok(await aside.getByRole("button", { name: "Current: One", exact: true }).isVisible());
    assert.ok((await page.evaluate(() => JSON.parse(localStorage.getItem("lific:sidebar:collapsed-groups")!))).includes(11));
  });

  await test("later failed group snapshot defers newly visible project reveal until recovery", async () => {
    const s = await session({ observeReveal: true });
    const { page, aside, api } = s;
    const scrollCalls = () => page.evaluate(() => (window as any).sidebarScrollCalls);
    const group = aside.locator("#group-11");
    const projectNav = aside.locator("#project-nav-5");
    const revealed = [{ id: "5", options: { block: "nearest" } }];
    await settle(s);
    // Establish an earlier successful membership snapshot, then collapse through
    // the real UI. This is deliberately not the initial-load failure case above.
    await attr(group, "hidden", null);
    await aside.getByRole("button", { name: "Work", exact: true }).click();
    await attr(group, "hidden", "");
    assert.ok((await page.evaluate(() => JSON.parse(localStorage.getItem("lific:sidebar:collapsed-groups")!))).includes(11));

    api.projects.push({ ...api.projects[0], id: 5, identifier: "FIVE", name: "Five", sort_order: 4 });
    api.folders.get(11)!.items.push(project(5));
    api.failingGets.add("/project-groups/layout");
    await routeTo(page, "/FIVE/issues");
    await settle(s);
    assert.ok(await aside.locator('a[title="Five"]').isVisible(), "New project arrived in the successful project response");
    assert.equal(await group.locator('a[title="Five"]').count(), 0, "Failed group response leaves the older membership snapshot in place");
    await attr(group, "hidden", "");
    await attr(projectNav, "hidden", "");
    assert.deepEqual(await scrollCalls(), [], "An older successful group load must not permit premature reveal");

    api.failingGets.delete("/project-groups/layout");
    const gate = held(api, "GET", "/project-groups/layout");
    const count = api.calls.filter(c => c.path === "/project-groups/layout").length;
    const recovering = page.evaluate(() => (window as any).fixture.refresh());
    await called(api, count + 1, "GET", "/project-groups/layout");
    assert.deepEqual(await scrollCalls(), [], "Recovery must wait for the current membership response");
    gate.release(); await recovering;
    await settle(s);
    await attr(group, "hidden", null);
    await attr(projectNav, "hidden", null);
    assert.ok(await group.locator('a[title="Five"]').isVisible());
    assert.deepEqual(await scrollCalls(), revealed);

    // Both disclosure states must remain stable across fresh successful snapshots.
    // First keep the automatic reveal open, then respect a deliberate collapse.
    for (const collapsed of [false, true, false]) {
      if (collapsed) {
        await aside.getByRole("button", { name: "Collapse Five", exact: true }).click();
        await aside.getByRole("button", { name: "Work", exact: true }).click();
      } else if (await group.getAttribute("hidden") !== null) {
        await aside.getByRole("button", { name: "Work", exact: true }).click();
        await aside.getByRole("button", { name: "Expand Five", exact: true }).click();
      }
      await routeTo(page, collapsed ? "/FIVE/pages" : "/FIVE/board");
      await page.evaluate(() => (window as any).fixture.refresh());
      await settle(s);
      await attr(group, "hidden", collapsed ? "" : null);
      await attr(projectNav, "hidden", collapsed ? "" : null);
      assert.equal((await page.evaluate(() => JSON.parse(localStorage.getItem("lific:sidebar:collapsed-groups")!))).includes(11), collapsed);
      assert.deepEqual(await scrollCalls(), revealed, "Later snapshots neither re-reveal nor override deliberate disclosure choices");
    }
  });

  await test("long sidebar reveals nearest once and respects subsequent user scrolling", async () => {
    const api = new API();
    const template = api.projects[0];
    api.arrange([]);
    api.projects = Array.from({ length: 45 }, (_, i) => ({
      ...template, id: i + 1, identifier: `P${i + 1}`, name: `Project ${i + 1}`, sort_order: i,
    }));
    const s = await session({ api, observeReveal: true });
    const { page, aside } = s;
    const nav = aside.locator(".sidebar-scroll");
    const target = aside.locator('[data-sidebar-project="45"]');
    await settle(s);
    const navBox = (await nav.boundingBox())!;
    assert.ok((await target.boundingBox())!.y > navBox.y + navBox.height, "Target begins below the visible sidebar");
    assert.equal(await nav.evaluate(el => el.scrollTop), 0);
    await routeTo(page, "/P45/issues");
    await settle(s);
    const targetBox = (await target.boundingBox())!;
    assert.ok(targetBox.y >= navBox.y && targetBox.y + targetBox.height <= navBox.y + navBox.height + 1, "Route entry scrolls target into view");
    assert.ok(Math.abs(targetBox.y + targetBox.height - navBox.y - navBox.height) <= 2, "Nearest scroll aligns the offscreen row to the lower edge");
    assert.deepEqual(await page.evaluate(() => (window as any).sidebarScrollCalls), [{ id: "45", options: { block: "nearest" } }]);

    // Actual wheel input, not a state setter. No locator clicks below that would
    // implicitly scroll the active project back into view on the test's behalf.
    await page.mouse.move(navBox.x + navBox.width / 2, navBox.y + navBox.height / 2);
    await page.mouse.wheel(0, -10_000);
    await page.waitForFunction(() => document.querySelector("aside .sidebar-scroll")!.scrollTop === 0);
    for (const path of ["/P45/issues/P45-1", "/P45/pages"]) {
      await routeTo(page, path);
      await page.evaluate(() => (window as any).fixture.refresh());
      await settle(s);
      assert.equal(await nav.evaluate(el => el.scrollTop), 0, "Within-project navigation must not fight manual scrolling");
      assert.deepEqual(await page.evaluate(() => (window as any).sidebarScrollCalls), [{ id: "45", options: { block: "nearest" } }]);
    }
    await routeTo(page, "/P44/issues");
    await settle(s);
    assert.ok(await nav.evaluate(el => el.scrollTop > 0), "Entering another project reveals it again");
    assert.deepEqual(await page.evaluate(() => (window as any).sidebarScrollCalls), [
      { id: "45", options: { block: "nearest" } }, { id: "44", options: { block: "nearest" } },
    ]);
  });

  await test("stable destinations and cached recents across slow failed refreshes", async () => {
    const s = await session({ route: "/ONE/issues" }); const { page, aside, api } = s;
    const nav = aside.locator("#project-nav-1");
    assert.deepEqual(await nav.locator(":scope > a").allTextContents().then(rows => rows.map(s => s.trim())), destinations);
    const boardY = (await nav.getByRole("link", { name: "Board", exact: true }).boundingBox())!.y;
    for (const section of ["issues", "modules", "pages", "plans"]) {
      await routeTo(page, `/ONE/${section}`);
      const toggle = nav.getByRole("button", { name: `Recent ${section}`, exact: true });
      if (await toggle.getAttribute("aria-expanded") !== "true") await toggle.click();
      const recent = nav.locator("#recent-1");
      await attr(recent, "aria-busy", "false");
      assert.equal(await recent.locator("a").count(), 1);
      const before = await recent.locator("a").allTextContents();
      assert.ok((await toggle.boundingBox())!.y > (await nav.getByRole("link", { name: "Insights", exact: true }).boundingBox())!.y);
      assert.equal((await nav.getByRole("link", { name: "Board", exact: true }).boundingBox())!.y, boardY);
      const gate = held(api, "GET", `/${section}`, 409);
      await routeTo(page, `/ONE/${section}/${section === "issues" ? "ONE-1" : "10"}`);
      await attr(recent, "aria-busy", "true");
      assert.deepEqual(await recent.locator("a").allTextContents(), before);
      gate.release();
      await attr(recent, "aria-busy", "false");
      assert.deepEqual(await recent.locator("a").allTextContents(), before);
      assert.equal((await nav.getByRole("link", { name: "Board", exact: true }).boundingBox())!.y, boardY);
    }
    const gate = held(api, "GET", "/issues");
    await routeTo(page, "/TWO/issues");
    const recent = aside.locator("#recent-2");
    await attr(recent, "aria-busy", "true");
    assert.equal(await recent.locator("a").count(), 0, "No previous-project cache leakage");
    gate.release(); await attr(recent, "aria-busy", "false");
    assert.match(await recent.innerText(), /Other project item/);
  });

  await test("long titles focus hover and 230 190 pixel widths", async () => {
    const s = await session({ route: "/ONE/issues" }); const { page, aside } = s;
    await aside.getByRole("button", { name: "Recent issues", exact: true }).click();
    const recent = aside.locator('.recent-link[href="#/ONE/issues/ONE-1"]');
    await recent.waitFor();
    for (const width of [230, 190]) {
      const separator = aside.getByRole("separator", { name: "Resize sidebar" });
      await separator.focus();
      while (Number(await separator.getAttribute("aria-valuenow")) > width) await page.keyboard.press("ArrowLeft");
      assert.equal((await aside.boundingBox())!.width, width);
      await recent.hover();
      assert.equal(await recent.getAttribute("title"), `ONE-1: ${longTitle}`);
      await page.keyboard.press("Tab"); await recent.focus();
      assert.equal(await recent.evaluate(el => el.matches(":focus-visible")), true);
      const tip = recent.locator(".focus-title");
      assert.equal(await tip.isVisible(), true);
      assert.equal(await tip.textContent(), longTitle);
      const box = (await tip.boundingBox())!;
      assert.ok(box.x >= 0 && box.x + box.width <= width, "Focus title stays inside sidebar");
      assert.ok(await tip.evaluate(el => el.scrollHeight <= el.clientHeight && el.scrollWidth <= el.clientWidth), "Focus title wraps without clipping");
      await page.screenshot({ path: resolve(shotDir, width === 230 ? "sidebar-desktop-default.png" : "sidebar-desktop-narrow.png") });
      await separator.focus();
      const project = aside.locator('a[title]').filter({ hasText: longName });
      await project.hover();
      assert.equal(await project.getAttribute("title"), longName);
      const actions = aside.getByRole("button", { name: `Actions for ${longName}`, exact: true });
      await actions.focus();
      assert.equal(await actions.evaluate(el => getComputedStyle(el).opacity), "1");
      const actionBox = (await actions.boundingBox())!;
      assert.ok(actionBox.x + actionBox.width <= width, "Long project preserves visible actions");
      assert.equal(await aside.locator(".sidebar-scroll").evaluate(el => el.scrollWidth <= el.clientWidth), true, "No horizontal sidebar scroll");
    }
    assert.equal(await page.evaluate(() => localStorage.getItem("lific:sidebar:width")), "190");
    await page.reload();
    await attr(aside.getByRole("separator", { name: "Resize sidebar" }), "aria-valuenow", "190");
  });

  await test("focused recent tooltip does not steal another project click", async () => {
    const s = await session({ route: "/ONE/issues" }); const { page, aside } = s;
    await aside.getByRole("button", { name: "Recent issues", exact: true }).click();
    const recent = aside.locator(".recent-link").first();
    await page.keyboard.press("Tab"); await recent.focus();
    await recent.locator(".focus-title").waitFor();
    const project = aside.locator('a[title]').filter({ hasText: longName });
    // Do not blur or force-click: that would conceal the pointer interception.
    await project.click();
    await page.waitForURL("**/#/LONG/overview");
  });

  await test("text-scaled default preserves pixel preference and resize semantics", async () => {
    const { page, aside } = await session({ route: "/settings" });
    const handle = aside.getByRole("separator", { name: "Resize sidebar" });
    const stored = () => page.evaluate(() => localStorage.getItem("lific:sidebar:width"));
    const width = async (expected: number) => {
      await attr(handle, "aria-valuenow", String(expected));
      assert.ok(Math.abs((await aside.boundingBox())!.width - expected) < 0.02, "ARIA reports the rendered CSS-pixel width");
    };
    const scale = async (name: string) => { await page.getByRole("button", { name, exact: true }).click(); };
    await width(230);
    await scale("L"); await width(258.75);
    await attr(handle, "aria-valuemin", "202.5");
    await scale("S"); await width(215.625);
    await scale("M"); await width(230);
    assert.equal(await stored(), null, "Text changes do not create a manual preference");
    await handle.click();
    assert.equal(await stored(), null, "Clicking the resize handle is not a resize");

    // Simulate a pre-upgrade preference, including a load already using Large.
    await scale("L");
    await page.evaluate(() => localStorage.setItem("lific:sidebar:width", "300"));
    await page.reload(); await width(300);
    await scale("S"); await width(300);
    await scale("M"); await width(300);
    assert.equal(await stored(), "300");

    await page.evaluate(() => localStorage.setItem("lific:sidebar:width", "190"));
    await page.reload(); await width(190);
    await scale("L"); await width(202.5);
    assert.equal(await stored(), "190", "Temporary minimum does not overwrite the saved width");
    await page.reload(); await width(202.5);
    await scale("M"); await width(190);
    await scale("L"); await width(202.5);
    await handle.focus(); await page.keyboard.press("ArrowRight"); await width(212.5);
    assert.equal(await stored(), "212.5", "Keyboard resize is still ten physical pixels");

    const box = (await handle.boundingBox())!;
    await page.mouse.move(box.x + box.width / 2, box.y + 100);
    await page.mouse.down();
    await page.mouse.move(box.x + box.width / 2 + 37, box.y + 100);
    await width(249.5);
    assert.equal(await stored(), "212.5", "Dragging does not persist until release");
    await page.mouse.up();
    assert.equal(await stored(), "249.5");
    await page.reload(); await width(249.5);
    await handle.dblclick(); await width(258.75);
    assert.equal(await stored(), null, "Reset resumes the proportional default");

    await aside.getByRole("button", { name: "Collapse sidebar", exact: true }).click();
    await scale("M");
    await page.getByRole("button", { name: "Expand sidebar", exact: true }).click();
    await width(230);
    assert.equal(await stored(), null);
  });

  await test("workspace counts and the collapsed icon rail", async () => {
    const s = await session({ route: "/ONE/issues" }); const { page, aside } = s;
    const inbox = aside.locator('a[data-workspace="inbox"]');
    await inbox.locator(".sidebar-count").filter({ hasText: "2" }).waitFor();
    assert.equal((await aside.locator('a[data-workspace="assigned"] .sidebar-count').innerText()).trim(), "1");
    await inbox.click(); await page.waitForURL("**/#/inbox");
    await attr(inbox, "aria-current", "page");

    await aside.getByRole("button", { name: "Collapse sidebar", exact: true }).click();
    assert.ok(Math.abs((await aside.boundingBox())!.width - 56) < 1, "Rail is 3.5rem wide");
    assert.equal(await aside.getByRole("separator", { name: "Resize sidebar" }).count(), 0, "No resize handle on the rail");
    await attr(aside.getByRole("link", { name: "Inbox, 2", exact: true }), "aria-current", "page");
    const assigned = aside.getByRole("link", { name: "Assigned to me", exact: true });
    await assigned.hover();
    await page.getByRole("tooltip").filter({ hasText: "Assigned to me" }).waitFor();
    await assigned.click(); await page.waitForURL("**/#/assigned");
    await page.getByRole("tooltip").waitFor({ state: "detached" });

    const one = aside.getByRole("button", { name: "One views", exact: true });
    await one.click();
    await menuItem(page, "Board").click();
    await page.waitForURL("**/#/ONE/board");
    await attr(one, "aria-current", "true");

    await page.reload();
    await aside.getByRole("button", { name: "Expand sidebar", exact: true }).waitFor();
    await page.keyboard.press("Control+\\");
    await aside.getByRole("button", { name: "Collapse sidebar", exact: true }).waitFor();
    assert.equal(await page.evaluate(() => localStorage.getItem("lific:sidebar:collapsed")), "0");
  });

  await test("a long project list scrolls the rail instead of squashing it", async () => {
    const api = new API();
    for (let i = 5; i <= 30; i++) api.projects.push({ ...api.projects[3], id: i, name: `Project ${i}`, identifier: `P${i}`, sort_order: i });
    const { page, aside } = await session({ api, route: "/inbox" });
    await page.evaluate(() => { localStorage.setItem("lific:sidebar:collapsed", "1"); });
    await page.reload();
    await aside.getByRole("button", { name: "Project 30 views", exact: true }).waitFor({ state: "attached" });
    const heights = await aside.locator(".rail-item").evaluateAll(els => els.map(el => el.getBoundingClientRect().height));
    assert.ok(heights.length > 30 && heights.every(h => Math.abs(h - 40) < 0.5), JSON.stringify(heights));
    assert.ok(await aside.locator(".rail-nav").evaluate(el => el.scrollHeight > el.clientHeight), "Rail nav scrolls");
  });

  await test("native modified sidebar links", async () => {
    const s = await session({ route: "/ONE/issues" }); const { page, aside } = s;
    await aside.getByRole("button", { name: "Recent issues", exact: true }).click();
    const links = [aside.locator('a[title="Two"]'), aside.locator('#project-nav-1 > a[href="#/ONE/board"]'), aside.locator(".recent-link").first(), aside.locator('a[title="Account settings"]')];
    await withNativeLinkDiagnostics(page, async openPopup => {
      for (const link of links) {
        const href = await link.getAttribute("href");
        for (const gesture of ["ctrl", "middle"] as const) {
          await openPopup(link, gesture, async popup => {
            assert.equal(new URL(popup.url()).hash, href);
            assert.equal(new URL(page.url()).hash, "#/ONE/issues");
            await popup.locator('aside a[title="Account settings"]').waitFor();
            await settle({ ...s, page: popup });
          });
        }
      }
    });
  });

  await test("folder create rename failure retry Cancel Escape", async () => {
    const s = await session(); const { page, aside, api } = s;
    const input = aside.getByRole("textbox", { name: "Folder name", exact: true });
    for (const mode of ["create", "rename"]) {
      const start = async () => {
        if (mode === "create") { const b = aside.getByRole("button", { name: "New folder", exact: true }); await b.focus(); await b.click(); }
        else await action(s, "Actions for folder Work", "Rename");
        await input.waitFor();
      };
      for (const cancel of ["Cancel", "Escape"]) {
        await start(); await input.fill("Discard this draft");
        const before = api.calls.filter(c => c.method !== "GET").length;
        if (cancel === "Escape") await input.press("Escape");
        else await aside.getByRole("button", { name: "Cancel", exact: true }).click();
        await input.waitFor({ state: "hidden" });
        assert.equal(api.calls.filter(c => c.method !== "GET").length, before);
        const trigger = aside.getByRole("button", { name: mode === "create" ? "New folder" : "Actions for folder Work", exact: true });
        assert.equal(await trigger.evaluate(el => document.activeElement === el), true, "Cancel restores the edit trigger");
      }
      await start();
      const name = mode === "create" ? "Research" : "Renamed work";
      const path = mode === "create" ? "/project-groups" : "/project-groups/11";
      const method = mode === "create" ? "POST" : "PATCH";
      await input.fill(name); await input.press("Tab");
      assert.equal(await input.inputValue(), name, "Blur must not save or discard");
      const gate = held(api, method, path, 409);
      await aside.getByRole("button", { name: "Save", exact: true }).click();
      await attr(input, "disabled", "");
      assert.equal(await aside.getByRole("button", { name: "Saving…", exact: true }).isDisabled(), true);
      gate.release();
      await aside.getByRole("alert").waitFor();
      assert.match(await aside.getByRole("alert").innerText(), /Fixture rejected/);
      assert.equal(await input.inputValue(), name);
      await attr(input, "aria-invalid", "true");
      assert.equal(await input.evaluate(el => document.activeElement === el), true);
      await input.press("Enter"); await input.waitFor({ state: "hidden" });
      await aside.getByRole("button", { name: `Actions for folder ${name}`, exact: true }).waitFor();
      const writes = api.calls.filter(c => c.method === method && c.path === path);
      const body = mode === "create" ? { name, parent_id: null, contains: null } : { name };
      assert.deepEqual(writes.map(c => c.body), [body, body]);
    }
    assert.equal(await domTree(page), api.describe());
  });

  await test("menu move up and down payloads, rollback, reload and per-user order", async () => {
    const s = await session(); const { aside, api } = s;
    const path = "/project-groups/move";
    let n = 0;
    const cases = [
      { trigger: "Actions for folder Work", name: "Work", down: { item: folder(11), parent_id: null, before: project(4) }, up: { item: folder(11), parent_id: null, before: folder(12) },
        original: `Work[One,Two],Personal[${longName}],Four`, moved: `Personal[${longName}],Work[One,Two],Four` },
      { trigger: "Actions for One", name: "One", down: { item: project(1), parent_id: 11, before: null }, up: { item: project(1), parent_id: 11, before: project(2) },
        original: `Work[One,Two],Personal[${longName}],Four`, moved: `Work[Two,One],Personal[${longName}],Four` },
    ];
    for (const c of cases) {
      await aside.getByRole("button", { name: c.trigger, exact: true }).click();
      assert.equal(await menuItem(s.page, "Move up").isDisabled(), true);
      await s.page.keyboard.press("Escape");
      await action(s, c.trigger, "Move down");
      assert.deepEqual((await called(api, ++n, "PUT", path)).body, c.down);
      await settle(s); assert.equal(await domTree(s.page), c.moved);
      await action(s, c.trigger, "Move up");
      assert.deepEqual((await called(api, ++n, "PUT", path)).body, c.up);
      await settle(s); assert.equal(await domTree(s.page), c.original);
      const gate = held(api, "PUT", path, 409);
      await action(s, c.trigger, "Move down");
      await called(api, ++n, "PUT", path);
      assert.equal(await domTree(s.page), c.moved, "Optimistic order is visible");
      gate.release(); await aside.getByRole("alert").waitFor();
      assert.match(await aside.getByRole("alert").innerText(), new RegExp(`${c.name} wasn't moved: Fixture rejected`));
      assert.equal(await domTree(s.page), c.original, "Failed write rolls visible order back");
    }
    await action(s, "Actions for One", "Move down"); await settle(s);
    await s.page.reload(); await settle(s);
    assert.equal(await domTree(s.page), `Work[Two,One],Personal[${longName}],Four`, "Canonical order reloads from API");
    const otherAPI = new API(); otherAPI.user.id = 2; otherAPI.user.display_name = "Other Operator";
    const other = await session({ api: otherAPI });
    assert.equal(await domTree(other.page), `Work[One,Two],Personal[${longName}],Four`);
    assert.equal(otherAPI.calls.filter(c => c.method !== "GET").length, 0);
    // The shared order other users read is never written by the sidebar.
    assert.equal(api.calls.some(c => c.method === "PUT" && (/^\/projects\/\d+$/.test(c.path) || c.path === "/projects/reorder")), false,
      "Sidebar order must not touch project records or the global order");
  });

  const folderAPI = () => { const api = new API(); api.projects[2].name = "Long"; return api; };
  const folderShots = process.env.E2E_FOLDER_SHOTS ?? shotDir;

  await test("drag projects and folders in, out, nested, reordered, and reloaded", async () => {
    const api = folderAPI();
    const s = await session({ api }); const { page, aside } = s;
    await settle(s);
    assert.equal(await domTree(page), "Work[One,Two],Personal[Long],Four");
    const ghost = page.locator("[data-sidebar-drag-ghost]");
    const indicator = aside.locator(".sidebar-drop-indicator");

    // Into a folder: the heading's middle, shown as a box around it.
    await grab(page, row(s, "project", 4));
    await hoverAt(page, row(s, "folder", 12), "inside");
    await indicator.and(aside.locator("[data-box]")).waitFor();
    assert.equal(await indicator.getAttribute("data-refused"), null);
    assert.match(await ghost.innerText(), /Four/);
    await page.screenshot({ path: resolve(folderShots, "folders-drag-desktop.png") });
    await page.mouse.up();
    assert.deepEqual(await moved(api), { item: project(4), parent_id: 12, before: null });
    await settle(s);
    assert.equal(await domTree(page), "Work[One,Two],Personal[Long,Four]");

    // Reorder inside a folder: before a sibling, a line above it.
    await grab(page, row(s, "project", 2));
    await hoverAt(page, row(s, "project", 1), "before");
    await indicator.waitFor();
    assert.equal(await indicator.getAttribute("data-box"), null);
    await page.mouse.up();
    assert.deepEqual(await moved(api), { item: project(2), parent_id: 11, before: project(1) });
    await settle(s);
    assert.equal(await domTree(page), "Work[Two,One],Personal[Long,Four]");

    // Out to the top level: the empty space under the tree is its end.
    await dragTo(page, row(s, "project", 1), aside.locator("[data-tree-root]"), "below");
    assert.deepEqual(await moved(api), { item: project(1), parent_id: null, before: null });
    await settle(s);
    assert.equal(await domTree(page), "Work[Two],Personal[Long,Four],One");

    // A folder into a folder, carrying its contents.
    await dragTo(page, row(s, "folder", 12), row(s, "folder", 11), "inside");
    assert.deepEqual(await moved(api), { item: folder(12), parent_id: 11, before: null });
    await settle(s);
    assert.equal(await domTree(page), "Work[Two,Personal[Long,Four]],One");
    assert.equal(await domTree(page), api.describe());
    await page.screenshot({ path: resolve(folderShots, "folders-tree-desktop.png") });

    // "After" an open folder's heading is its first slot.
    await dragTo(page, row(s, "project", 1), row(s, "folder", 12), "after");
    assert.deepEqual(await moved(api), { item: project(1), parent_id: 12, before: project(3) });
    await settle(s);
    assert.equal(await domTree(page), "Work[Two,Personal[One,Long,Four]]");

    // Releasing where it started sends nothing.
    const before = moves(api).length;
    await dragTo(page, row(s, "project", 4), row(s, "project", 4), "inside");
    assert.equal(moves(api).length, before);

    await page.reload(); await settle(s);
    assert.equal(await domTree(page), "Work[Two,Personal[One,Long,Four]]", "The arrangement reloads from the server");
  });

  await test("dragging folds only the dragged item, cancels on Escape, and springs folders open", async () => {
    const api = folderAPI();
    const s = await session({ api, route: "/ONE/issues", collapsed: [12] }); const { page, aside } = s;
    await settle(s);
    const ghost = page.locator("[data-sidebar-drag-ghost]");
    await attr(aside.locator("#project-nav-1"), "hidden", null);
    await attr(aside.locator("#group-12"), "hidden", "");

    // A dragged project leaves its sub-tabs behind; the ghost is one row.
    await grab(page, row(s, "project", 1));
    await attr(aside.locator("#project-nav-1"), "hidden", "");
    assert.ok((await ghost.boundingBox())!.height < 40, "Ghost is one compact row");
    await attr(aside.locator("#group-11"), "hidden", null);
    await page.keyboard.press("Escape");
    await ghost.waitFor({ state: "detached" });
    await page.mouse.up();
    await attr(aside.locator("#project-nav-1"), "hidden", null);
    assert.equal(moves(api).length, 0, "Escape cancels without saving");
    assert.equal(new URL(page.url()).hash, "#/ONE/issues", "The release after a drag is not a click");

    // A dragged folder folds its own contents away, then restores them.
    await grab(page, row(s, "folder", 11));
    await attr(aside.locator("#group-11"), "hidden", "");
    await page.keyboard.press("Escape"); await page.mouse.up();
    await attr(aside.locator("#group-11"), "hidden", null);

    // Hovering a closed folder opens it; dropping elsewhere closes it again.
    // (Sub-tabs folded first so no row sits in the auto-scroll edge.)
    await aside.getByRole("button", { name: "Collapse One", exact: true }).click();
    await grab(page, row(s, "project", 4));
    await hoverAt(page, row(s, "folder", 12), "inside");
    await attr(aside.locator("#group-12"), "hidden", null);
    await hoverAt(page, aside.locator("[data-tree-root]"), "below");
    await page.mouse.up();
    await attr(aside.locator("#group-12"), "hidden", "");
    assert.equal(moves(api).length, 0, "Four was already last at the top level");

    // Dropping inside a spring-opened folder keeps the way to it open.
    await grab(page, row(s, "project", 4));
    await hoverAt(page, row(s, "folder", 12), "inside");
    await attr(aside.locator("#group-12"), "hidden", null);
    await hoverAt(page, row(s, "project", 3), "after");
    await page.mouse.up();
    assert.deepEqual(await moved(api), { item: project(4), parent_id: 12, before: null });
    await settle(s);
    await attr(aside.locator("#group-12"), "hidden", null);
    assert.equal(await domTree(page), "Work[One,Two],Personal[Long,Four]");
    assert.deepEqual(await page.evaluate(() => JSON.parse(localStorage.getItem("lific:sidebar:collapsed-groups")!)), []);

    // A failed save puts the tree back and says so.
    const gate = held(api, "PUT", "/project-groups/move", 409);
    await dragTo(page, row(s, "project", 4), row(s, "folder", 11), "inside");
    await called(api, 2, "PUT", "/project-groups/move");
    assert.equal(await domTree(page), "Work[One,Two,Four],Personal[Long]", "Optimistic move is visible");
    gate.release();
    await aside.getByRole("alert").filter({ hasText: "Four wasn't moved: Fixture rejected" }).waitFor();
    assert.equal(await domTree(page), "Work[One,Two],Personal[Long,Four]");
  });

  await test("refused drops explain themselves and send nothing", async () => {
    const api = folderAPI();
    api.arrange([["A", 21, [["B", 22, [["C", 23, [["D", 24, [1]]]]]]]], ["E", 25, [["F", 26, [2]]]], 3, 4]);
    const s = await session({ api }); const { page, aside } = s;
    await settle(s);
    assert.equal(await domTree(page), "A[B[C[D[One]]]],E[F[Two]],Long,Four");
    const ghost = page.locator("[data-sidebar-drag-ghost]");
    const indicator = aside.locator(".sidebar-drop-indicator");

    // Too deep: E holds a subfolder, so inside C would make 5 levels.
    await grab(page, row(s, "folder", 25));
    await hoverAt(page, row(s, "folder", 23), "inside");
    await indicator.and(aside.locator("[data-refused]")).waitFor();
    assert.match(await ghost.innerText(), /Folders can nest 4 levels deep\. Moving 'E' there would make 5 levels\./);
    await page.screenshot({ path: resolve(folderShots, "folders-refused-desktop.png") });
    await page.mouse.up();
    await page.getByRole("alert").filter({ hasText: "Can't move E there. Folders can nest 4 levels deep." }).waitFor();
    assert.equal(moves(api).length, 0);

    // A folder over its own heading, after leaving it, is "inside itself".
    await grab(page, row(s, "folder", 26));
    await hoverAt(page, row(s, "project", 3), "before");
    await hoverAt(page, row(s, "folder", 26), "inside");
    await indicator.and(aside.locator("[data-refused]")).waitFor();
    assert.match(await ghost.innerText(), /A folder can't go inside itself/);
    await page.mouse.up();
    assert.equal(moves(api).length, 0);

    // F alone fits inside C, at 4 levels.
    await dragTo(page, row(s, "folder", 26), row(s, "folder", 23), "inside");
    assert.deepEqual(await moved(api), { item: folder(26), parent_id: 23, before: null });
    await settle(s);
    assert.equal(await domTree(page), "A[B[C[D[One],F[Two]]]],E[],Long,Four");

    // The menus say the same: no subfolder at the cap, and the picker marks
    // each refused destination with its reason.
    await action(s, "Actions for folder D", "Move to folder…");
    for (const [name, hint] of [["Top level", null], ["C", "Here now"], ["D", "Inside itself"], ["E", null]] as const) {
      const item = menuItem(page, hint ? `${name} ${hint}` : name);
      assert.equal(await item.isDisabled(), !!hint, name);
    }
    await page.screenshot({ path: resolve(folderShots, "folders-picker-desktop.png") });
    await page.keyboard.press("Escape");
    await aside.getByRole("button", { name: "Actions for folder D", exact: true }).click();
    assert.equal(await menuItem(page, "New subfolder 4 levels max").isDisabled(), true);
    await page.keyboard.press("Escape");

    // Four levels still fit the narrowest sidebar without sideways scrolling.
    await page.evaluate(() => localStorage.setItem("lific:sidebar:width", "190"));
    await page.reload(); await settle(s);
    const one = aside.locator('a[title="One"]');
    await page.mouse.move(600, 500);
    assert.ok((await one.locator("span.truncate").boundingBox())!.width >= 24, "A name four levels deep stays readable at 190px");
    await one.hover();
    const actions = aside.getByRole("button", { name: "Actions for One", exact: true });
    const box = (await actions.boundingBox())!;
    assert.ok(box.x + box.width <= 190, "Deepest project keeps its actions in view");
    assert.equal(await aside.locator(".sidebar-scroll").evaluate(el => el.scrollWidth <= el.clientWidth), true, "No horizontal sidebar scroll");
    await page.screenshot({ path: resolve(folderShots, "folders-tree-narrow.png") });
  });

  await test("folder buttons and menus create, file, move out and delete", async () => {
    const api = folderAPI();
    const s = await session({ api }); const { page, aside } = s;
    await settle(s);
    const input = aside.getByRole("textbox", { name: "Folder name", exact: true });
    const posts = () => api.calls.filter(c => c.method === "POST" && c.path === "/project-groups").map(c => c.body);

    await aside.getByRole("button", { name: "New folder", exact: true }).click();
    await input.fill("Clients"); await input.press("Enter"); await input.waitFor({ state: "hidden" });
    assert.deepEqual(posts().at(-1), { name: "Clients", parent_id: null, contains: null });
    await settle(s);
    assert.equal(await domTree(page), "Work[One,Two],Personal[Long],Clients[],Four");
    assert.ok(await aside.locator("#group-13").getByText("Empty", { exact: true }).isVisible());

    await action(s, "Actions for folder Work", "New subfolder");
    await input.waitFor();
    assert.equal(await aside.locator("#group-11").getByRole("textbox", { name: "Folder name" }).count(), 1, "The name field shows where the folder goes");
    await input.fill("Acme"); await input.press("Enter"); await input.waitFor({ state: "hidden" });
    assert.deepEqual(posts().at(-1), { name: "Acme", parent_id: 11, contains: null });
    await settle(s);
    assert.equal(await domTree(page), "Work[Acme[],One,Two],Personal[Long],Clients[],Four");

    await action(s, "Actions for Four", "Move to folder…");
    assert.equal(await menuItem(page, "Top level Here now").isDisabled(), true);
    await menuItem(page, "Acme").click();
    assert.deepEqual(await moved(api), { item: project(4), parent_id: 14, before: null });
    await settle(s);
    assert.equal(await domTree(page), "Work[Acme[Four],One,Two],Personal[Long],Clients[]");

    await aside.getByRole("button", { name: "Actions for folder Work", exact: true }).click();
    await menuItem(page, "Rename").waitFor();
    await page.screenshot({ path: resolve(folderShots, "folders-menu-desktop.png") });
    await page.keyboard.press("Escape");

    await action(s, "Actions for Four", "Move out of Acme");
    assert.deepEqual(await moved(api), { item: project(4), parent_id: 11, before: project(1) });
    await settle(s);
    assert.equal(await domTree(page), "Work[Acme[],Four,One,Two],Personal[Long],Clients[]");

    // "New folder…" in the picker wraps the item in a new folder where it is.
    await action(s, "Actions for Long", "Move to folder…");
    await menuItem(page, "New folder…").click();
    await aside.locator("#group-12").getByRole("textbox", { name: "Folder name" }).waitFor();
    await input.fill("Archive"); await input.press("Enter"); await input.waitFor({ state: "hidden" });
    assert.deepEqual(posts().at(-1), { name: "Archive", parent_id: 12, contains: project(3) });
    await settle(s);
    assert.equal(await domTree(page), "Work[Acme[],Four,One,Two],Personal[Archive[Long]],Clients[]");

    // Deleting a folder hands its contents to its parent, in its place.
    await action(s, "Actions for folder Personal", "Delete folder");
    await called(api, 1, "DELETE", "/project-groups/12");
    await settle(s);
    assert.equal(await domTree(page), "Work[Acme[],Four,One,Two],Archive[Long],Clients[]");
    assert.ok(await aside.locator('a[title="Long"]').isVisible(), "Deleting never hides a project");
    assert.equal(await domTree(page), api.describe());
  });

  await test("mobile folders nest with counts and move through menus", async () => {
    const api = folderAPI();
    api.arrange([["Work", 11, [1, ["Clients", 13, [2, ["Acme", 14, []]]]]], ["Personal", 12, [3]], 4]);
    const s = await session({ api, mobile: true }); const { page } = s;
    const rootPane = page.locator("[data-mobile-root]");
    await page.getByRole("button", { name: "Open navigation", exact: true }).click();
    await page.waitForFunction(() => new DOMMatrix(getComputedStyle(document.querySelector('[data-mobile-nav]')!).transform).m41 === 0);
    const work = rootPane.getByRole("button", { name: /^Work/ });
    assert.match((await work.innerText()).replace(/\s+/g, " "), /Work 2/, "Folder counts every project inside it");
    await rootPane.getByText("Empty", { exact: true }).waitFor();
    await page.screenshot({ path: resolve(folderShots, "folders-tree-mobile.png") });
    await rootPane.getByRole("button", { name: "Actions for folder Clients", exact: true }).click();
    await menuItem(page, "New subfolder").waitFor();
    await page.screenshot({ path: resolve(folderShots, "folders-menu-mobile.png") });
    await menuItem(page, "Move to folder…").click();
    for (const name of ["Clients Inside itself", "Acme Inside itself", "Work Here now"]) assert.equal(await menuItem(page, name).isDisabled(), true, name);
    await page.screenshot({ path: resolve(folderShots, "folders-refused-mobile.png") });
    await menuItem(page, "Personal").click();
    assert.deepEqual((await called(api, 1, "PUT", "/project-groups/move")).body, { item: folder(13), parent_id: 12, before: null });
    await settle(s);
    assert.equal(api.describe(), "Work[One],Personal[Long,Clients[Two,Acme[]]],Four");
    await rootPane.getByRole("button", { name: "Actions for folder Clients", exact: true }).waitFor();
  });

  await test("Settings identity and theme share live shell state", async () => {
    const s = await session(); const { page, aside } = s;
    const identity = aside.locator('a[title="Account settings"]');
    await identity.click(); await page.waitForURL("**/#/settings");
    await page.getByLabel("Display name", { exact: true }).fill("Updated Operator");
    await page.getByRole("button", { name: "Save changes", exact: true }).click();
    await identity.filter({ hasText: "Updated Operator" }).waitFor();
    await page.getByRole("button", { name: "Dark", exact: true }).click();
    await attr(aside.getByRole("button", { name: "Choose theme, current: dark", exact: true }), "title", "Theme: dark");
    assert.equal(await page.evaluate(() => document.documentElement.classList.contains("dark")), true);
    await action(s, "Choose theme, current: dark", "Light");
    assert.equal(await page.evaluate(() => localStorage.getItem("lific_theme")), "light");
    assert.ok((await page.getByRole("button", { name: "Light", exact: true }).getAttribute("class"))?.includes("bg-[var(--surface)]"));
    await page.reload();
    await identity.filter({ hasText: "Updated Operator" }).waitFor();
    await aside.getByRole("button", { name: "Choose theme, current: light", exact: true }).waitFor();
  });

  await test("real Layout mobile menus history and palette suppression", async () => {
    const s = await session({ mobile: true }); const { page } = s;
    const depth = (n: number) => page.waitForFunction(n => history.state?.lificMobileNav?.depth === n, n);
    const rootPane = page.locator("[data-mobile-root]");
    const projectPane = page.locator("[data-mobile-project]");
    const open = async () => { await page.getByRole("button", { name: "Open navigation", exact: true }).click(); await depth(1); };
    const closed = () => page.waitForFunction(() => !(window as any).fixture.mobile());
    await open();
    assert.equal(await page.locator("main").evaluate(el => !!el.closest("[inert]")), true);
    for (const key of ["Control+k", "Control+p", "Meta+k", "Meta+p"]) await page.keyboard.press(key);
    assert.equal(await page.evaluate(() => (window as any).fixture.palette()), false);
    await rootPane.getByRole("button", { name: "Open One navigation", exact: true }).click(); await depth(2);
    await page.goBack(); await depth(1);
    await page.goForward(); await depth(2);
    await projectPane.getByRole("link", { name: "Board", exact: true }).click();
    await page.waitForURL("**/#/ONE/board"); await closed();
    await page.goBack(); await depth(0);
    assert.equal(new URL(page.url()).hash, "#/");
    await open();
    await rootPane.getByRole("button", { name: "Actions for One", exact: true }).click();
    await menuItem(page, "Move down").click();
    assert.deepEqual((await called(s.api, 1, "PUT", "/project-groups/move")).body, { item: project(1), parent_id: 11, before: null });
    await rootPane.getByRole("button", { name: "Actions for folder Work", exact: true }).click();
    await menuItem(page, "Rename").waitFor();
    await page.keyboard.press("Escape"); await depth(1);
    await rootPane.getByRole("button", { name: "New project or folder", exact: true }).click();
    await menuItem(page, "New project").click();
    await page.waitForURL("**/#/projects/new"); await closed();
    await page.goBack(); await depth(0);
    await open();
    await page.waitForFunction(() => new DOMMatrix(getComputedStyle(document.querySelector('[data-mobile-nav]')!).transform).m41 === 0);
    await page.screenshot({ path: resolve(shotDir, "sidebar-mobile.png") });
    await page.keyboard.press("Escape"); await depth(0); await closed();
    await page.keyboard.press("Control+k");
    await page.waitForFunction(() => (window as any).fixture.palette());
    await page.keyboard.press("Escape");
  });

  console.log(`Sidebar: ${passed} passed, ${failures.length} failed. Screenshots: ${shotDir}`);
  if (failures.length) process.exitCode = 1;
} finally {
  for (const gate of allGates) gate.release();
  await browser!?.close();
  await server.close();
  clearTimeout(deadline);
}

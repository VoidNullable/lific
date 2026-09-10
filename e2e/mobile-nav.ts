#!/usr/bin/env bun
// Real MobileNav and ContextMenu in Chromium, with a hash-router fixture.
// Vite runs in-process and closes in finally. No database or background jobs.
import { strict as assert } from "node:assert";
import { resolve } from "node:path";
import { chromium } from "playwright";
import { createServer } from "../web/node_modules/vite/dist/node/index.js";

const root = resolve(import.meta.dir, "../web");
const fixtureId = root + "/src/MobileNavFixture.svelte";
const fixture = `<script>
  import MobileNav from './lib/MobileNav.svelte';
  import ContextMenu from './lib/ContextMenu.svelte';
  import { openContextMenu } from './lib/contextMenuState.svelte';
  import { mobileNavState } from './lib/mobileNavState.svelte';
  let nav = $state();
  let open = $state(false);
  let route = $state(location.hash.slice(1) || '/');
  let editingGroupId = $state(null);
  let draftGroupName = $state('');
  let groupEditError = $state('');
  let orderError = $state('');
  let ready = $state(!sessionStorage.getItem('hold-mobile-mount'));
  let fail = true;
  let projects = $state([{ id: 1, identifier: 'ONE', name: 'One', emoji: '' }, { id: 2, identifier: 'TWO', name: 'Two', emoji: '' }].filter(p => p.id !== 1 || !sessionStorage.getItem('missing-one')));
  let groups = $state([{ id: 1, name: 'Work', project_ids: [1, 2] }, { id: 2, name: 'Other', project_ids: [] }]);
  const navigate = (path) => { window.navCalls.push(path); location.hash = path; route = path; };
  window.navCalls = [];
  window.moves = [];
  window.externalRoute = navigate;
  window.cancelQueued = () => { nav.navigateTo('/cancelled'); route = '/changed-elsewhere'; };
  window.sharedOpen = () => mobileNavState.open;
  window.finishLoading = () => { sessionStorage.removeItem('hold-mobile-mount'); ready = true; };
  window.removeOne = () => { projects = projects.filter(p => p.id !== 1); sessionStorage.setItem('missing-one', '1'); };
  window.failMove = () => { orderError = "Project order wasn't saved: Connection lost."; };
  function menu(e, items) {
    e.preventDefault(); e.stopPropagation();
    const r = e.currentTarget.getBoundingClientRect();
    openContextMenu(r.left, r.bottom, items);
  }
  async function save() {
    await Promise.resolve();
    if (fail) { fail = false; groupEditError = 'Name already exists.'; return false; }
    groups = groups.map(g => g.id === editingGroupId ? {...g, name: draftGroupName} : g);
    editingGroupId = null; groupEditError = ''; return true;
  }
</script>
<svelte:window onhashchange={() => route = location.hash.slice(1) || '/'} />
<main id="background"><button id="trigger" aria-label="Open navigation" onclick={() => nav.openAt(null)}>Open navigation</button><button id="direct" onclick={() => nav.openAt(projects[0])}>Open One directly</button><a href="#/outside">Outside</a><p>{route}</p></main>
{#if ready}
<MobileNav bind:this={nav} bind:open {route} {navigate} user={{username:'test',display_name:'Test'}}
 {projects} {groups} projectsIn={(g) => projects.filter(p => g.project_ids.includes(p.id))} ungrouped={[]}
 collapsedGroups={new Set()} onToggleGroup={() => {}} onOpenPalette={() => window.paletteOpened = true}
 onOpenCreateMenu={(e) => menu(e, [{label:'New project', action:() => nav.navigateTo('/projects/new')}])}
 onProjectMenu={(e) => menu(e, [{label:'Inspect', action:() => {}}])}
 onGroupMenu={(e,g) => menu(e, [{label:'Rename', action:() => {editingGroupId=g.id; draftGroupName=g.name; groupEditError='';}}])}
 bind:editingGroupId bind:draftGroupName {groupEditError} {orderError} onCommitGroupName={save}
 onCancelGroupEdit={() => {editingGroupId=null; groupEditError='';}}
 onMoveProject={(p,d) => window.moves.push(['project',p.id,d])}
 onMoveGroup={(g,d) => window.moves.push(['group',g.id,d])}
 themePref="system" themeResolved="light" onCycleTheme={(e) => menu(e, [{label:'System',action:() => {}}])} />
{/if}
<ContextMenu />`;

const server = await createServer({
  root,
  server: { host: "127.0.0.1", port: 0, strictPort: false },
  plugins: [{
    name: "mobile-nav-fixture", enforce: "pre",
    resolveId(id) { if (id === "/src/MobileNavFixture.svelte") return fixtureId; },
    load(id) { if (id === fixtureId) return fixture; },
    configureServer(server) {
      server.middlewares.use(async (req, res, next) => {
        if (req.url?.split("?")[0] !== "/") return next();
        res.setHeader("Content-Type", "text/html");
        res.end(await server.transformIndexHtml("/", `<div id="app"></div><script type="module">import {mount} from 'svelte'; import Fixture from '/src/MobileNavFixture.svelte'; import '/src/app.css'; mount(Fixture,{target:document.getElementById('app')});</script>`));
      });
    },
  }],
});
const deadline = setTimeout(() => { console.error("Mobile navigation test deadline exceeded"); process.exit(1); }, 90_000);
let browser;
try {
  await server.listen();
  browser = await chromium.launch({ headless: true });
  const page = await browser.newPage({ viewport: { width: 390, height: 844 } });
  page.setDefaultTimeout(8_000);
  const errors: string[] = [];
  page.on("pageerror", (error) => errors.push(error.message));
  await page.goto(server.resolvedUrls!.local[0] + "#/");
  const panel = page.locator("[data-mobile-nav]");
  const rootPane = page.locator("[data-mobile-root]");
  const projectPane = page.locator("[data-mobile-project]");
  const depth = (n: number) => page.waitForFunction(n => history.state?.lificMobileNav?.depth === n, n);
  const active = () => page.evaluate(() => document.activeElement?.getAttribute("aria-label") || document.activeElement?.id || document.activeElement?.textContent?.trim());
  const closed = () => page.waitForFunction(() => document.querySelector("[data-mobile-nav]")?.hasAttribute("inert"));
  const openRoot = async () => { await page.locator("#trigger").click(); await depth(1); };

  await openRoot();
  assert.equal(await page.evaluate(() => (document.querySelector("#background") as HTMLElement).inert), true);
  assert.equal(await active(), "Close navigation");
  await page.locator("#trigger").evaluate((el: HTMLElement) => el.focus());
  assert.equal(await active(), "Close navigation");
  if (process.env.E2E_SCREENSHOT) {
    await page.waitForFunction(() => new DOMMatrix(getComputedStyle(document.querySelector('[data-mobile-nav]')!).transform).m41 === 0);
    await page.screenshot({ path: process.env.E2E_SCREENSHOT });
  }
  await page.keyboard.press("Shift+Tab");
  assert.match((await active())!, /Choose theme/);
  await page.keyboard.press("Tab");
  assert.equal(await active(), "Close navigation");
  await rootPane.getByRole("button", { name: "Open One navigation", exact: true }).click();
  await depth(2);
  assert.equal(await rootPane.getAttribute("inert"), "");
  assert.equal(await active(), "Projects");
  await rootPane.getByRole("link", { name: "Home", exact: true, includeHidden: true }).evaluate((el: HTMLElement) => el.focus());
  assert.equal(await active(), "Projects");
  await page.goBack(); await depth(1);
  assert.equal(await active(), "Open One navigation");
  await page.goBack(); await depth(0); await closed();
  assert.equal(await active(), "Open navigation");
  await rootPane.locator('a[href="#/"]').evaluate((el: HTMLElement) => el.focus());
  assert.equal(await active(), "Open navigation");
  await page.goForward(); await depth(1);
  await page.goForward(); await depth(2);
  assert.equal(await projectPane.getAttribute("inert"), null);
  const popupReady = page.context().waitForEvent("page");
  await projectPane.getByRole("link", { name: "Issues", exact: true }).click({ modifiers: ["Control"] });
  const popup = await popupReady;
  await popup.waitForLoadState();
  assert.equal(new URL(popup.url()).hash, "#/ONE/issues");
  await popup.close();
  await depth(2);
  assert.equal(await panel.getAttribute("inert"), null);
  await projectPane.getByRole("link", { name: "Issues", exact: true }).click();
  await page.waitForFunction(() => location.hash === "#/ONE/issues");
  await closed();
  await page.goBack(); await depth(0);
  assert.equal(new URL(page.url()).hash, "#/");
  assert.equal(await panel.getAttribute("inert"), "");
  await openRoot();
  await rootPane.getByRole("link", { name: "Home", exact: true }).click();
  await depth(0); await closed();
  assert.deepEqual(await page.evaluate(() => (window as any).navCalls), ["/ONE/issues"]);

  await page.locator("#direct").click(); await depth(2);
  await page.keyboard.press("Escape"); await depth(1);
  await page.keyboard.press("Escape"); await depth(0); await closed();
  assert.equal(await active(), "direct");
  await openRoot();
  await rootPane.getByRole("button", { name: "Actions for One", exact: true }).click();
  await page.getByRole("menu").waitFor();
  await page.waitForFunction(() => !!document.activeElement?.closest('[role="menu"]'));
  assert.equal(await page.getByRole("menuitem", { name: "Move up", exact: true }).isDisabled(), true);
  await page.keyboard.press("Escape");
  await page.getByRole("menu").waitFor({ state: "hidden" });
  await depth(1);
  await rootPane.getByRole("button", { name: "Actions for One", exact: true }).click();
  await page.getByRole("menuitem", { name: "Move down", exact: true }).click();
  assert.deepEqual(await page.evaluate(() => (window as any).moves), [["project", 1, "down"]]);

  await rootPane.getByRole("button", { name: "Actions for Work", exact: true }).click();
  await page.getByRole("menuitem", { name: "Rename", exact: true }).click();
  const input = rootPane.getByRole("textbox", { name: "Group name", exact: true });
  await input.fill("Personal");
  await input.press("Tab");
  assert.equal(await input.inputValue(), "Personal");
  await rootPane.getByRole("button", { name: "Save", exact: true }).click();
  await rootPane.getByRole("alert").waitFor();
  assert.equal(await input.inputValue(), "Personal");
  assert.match(await rootPane.getByRole("alert").innerText(), /Name already exists/);
  await input.press("Escape");
  await input.waitFor({ state: "hidden" });
  await depth(1);
  assert.equal(await active(), "Actions for Work");
  await rootPane.getByRole("button", { name: "Actions for Work", exact: true }).click();
  await page.getByRole("menuitem", { name: "Rename", exact: true }).click();
  await input.fill("Personal"); await input.press("Enter");
  await input.waitFor({ state: "hidden" });
  await rootPane.getByRole("button", { name: "Actions for Personal", exact: true }).waitFor();
  assert.equal(await active(), "Actions for Personal");
  await rootPane.getByRole("button", { name: "Actions for Personal", exact: true }).click();
  await page.getByRole("menuitem", { name: "Move down", exact: true }).click();
  assert.deepEqual(await page.evaluate(() => (window as any).moves), [["project", 1, "down"], ["group", 1, "down"]]);

  await page.evaluate(() => (window as any).externalRoute('/outside'));
  await closed();
  await page.goBack(); await depth(1);
  assert.equal(await panel.getAttribute("inert"), null);
  await page.goForward(); await closed();
  await openRoot();
  await page.evaluate(() => (window as any).cancelQueued());
  await depth(0); await closed();
  assert.equal(await page.evaluate(() => (window as any).navCalls.includes('/cancelled')), false);
  await openRoot();
  await rootPane.getByRole("button", { name: "New project or group", exact: true }).click();
  await page.getByRole("menuitem", { name: "New project", exact: true }).click();
  await page.waitForFunction(() => location.hash === '#/projects/new');
  await closed();
  await page.goBack(); await depth(0);
  await openRoot();
  await page.setViewportSize({ width: 1024, height: 768 });
  await depth(0); await closed();
  assert.equal(await page.evaluate(() => (window as any).sharedOpen()), false);

  // Reload adopts the entry rather than pushing a fresh root over it.
  await page.setViewportSize({ width: 390, height: 844 });
  await openRoot();
  const rootHistory = await page.evaluate(() => ({state: history.state, length: history.length}));
  await page.reload();
  await rootPane.getByRole('button', {name: 'Open One navigation', exact: true}).waitFor();
  assert.equal(await panel.getAttribute('inert'), null);
  assert.equal(await active(), 'Close navigation');
  assert.deepEqual(await page.evaluate(() => ({state: history.state, length: history.length})), rootHistory);
  await page.goBack(); await depth(0); await closed();
  assert.equal(await active(), 'Open navigation');
  await page.goForward(); await depth(1);
  await rootPane.getByRole('button', {name: 'Open One navigation', exact: true}).click();
  const projectHistory = await page.evaluate(() => ({state: history.state, length: history.length}));

  // Match Layout's asynchronous user/project bootstrap: restore on mount,
  // not before the project catalog has arrived.
  await page.evaluate(() => sessionStorage.setItem('hold-mobile-mount', '1'));
  await page.reload();
  await page.waitForFunction(() => typeof (window as any).finishLoading === 'function');
  assert.equal(await panel.count(), 0);
  await page.evaluate(() => (window as any).finishLoading());
  await projectPane.getByRole('heading', {name: 'One', exact: true}).waitFor();
  assert.equal(await projectPane.getAttribute('inert'), null);
  assert.equal(await active(), 'Projects');
  assert.deepEqual(await page.evaluate(() => ({state: history.state, length: history.length})), projectHistory);

  const ownedHash = new URL(page.url()).hash;
  await page.evaluate(() => (window as any).externalRoute('/another-route'));
  await closed();
  await page.goBack(); await depth(2);
  assert.equal(new URL(page.url()).hash, ownedHash);
  assert.equal(await projectPane.getAttribute('inert'), null);
  await page.goForward(); await closed();
  assert.equal(new URL(page.url()).hash, '#/another-route');
  await page.goBack(); await depth(2);
  await page.evaluate(() => { history.back(); history.back(); });
  await depth(0); await closed();
  await page.evaluate(() => { history.forward(); history.forward(); });
  await depth(2);
  await page.waitForFunction(() => !!document.activeElement?.closest('[data-mobile-project]:not([inert])'));
  assert.equal(await projectPane.getAttribute('inert'), null);
  assert.deepEqual(await page.evaluate(() => (window as any).navCalls), ['/another-route']);
  await page.evaluate(() => new Promise<void>((resolve) => {
    let events = 0;
    const onPop = () => {
      if (++events === 1) history.back();
      else { window.removeEventListener('popstate', onPop); resolve(); }
    };
    window.addEventListener('popstate', onPop);
    history.forward();
  }));
  await depth(2);
  assert.equal(new URL(page.url()).hash, ownedHash);
  assert.equal(await projectPane.getAttribute('inert'), null);

  // A removed project never leaves stale destinations behind. The owned
  // entry remains an honest, navigable unavailable-project pane on Forward.
  await page.evaluate(() => (window as any).removeOne());
  await projectPane.getByRole('heading', {name: 'Project unavailable', exact: true}).waitFor();
  assert.equal(await projectPane.locator('a[href]').count(), 0);
  await page.reload();
  await projectPane.getByRole('heading', {name: 'Project unavailable', exact: true}).waitFor();
  await page.goBack(); await depth(1);
  await page.goForward(); await depth(2);
  await projectPane.getByRole('button', {name: 'Projects', exact: true}).click(); await depth(1);
  const theme = rootPane.getByRole('button', {name: 'Choose theme, current: system', exact: true});
  assert.equal(await theme.getAttribute('aria-haspopup'), 'menu');
  assert.equal(await theme.getAttribute('title'), 'Choose theme, current: system');
  await theme.click();
  await page.getByRole('menuitem', {name: 'System', exact: true}).waitFor();
  await page.keyboard.press('Escape'); await depth(1);
  await page.evaluate(() => (window as any).failMove());
  assert.equal(await rootPane.getByRole('alert').innerText(), "Project order wasn't saved: Connection lost.");

  // Reject malformed and foreign namespaced data without trusting its depth.
  for (const invalid of [
    {version: 2}, {session: 'not-a-session'}, {depth: 99}, {depth: '1'},
    {project: 'ONE'}, {depth: 2, project: '../outside'}, {href: 'https://elsewhere.invalid/'},
  ]) {
    await page.evaluate((invalid) => {
      history.replaceState({lificMobileNav: {...history.state.lificMobileNav, ...invalid}}, '');
    }, invalid);
    await page.reload();
    await page.waitForFunction(() => typeof (window as any).sharedOpen === 'function');
    assert.equal(await page.evaluate(() => (window as any).sharedOpen()), false);
    await page.locator('#trigger').click(); await depth(1);
    await rootPane.getByRole('button', {name: 'Close navigation', exact: true}).click(); await depth(0); await closed();
    await page.goForward(); await depth(1);
  }

  // LAN HTTP lacks randomUUID. Disable it before any app code runs, on
  // every reload, and verify the getRandomValues fallback owns real entries.
  const lan = await browser.newContext({ viewport: { width: 390, height: 844 } });
  try {
    await lan.addInitScript(() => {
      Object.defineProperty(crypto, 'randomUUID', { value: undefined, configurable: true });
      const getRandomValues = crypto.getRandomValues.bind(crypto);
      (window as any).randomValueCalls = 0;
      Object.defineProperty(crypto, 'getRandomValues', {
        value: (array: ArrayBufferView) => {
          (window as any).randomValueCalls++;
          return getRandomValues(array);
        },
        configurable: true,
      });
    });
    const fallback = await lan.newPage();
    fallback.setDefaultTimeout(8_000);
    fallback.on('pageerror', (error) => errors.push(error.message));
    await fallback.goto(server.resolvedUrls!.local[0] + '#/');
    assert.equal(await fallback.evaluate(() => typeof crypto.randomUUID), 'undefined');
    const callsBefore = await fallback.evaluate(() => (window as any).randomValueCalls);
    const fallbackDepth = (depth: number) => fallback.waitForFunction((depth) => {
      if (history.state?.lificMobileNav?.depth !== depth) return false;
      const panel = document.querySelector('[data-mobile-nav]');
      if (!panel || (window as any).sharedOpen() !== (depth > 0)) return false;
      if (depth === 0) return panel.hasAttribute('inert');
      const pane = panel.querySelector(depth === 1 ? '[data-mobile-root]' : '[data-mobile-project]');
      return !panel.hasAttribute('inert') && pane && !pane.hasAttribute('inert');
    }, depth);
    await fallback.locator('#trigger').click(); await fallbackDepth(1);
    assert.ok(await fallback.evaluate(() => (window as any).randomValueCalls) > callsBefore);
    const session = await fallback.evaluate(() => history.state.lificMobileNav.session);
    assert.match(session, /^[0-9a-f]{8}-[0-9a-f]{4}-4[0-9a-f]{3}-[89ab][0-9a-f]{3}-[0-9a-f]{12}$/);
    await fallback.getByRole('button', {name: 'Open One navigation', exact: true}).click();
    await fallbackDepth(2);
    await fallback.getByRole('button', {name: 'Close navigation', exact: true}).click();
    await fallbackDepth(0);
    await fallback.goForward(); await fallbackDepth(1);
    const beforeReload = await fallback.evaluate(() => ({state: history.state, length: history.length}));
    await fallback.reload(); await fallbackDepth(1);
    assert.equal(await fallback.evaluate(() => typeof crypto.randomUUID), 'undefined');
    assert.deepEqual(await fallback.evaluate(() => ({state: history.state, length: history.length})), beforeReload);
    await fallback.goForward(); await fallbackDepth(2);
    await fallback.reload(); await fallbackDepth(2);
    await fallback.getByRole('heading', {name: 'One', exact: true}).waitFor();
    assert.equal(await fallback.evaluate(() => typeof crypto.randomUUID), 'undefined');
    assert.equal(await fallback.evaluate(() => history.state.lificMobileNav.session), session);
    await fallback.goBack(); await fallbackDepth(1);
    await fallback.goBack(); await fallbackDepth(0);
    await fallback.goForward(); await fallbackDepth(1);
    await fallback.goForward(); await fallbackDepth(2);
    assert.equal(await fallback.evaluate(() => history.state.lificMobileNav.session), session);
    await fallback.getByRole('button', {name: 'Close navigation', exact: true}).click();
    await fallbackDepth(0);
  } finally {
    await lan.close();
  }
  assert.deepEqual(errors, []);
  console.log("Mobile navigation passed: modal/focus, Back/Forward and rapid traversals, reload/async mount, LAN HTTP UUID fallback, missing projects, malformed history, route unwind/cancellation, links, menus/theme, move errors, group edit, breakpoint.");
} finally {
  await browser?.close();
  await server.close();
  clearTimeout(deadline);
}

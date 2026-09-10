#!/usr/bin/env bun
// Real Svelte components and browser focus/default actions. No backend or persistent server.
import { strict as assert } from "node:assert";
import { resolve } from "node:path";
import { chromium } from "playwright";
import { createServer } from "../web/node_modules/vite/dist/node/index.js";

const root = resolve(import.meta.dir, "../web");
const fixtureId = root + "/src/ContextMenuFixture.svelte";
const fixture = `<script>
  import ContextMenu from './lib/ContextMenu.svelte';
  import CommandPalette from './lib/CommandPalette.svelte';
  import Settings from './routes/Settings.svelte';
  import { openContextMenu, contextMenuState } from './lib/contextMenuState.svelte';
  import { mobileNavState } from './lib/mobileNavState.svelte';
  import { commandPaletteState } from './lib/commandPaletteState.svelte';
  import { shortcutsSuppressed } from './lib/shortcuts';
  import { init, themePreference, resolvedTheme, getPreference, setPreference } from './lib/theme';
  import { currentUser } from './lib/userState';
  init();
  let showSettings = $state(false);
  window.actions = [];
  const items = [
    {label:'Disabled first', disabled:true, action:()=>window.actions.push('disabled')},
    {label:'First', action:()=>window.actions.push('first')},
    {label:'Disabled link', disabled:true, href:'#/disabled', action:()=>window.actions.push('disabled')},
    {label:'Middle', action:()=>window.actions.push('middle')},
    {label:'Focus editor', action:()=>document.querySelector('#editor').focus()},
    {label:'Navigate', href:'#/destination', action:()=>window.actions.push('navigate')},
    {label:'Native link', href:'#/native'},
    {label:'Disabled last', disabled:true, action:()=>window.actions.push('disabled')},
  ];
  function open(e) {
    e.preventDefault(); e.stopPropagation();
    openContextMenu(24, 110, items, e.currentTarget);
  }
  window.fixture = {
    open: (mode = 'normal') => openContextMenu(24,110,mode === 'empty' ? [] : mode === 'disabled' ? items.filter(i=>i.disabled) : items, document.querySelector('#trigger')),
    mobile: value => mobileNavState.open = value,
    suppressed: shortcutsSuppressed,
    paletteOpen: () => commandPaletteState.open,
    menuOpen: () => contextMenuState.open,
    setTheme: setPreference, getTheme: getPreference,
    storeTheme: value => themePreference.set(value),
    settings: () => showSettings = true,
  };
</script>
<button id="trigger" onclick={open}>Open menu</button>
<!-- svelte-ignore a11y_no_static_element_interactions -->
<div id="right-target" oncontextmenu={open}>Right click target</div>
<input id="editor" aria-label="Editor" />
<output id="theme">{$themePreference}:{$resolvedTheme}</output>
<output id="identity">{$currentUser?.display_name ?? 'none'}</output>
{#if showSettings}<Settings navigate={()=>{}} />{/if}
<CommandPalette navigate={()=>{}} />
<ContextMenu />`;

const server = await createServer({
  root,
  server: { host: "127.0.0.1", port: 0, strictPort: false },
  plugins: [{
    name: "context-menu-fixture", enforce: "pre",
    resolveId(id) { if (id === "/src/ContextMenuFixture.svelte") return fixtureId; },
    load(id) { if (id === fixtureId) return fixture; },
    configureServer(server) {
      server.middlewares.use(async (req, res, next) => {
        if (req.url?.split("?")[0] !== "/") return next();
        res.setHeader("Content-Type", "text/html");
        res.end(await server.transformIndexHtml("/", `<div id="app"></div><script type="module">import {mount} from 'svelte'; import Fixture from '/src/ContextMenuFixture.svelte'; import '/src/app.css'; mount(Fixture,{target:document.getElementById('app')});</script>`));
      });
    },
  }],
});
const deadline = setTimeout(() => { console.error("Context menu test deadline exceeded"); process.exit(1); }, 90_000);
let browser;
try {
  await server.listen();
  browser = await chromium.launch({ headless: true });
  const context = await browser.newContext({ viewport: { width: 390, height: 844 }, reducedMotion: "reduce", colorScheme: "light" });
  const page = await context.newPage();
  page.setDefaultTimeout(8_000);
  const errors: string[] = [];
  page.on("pageerror", error => errors.push(error.message));
  await page.goto(server.resolvedUrls!.local[0]);
  await page.waitForFunction(() => !!(window as any).fixture);
  const menu = page.getByRole("menu", { name: "Context menu" });
  const item = (name: string) => menu.getByRole("menuitem", { name, exact: true });
  const focused = async (name: string) => {
    await page.waitForFunction(name => document.activeElement?.textContent?.trim() === name, name);
    assert.equal(await item(name).getAttribute("tabindex"), "0");
    assert.equal(await menu.locator('[tabindex="0"]').count(), 1);
    assert.ok((await item(name).getAttribute("class"))?.includes("bg-[var(--bg-subtle)]"));
  };
  const open = async () => { await page.mouse.move(380, 800); await page.locator("#trigger").click(); await focused("First"); };
  let closeCheck = 0;
  const closedAt = async (id: string) => {
    closeCheck++;
    await menu.waitFor({ state: "hidden" });
    try { await page.waitForFunction(id => document.activeElement?.id === id, id); }
    catch (error) {
      console.error({ closeCheck, expected: id, actual: await page.evaluate(() => document.activeElement?.outerHTML) });
      throw error;
    }
  };
  await open();
  assert.ok((await item("First").boundingBox())!.height >= 44);
  await page.keyboard.press("ArrowDown"); await focused("Middle");
  await page.keyboard.press("End"); await focused("Native link");
  await page.keyboard.press("ArrowDown"); await focused("First");
  await page.keyboard.press("ArrowUp"); await focused("Native link");
  await page.keyboard.press("Home"); await focused("First");
  await item("Middle").hover(); await focused("Middle");
  await item("Disabled link").hover(); await focused("Middle");
  await page.keyboard.press("Enter"); await closedAt("trigger");
  assert.deepEqual(await page.evaluate(() => (window as any).actions), ["middle"]);

  for (const key of ["Tab", "Shift+Tab", "Escape"]) {
    await open(); await page.keyboard.press(key); await closedAt("trigger");
  }
  // A later capture listener and every bubble listener must miss the consumed Escape.
  await page.evaluate(() => {
    (window as any).escapes = 0;
    window.addEventListener("keydown", e => { if (e.key === "Escape") (window as any).escapes++; }, true);
    window.addEventListener("keydown", e => { if (e.key === "Escape") (window as any).escapes++; });
  });
  await open(); await page.keyboard.press("Escape"); await closedAt("trigger");
  assert.equal(await page.evaluate(() => (window as any).escapes), 0);
  await page.locator("#editor").focus();
  await page.locator("#right-target").click({ button: "right" }); await focused("First");
  await page.keyboard.press("Escape"); await closedAt("right-target");
  assert.equal(await page.locator("#right-target").getAttribute("tabindex"), "-1");
  await page.locator("#editor").focus();
  assert.equal(await page.locator("#right-target").getAttribute("tabindex"), null);

  await open(); await item("Focus editor").click(); await closedAt("editor");
  await page.evaluate(() => new Promise(resolve => requestAnimationFrame(() => requestAnimationFrame(resolve))));
  assert.equal(await page.locator("#editor").evaluate(el => document.activeElement === el), true);
  await open(); await page.locator("#editor").click(); await closedAt("editor");

  // Replacing an already open menu must reset actual focus even at identical coordinates.
  await open(); await item("Middle").hover();
  await page.evaluate(() => (window as any).fixture.open()); await focused("First");
  await page.evaluate(() => (window as any).fixture.open()); await focused("First");
  await page.keyboard.press("Escape");
  for (const mode of ["disabled", "empty"]) {
    await page.evaluate(mode => (window as any).fixture.open(mode), mode);
    await page.waitForFunction(() => document.activeElement?.getAttribute("role") === "menu");
    await page.keyboard.press("ArrowDown"); await page.keyboard.press("Home"); await page.keyboard.press("Enter");
    assert.equal(await page.evaluate(() => (window as any).fixture.menuOpen()), true);
    await page.keyboard.press("Escape"); await closedAt("trigger");
  }
  await open();
  assert.equal(await item("Navigate").getAttribute("href"), "#/destination");
  const popupReady = context.waitForEvent("page");
  await item("Navigate").click({ modifiers: ["Control"] });
  const popup = await popupReady; await popup.waitForLoadState();
  assert.ok(popup.url().endsWith("#/destination")); await popup.close();
  assert.deepEqual(await page.evaluate(() => (window as any).actions), ["middle"]);
  const middlePopupReady = context.waitForEvent("page");
  await item("Navigate").click({ button: "middle" });
  const middlePopup = await middlePopupReady; await middlePopup.waitForLoadState();
  assert.ok(middlePopup.url().endsWith("#/destination")); await middlePopup.close();
  assert.deepEqual(await page.evaluate(() => (window as any).actions), ["middle"]);
  await item("Navigate").click(); await closedAt("trigger");
  assert.deepEqual(await page.evaluate(() => (window as any).actions), ["middle", "navigate"]);
  assert.ok(!page.url().endsWith("#/destination"));
  await open(); await item("Native link").click(); await closedAt("trigger");
  assert.ok(page.url().endsWith("#/native"));
  await open(); await item("Navigate").focus(); await page.keyboard.press("Space"); await closedAt("trigger");
  assert.deepEqual(await page.evaluate(() => (window as any).actions), ["middle", "navigate", "navigate"]);
  await open(); await item("Focus editor").focus(); await page.keyboard.press("Space"); await closedAt("editor");
  await page.locator("#trigger").focus();

  // Shared suppression and the real palette's modifier handler agree on mobile ownership.
  await page.evaluate(() => (window as any).fixture.mobile(true));
  assert.equal(await page.evaluate(() => (window as any).fixture.suppressed()), true);
  for (const key of ["Control+k", "Control+p", "Meta+k", "Meta+p"]) await page.keyboard.press(key);
  assert.equal(await page.evaluate(() => (window as any).fixture.paletteOpen()), false);
  await page.evaluate(() => (window as any).fixture.mobile(false));
  assert.equal(await page.evaluate(() => (window as any).fixture.suppressed()), false);

  // Theme writes through either API update subscribers and the document, including OS changes.
  await page.evaluate(() => (window as any).fixture.setTheme("dark"));
  await page.waitForFunction(() => document.querySelector("#theme")?.textContent === "dark:dark");
  assert.equal(await page.evaluate(() => localStorage.getItem("lific_theme")), "dark");
  await page.evaluate(() => (window as any).fixture.storeTheme("system"));
  await page.emulateMedia({ colorScheme: "dark" });
  await page.waitForFunction(() => document.querySelector("#theme")?.textContent === "system:dark");
  await page.emulateMedia({ colorScheme: "light" });
  await page.waitForFunction(() => document.querySelector("#theme")?.textContent === "system:light");
  const other = await context.newPage(); await other.goto(server.resolvedUrls!.local[0]);
  await other.evaluate(() => localStorage.setItem("lific_theme", "dark"));
  await page.waitForFunction(() => document.querySelector("#theme")?.textContent === "dark:dark");
  assert.equal(await page.evaluate(() => document.documentElement.classList.contains("dark")), true);
  await other.evaluate(() => localStorage.clear());
  await page.waitForFunction(() => document.querySelector("#theme")?.textContent === "system:light");
  await other.close();

  // Real Settings publishes me and a successful profile response into the shell store.
  const user = { id: 1, username: "test", display_name: "Before", email: "test@example.com", is_admin: false };
  await page.route("**/api/**", async route => {
    const url = new URL(route.request().url());
    const data = url.pathname === "/api/auth/me"
      ? (route.request().method() === "PATCH" ? { ...user, ...route.request().postDataJSON() } : user)
      : url.pathname === "/api/instance" ? { web_auto_login: false } : [];
    await route.fulfill({ json: data });
  });
  await page.evaluate(() => (window as any).fixture.settings());
  await page.waitForFunction(() => document.querySelector("#identity")?.textContent === "Before");
  await page.getByLabel("Display name", { exact: true }).fill("After");
  await page.getByRole("button", { name: "Save changes", exact: true }).click();
  await page.waitForFunction(() => document.querySelector("#identity")?.textContent === "After");
  await page.evaluate(() => (window as any).fixture.setTheme("dark"));
  await page.waitForFunction(() => document.querySelector("#theme")?.textContent === "dark:dark");
  assert.ok((await page.getByRole("button", { name: "Dark", exact: true }).getAttribute("class"))?.includes("bg-[var(--surface)]"));
  assert.deepEqual(errors, []);
  console.log("Context menu: focus, disabled items, arrows/Home/End, hover/Enter, Tab, capture Escape, right-click, action focus, links, mobile guards; reactive theme/storage and Settings profile passed.");
} finally {
  await browser?.close();
  await server.close();
  clearTimeout(deadline);
}

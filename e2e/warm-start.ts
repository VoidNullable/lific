#!/usr/bin/env bun
// Real reactive read model, auth helpers and browser IndexedDB. Only HTTP is
// mocked. The finite process owns Vite and Chromium and closes both on failure.
import { strict as assert } from "node:assert";
import { resolve } from "node:path";
import { chromium, type Route } from "playwright";
import { createServer } from "../web/node_modules/vite/dist/node/index.js";

const root = resolve(import.meta.dir, "../web");
const fixtureId = root + "/src/WarmStartFixture.svelte";
const fixture = `<script>
  import {onMount} from 'svelte';
  import {ensureProjectModel} from './lib/sync/readModel.svelte';
  import * as cache from './lib/sync/persistence';
  import {setPublicProject} from './lib/publicScope';
  import {me, logout, login, saveSession, listProjects} from './lib/api';
  let model = $state(null);
  const isPublic = new URLSearchParams(location.search).has('public');
  setPublicProject(isPublic ? 'APP' : null);
  onMount(async () => {
    if (!isPublic) await me();
    model = ensureProjectModel(1);
    window.fixture = {
      cache, model: () => model, logout, listProjects,
      login: async (persistToken = true) => {
        const result = await login('other', 'password');
        if (result.ok && persistToken) saveSession(result.data.token);
        model = ensureProjectModel(1);
      },
    };
  });
</script>
{#if model}
  <div id="status">{model.status}</div>
  <div id="cursor">{model.cursor}</div>
  <ul id="issues">{#each model.issueList as row (row.id)}<li>{row.title}</li>{/each}</ul>
  <ul id="pages">{#each model.pageList as row (row.id)}<li>{row.title}</li>{/each}</ul>
{/if}`;

const server = await createServer({
  root, logLevel: "error",
  server: { host: "127.0.0.1", port: 0, strictPort: false },
  plugins: [{
    name: "warm-start-fixture", enforce: "pre",
    resolveId(id) { if (id === "/src/WarmStartFixture.svelte") return fixtureId; },
    load(id) { if (id === fixtureId) return fixture; },
    configureServer(server) {
      server.middlewares.use(async (req, res, next) => {
        if (req.url?.split("?")[0] !== "/") return next();
        res.setHeader("Content-Type", "text/html");
        res.end(await server.transformIndexHtml("/", `<div id="app"></div><script type="module">import {mount} from 'svelte'; import Fixture from '/src/WarmStartFixture.svelte'; mount(Fixture,{target:document.getElementById('app')});</script>`));
      });
    },
  }],
});

const issue = (title = "Private saved issue", seq = 10) => ({
  kind: "issue", id: 1, seq, deleted: false, identifier: "APP-1", title,
  status: "todo", priority: "none", module_id: null, sort_order: 0,
  start_date: null, target_date: null, created_at: "today", updated_at: "today",
  preview: "Summary", labels: [],
});
const pageRow = (id = 1, seq = 9) => ({
  kind: "page", id, seq, deleted: false, identifier: `APP-DOC-${id}`,
  title: `Page ${id}`, status: "draft", folder_id: null, pinned: false,
  created_at: "today", updated_at: "today", preview: "", labels: [],
});
let userId: number | null = 1;
let title = "Private saved issue";
let databaseId = "database-a";
let snapshotCursor = 10;
let indexCount = 0;
let changesCount = 0;
let delta = false;
let heldChanges: Route | null = null;
let holdChanges = false;
let heldIndex: Route | null = null;
let holdIndex = false;
const calls: string[] = [];
const key = "private:1:1";
let browser: Awaited<ReturnType<typeof chromium.launch>> | undefined;
const errors: string[] = [];
let passed = 0;
function pass(name: string) { passed++; console.log(`PASS ${name}`); }

try {
  await server.listen();
  const address = server.httpServer!.address();
  assert(address && typeof address !== "string");
  const origin = `http://127.0.0.1:${address.port}`;
  browser = await chromium.launch({
    headless: true,
    executablePath: process.env.PLAYWRIGHT_CHROMIUM_EXECUTABLE_PATH,
  });
  const context = await browser.newContext();
  await context.route("**/api/**", async route => {
    const url = new URL(route.request().url());
    calls.push(url.pathname + url.search);
    if (url.pathname === "/api/auth/me") {
      if (userId === null) return route.fulfill({ status: 401, json: { error: "not signed in" } });
      return route.fulfill({ json: {
        id: userId, username: `user${userId}`, display_name: "", email: "", is_admin: false,
      } });
    }
    if (url.pathname === "/api/auth/login") return route.fulfill({ json: {
      user: { id: userId }, token: `token-${userId}`, expires_at: "2099-01-01",
    } });
    if (url.pathname === "/api/auth/logout") return route.fulfill({ json: {} });
    if (url.pathname === "/api/projects") return route.fulfill({ json: [{ id: 1, identifier: "APP" }] });
    if (url.pathname.endsWith("/index")) {
      indexCount++;
      if (holdIndex) { heldIndex = route; return; }
      return route.fulfill({ json: { database_id: databaseId, cursor: snapshotCursor,
        issues: [issue(title, snapshotCursor)], pages: [pageRow()] } });
    }
    if (url.pathname.endsWith("/changes")) {
      changesCount++;
      if (holdChanges) { heldChanges = route; return; }
      const since = Number(url.searchParams.get("since"));
      return route.fulfill({ json: delta && since === 12
        ? { database_id: databaseId, cursor: 13, changes: [pageRow(2, 13)], has_more: false }
        : { database_id: databaseId, cursor: since, changes: [], has_more: false } });
    }
    throw new Error(`Unexpected request: ${url}`);
  });
  const page = await context.newPage();
  page.on("pageerror", error => errors.push(error.message));
  const ready = () => page.waitForFunction(() => (window as any).fixture?.model().status === "ready");
  const durable = async (cursor: number, cacheKey = key) => {
    const deadline = Date.now() + 10_000;
    while ((await read(cacheKey)).snapshot?.cursor !== cursor) {
      assert(Date.now() < deadline, `Timed out waiting for durable cursor ${cursor}`);
      await new Promise(resolve => setTimeout(resolve, 20));
    }
  };
  const read = (cacheKey = key) => page.evaluate(async cacheKey =>
    (window as any).fixture.cache.loadSnapshot(cacheKey), cacheKey);
  const waitHeld = async (kind: "index" | "changes") => {
    const deadline = Date.now() + 10_000;
    while (!(kind === "index" ? heldIndex : heldChanges)) {
      assert(Date.now() < deadline, `Timed out waiting for ${kind}`);
      await new Promise(resolve => setTimeout(resolve, 20));
    }
  };
  await page.goto(origin);
  await ready();
  await durable(10);
  assert.equal(indexCount, 1);
  const first = await read();
  assert(first.snapshot, JSON.stringify({ first, modelKey: await page.evaluate(() => (window as any).fixture.model().key), calls }));
  assert.equal(first.snapshot.issues[0].title, title);
  pass("cold start uses /index and writes issues, pages and cursor");

  const beforeReload = indexCount;
  holdChanges = true;
  await page.reload();
  await ready();
  await waitHeld("changes");
  assert.equal(await page.locator("#issues").textContent(), title);
  assert.equal(await page.locator("#pages").textContent(), "Page 1");
  assert.equal(indexCount, beforeReload);
  assert.equal(new URL(heldChanges!.request().url()).searchParams.get("since"), "10");
  pass("reload paints IndexedDB rows while /changes is held, without /index");

  holdChanges = false;
  delta = true;
  await heldChanges!.fulfill({ json: {
    database_id: databaseId,
    cursor: 12, has_more: true,
    changes: [issue("Updated issue", 11), { kind: "page", id: 1, seq: 12, deleted: true }],
  } });
  heldChanges = null;
  await durable(13);
  assert.equal(await page.locator("#issues").textContent(), "Updated issue");
  assert.equal(await page.locator("#pages").textContent(), "Page 2");
  const committed = await read();
  assert.deepEqual(committed.snapshot.pages.map((row: any) => row.id), [2]);
  pass("paged catch-up commits upserts, removals and the final cursor together");

  for (const failure of ["abort", "quota"]) {
    const result = await page.evaluate(async ({ key, failure }) => {
      const cache = (window as any).fixture.cache;
      const before = await cache.loadSnapshot(key);
      const original = IDBObjectStore.prototype.put;
      IDBObjectStore.prototype.put = function(...args) {
        if (this.name !== "snapshots") return original.apply(this, args as any);
        if (failure === "quota") throw new DOMException("Quota exhausted", "QuotaExceededError");
        const result = original.apply(this, args as any);
        this.transaction.abort();
        return result;
      };
      try {
        const saved = await cache.storeSnapshot(key, { database_id: before.snapshot.database_id, cursor: 99, issues: [], pages: [] }, before.epoch);
        return { saved, after: await cache.loadSnapshot(key) };
      } finally { IDBObjectStore.prototype.put = original; }
    }, { key, failure });
    assert.equal(result.saved, false);
    assert.deepEqual(result.after, committed);
  }
  pass("transaction abort and quota failure cannot advance cursor or lose durable rows");

  assert.equal(await page.evaluate(async key => {
    const cache = (window as any).fixture.cache;
    const { epoch, snapshot } = await cache.loadSnapshot(key);
    return cache.storeSnapshot(key, { database_id: snapshot.database_id, cursor: 1, issues: [], pages: [] }, epoch);
  }, key), false);
  assert.deepEqual(await read(), committed);
  pass("a slower tab cannot replace a newer durable replica");

  await page.evaluate(() => {
    const f = (window as any).fixture;
    f.writes = 0;
    f.originalPut = IDBObjectStore.prototype.put;
    IDBObjectStore.prototype.put = function(...args) {
      if (this.name === "snapshots") f.writes++;
      return f.originalPut.apply(this, args);
    };
    f.model().applyChanges({ cursor: 14, changes: [{
      ...f.model().issues.get(1), id: 99, identifier: "APP-99", seq: 14,
      title: "Ghost from the lost backup window",
    }], has_more: true });
    f.model().applyChanges({ cursor: 15, changes: [
      { kind: "page", id: 2, seq: 15, deleted: true },
    ], has_more: false });
  });
  assert.equal((await read()).snapshot.cursor, 13);
  await durable(15);
  assert.equal(await page.evaluate(() => {
    const f = (window as any).fixture;
    IDBObjectStore.prototype.put = f.originalPut;
    return f.writes;
  }), 1);
  assert.deepEqual((await read()).snapshot.pages, []);
  pass("a burst updates memory immediately and coalesces into one write-behind commit");

  holdChanges = true;
  await page.reload();
  await ready();
  await waitHeld("changes");
  const preRestore = await read();
  assert.equal(preRestore.snapshot.issues.length, 2);
  const beforeRestore = indexCount;
  snapshotCursor = 3;
  title = "Restored issue";
  holdChanges = false;
  await heldChanges!.fulfill({ json: {
    database_id: databaseId, reset: true, cursor: 0, changes: [], has_more: false,
  } });
  heldChanges = null;
  await durable(3);
  assert.equal(indexCount, beforeRestore + 1);
  assert.equal(await page.locator("#issues").textContent(), title);
  assert.equal((await read()).snapshot.issues[0].seq, 3);
  assert.equal(await page.evaluate(async ({ key, stale }) =>
    (window as any).fixture.cache.storeSnapshot(key, stale.snapshot, stale.epoch),
  { key, stale: preRestore }), false);
  pass("restore reset discards high-sequence rows and cursor, bootstraps /index and fences old writers");

  holdChanges = true;
  await page.reload();
  await ready();
  await waitHeld("changes");
  const beforeReplacement = indexCount;
  databaseId = "database-b";
  snapshotCursor = 20;
  title = "Replacement database";
  holdChanges = false;
  await heldChanges!.fulfill({ json: {
    database_id: databaseId, reset: false, cursor: 30,
    changes: [issue("Must not merge foreign delta", 30)], has_more: false,
  } });
  heldChanges = null;
  await durable(20);
  assert.equal(indexCount, beforeReplacement + 1);
  assert.equal(await page.locator("#issues").textContent(), title);
  assert.equal((await read()).snapshot.database_id, databaseId);
  pass("a different database with a higher counter is a cache miss, not a delta merge");
  snapshotCursor = 10;
  title = "Private saved issue";

  // Invalid data is rejected as a unit, including a version mismatch.
  for (const corrupt of [{ version: 999 }, { issues: [{ id: 1 }] }]) {
    await page.evaluate(async ({ key, corrupt }) => {
      const { snapshot } = await (window as any).fixture.cache.loadSnapshot(key);
      await new Promise<void>((resolve, reject) => {
        const request = indexedDB.open("lific-read-model", 1);
        request.onsuccess = () => {
          const db = request.result;
          const tx = db.transaction("snapshots", "readwrite");
          tx.objectStore("snapshots").put({ ...snapshot, ...corrupt }, key);
          tx.oncomplete = () => { db.close(); resolve(); };
          tx.onabort = () => { db.close(); reject(tx.error); };
        };
      });
    }, { key, corrupt });
    const before = indexCount;
    await page.reload();
    await ready();
    await durable(10);
    assert.equal(indexCount, before + 1);
    assert.equal(await page.locator("#issues").textContent(), title);
  }
  pass("corrupt and version-mismatched records cold-bootstrap and repair the cache");

  // Switch account without clearing disk to prove the key itself isolates it.
  const previousAccountTab = await context.newPage();
  await previousAccountTab.goto(origin);
  await previousAccountTab.waitForFunction(() => (window as any).fixture?.model().status === "ready");
  userId = 2;
  holdIndex = true;
  // No localStorage write: cookie-auth siblings must receive the change too.
  const switching = page.evaluate(() => (window as any).fixture.login(false));
  await waitHeld("index");
  await previousAccountTab.waitForFunction(() => (window as any).fixture.model().issues.size === 0);
  await previousAccountTab.close();
  assert.equal(await page.locator("#issues").textContent(), "");
  assert.equal((await read()).snapshot.issues[0].title, "Private saved issue");
  holdIndex = false;
  await heldIndex!.fulfill({ json: { database_id: databaseId, cursor: 10, issues: [issue("Other account")], pages: [] } });
  heldIndex = null;
  await switching;
  await durable(10, "private:2:1");
  assert.equal(await page.locator("#issues").textContent(), "Other account");
  pass("same project under another account cannot hydrate the first account's rows");

  const pruneSeed = await page.evaluate(async () => {
    const cache = (window as any).fixture.cache;
    const source = await cache.loadSnapshot("private:2:1");
    const result: Record<string, any> = {};
    for (const key of ["private:2:99", "private:2:88", "private:1:99", "public:signed-out:APP:99"]) {
      const { epoch } = await cache.loadSnapshot(key);
      result[key] = { snapshot: source.snapshot, epoch };
      if (key !== "private:2:88") await cache.storeSnapshot(key, source.snapshot, epoch);
    }
    return result;
  });
  await page.evaluate(() => (window as any).fixture.listProjects());
  const storedKeys = await page.evaluate(() => new Promise<IDBValidKey[]>((resolve, reject) => {
    const request = indexedDB.open("lific-read-model", 1);
    request.onsuccess = () => {
      const db = request.result;
      const tx = db.transaction("snapshots", "readonly");
      const keys = tx.objectStore("snapshots").getAllKeys();
      tx.oncomplete = () => { db.close(); resolve(keys.result); };
      tx.onabort = () => { db.close(); reject(tx.error); };
    };
    request.onerror = () => reject(request.error);
  }));
  assert(!storedKeys.includes("private:2:99"));
  assert(storedKeys.includes("private:1:99"));
  assert.equal((await read("private:2:99")).snapshot, null);
  assert((await read("private:2:1")).snapshot);
  assert((await read("private:1:99")).snapshot);
  assert((await read("public:signed-out:APP:99")).snapshot);
  for (const removed of ["private:2:99", "private:2:88"]) {
    assert.equal(await page.evaluate(async ({ key, stale }) =>
      (window as any).fixture.cache.storeSnapshot(key, stale.snapshot, stale.epoch),
    { key: removed, stale: pruneSeed[removed] }), false);
  }
  pass("project-list reconciliation prunes only the current scope and rejects queued writes for removed projects");

  const sibling = await context.newPage();
  sibling.on("pageerror", error => errors.push(error.message));
  await sibling.goto(origin);
  await sibling.waitForFunction(() => (window as any).fixture?.model().status === "ready");
  const stale = await read("private:2:1");
  await page.evaluate(() => (window as any).fixture.logout());
  await sibling.waitForFunction(() => (window as any).fixture.model().issues.size === 0);
  assert.equal((await read()).snapshot, null);
  assert.equal((await read("private:2:1")).snapshot, null);
  assert.equal(await sibling.evaluate(async stale =>
    (window as any).fixture.cache.storeSnapshot("private:2:1", stale.snapshot, stale.epoch), stale), false);
  await sibling.close();
  pass("sign-out clears all cached accounts and fences another tab's queued write");

  // A request started before sign-out must not repopulate memory or disk.
  holdIndex = true;
  await page.reload();
  await waitHeld("index");
  await page.evaluate(() => (window as any).fixture.logout());
  holdIndex = false;
  await heldIndex!.fulfill({ json: { database_id: databaseId, cursor: 100, issues: [issue("Late private response", 100)], pages: [] } });
  heldIndex = null;
  await page.waitForTimeout(400);
  assert.equal(await page.locator("#issues").textContent(), "");
  assert.equal((await read("private:2:1")).snapshot, null);
  pass("late /index response after sign-out cannot restore cleared data");

  userId = 1;
  await page.reload();
  await ready();
  await durable(10);
  const privateCalls = calls.filter(call => call.startsWith("/api/")).length;
  holdIndex = true;
  await page.goto(`${origin}/?public`);
  await waitHeld("index");
  assert.equal(await page.locator("#issues").textContent(), "");
  assert.equal(calls.filter(call => call.startsWith("/api/")).length, privateCalls);
  holdIndex = false;
  await heldIndex!.fulfill({ json: { database_id: databaseId, cursor: 10, issues: [issue("Public summary")], pages: [] } });
  heldIndex = null;
  await ready();
  await durable(10, "public:signed-out:APP:1");
  assert.equal(await page.locator("#issues").textContent(), "Public summary");
  pass("signed-out public scope never hydrates private data or calls private APIs");

  const anonymousSeed = await page.evaluate(async () => {
    const cache = (window as any).fixture.cache;
    const source = await cache.loadSnapshot("private:1:1");
    for (const key of ["private:9:1", "private:signed-out:99"]) {
      const { epoch } = await cache.loadSnapshot(key);
      await cache.storeSnapshot(key, source.snapshot, epoch);
    }
    return source;
  });
  // The normal-URL public-reader branch confirms signed-out through /auth/me.
  userId = null;
  holdIndex = true;
  await page.goto(origin);
  await waitHeld("index");
  assert.equal(await page.locator("#issues").textContent(), "");
  assert.equal((await read("private:1:1")).snapshot, null);
  assert.equal((await read("private:9:1")).snapshot, null);
  assert((await read("private:signed-out:99")).snapshot);
  assert((await read("public:signed-out:APP:1")).snapshot);
  assert.equal(await page.evaluate(async stale =>
    (window as any).fixture.cache.storeSnapshot("private:1:1", stale.snapshot, stale.epoch),
  anonymousSeed), false);
  pass("anonymous /auth/me purges every private account, keeps signed-out scopes and fences private writers");
  holdIndex = false;
  await heldIndex!.fulfill({ json: { database_id: databaseId, cursor: 10, issues: [issue("Signed-out summary")], pages: [] } });
  heldIndex = null;
  await ready();
  await durable(10, "private:signed-out:1");
  const beforeSignedOutReload = indexCount;
  await page.reload();
  await ready();
  assert.equal(indexCount, beforeSignedOutReload);
  assert.equal(await page.locator("#issues").textContent(), "Signed-out summary");
  pass("confirmed signed-out readers get their own warm scope without erasing it on /auth/me 401");
  userId = 1;

  await page.evaluate(() => new Promise<void>((resolve, reject) => {
    const request = indexedDB.open("lific-read-model", 2);
    request.onsuccess = () => { request.result.close(); resolve(); };
    request.onerror = () => reject(request.error);
  }));
  const beforeFutureSchema = indexCount;
  await page.goto(origin);
  await ready();
  assert.equal(indexCount, beforeFutureSchema + 1);
  assert.equal(await page.locator("#issues").textContent(), title);
  pass("a newer IndexedDB schema falls back to /index without blocking startup");

  await page.addInitScript(() => {
    IDBFactory.prototype.open = () => { throw new DOMException("Unavailable", "SecurityError"); };
  });
  const beforeUnavailable = indexCount;
  await page.goto(origin);
  await ready();
  assert.equal(indexCount, beforeUnavailable + 1);
  assert.equal(await page.locator("#issues").textContent(), title);
  pass("unavailable IndexedDB falls back to the server");

  assert.deepEqual(errors, []);
  console.log(`${passed} browser checks passed; ${indexCount} snapshots, ${changesCount} catch-up requests; no browser errors.`);
} finally {
  await browser?.close();
  await server.close();
}

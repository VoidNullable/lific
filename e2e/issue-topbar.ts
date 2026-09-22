#!/usr/bin/env bun
// Real issue toolbar and state, measured at the problematic small-tablet widths.
import { strict as assert } from "node:assert";
import { resolve } from "node:path";
import { chromium } from "playwright";
import { createServer } from "../web/node_modules/vite/dist/node/index.js";

const root = resolve(import.meta.dir, "../web");
const fixtureId = root + "/src/IssueTopbarFixture.svelte";
const fixture = `<script>
  import Topbar from './lib/issues/Topbar.svelte';
  import { IssueListState } from './lib/issues/state.svelte';
  const view = new IssueListState();
  let searchInputEl = $state(null);
  window.createCount = 0;
</script>
<main data-topbar>
<Topbar {view} projectIdentifier="TST" layout="list" navigate={() => {}}
  statusCounts={[{status:'todo',count:25},{status:'active',count:35},{status:'done',count:40}]}
  countLabel="100 issues" changedCount={123} labels={[]} modules={[]} priorityCssColor={() => ''}
  bind:searchInputEl onOpenSearch={() => view.searchExpanded = true}
  onMaybeCollapseSearch={() => view.searchExpanded = false}
  onQuickCreate={() => window.createCount++} />
</main>`;
const originalHtml = await Bun.file(root + "/index.html").text();
const html = originalHtml.replace('<script type="module" src="/src/main.ts"></script>', `<script type="module">import {mount} from 'svelte'; import Fixture from '/src/IssueTopbarFixture.svelte'; import '/src/app.css'; mount(Fixture,{target:document.getElementById('app')});</script>`);
const server = await createServer({ root, logLevel: "error", server: { host: "127.0.0.1", port: 0, strictPort: false }, plugins: [{
  name: "issue-topbar-fixture", enforce: "pre",
  resolveId(id) { if (id === "/src/IssueTopbarFixture.svelte") return fixtureId; },
  load(id) { if (id === fixtureId) return fixture; },
  configureServer(vite) { vite.middlewares.use(async (req, res, next) => {
    if (req.url?.split('?')[0] !== '/') return next();
    res.setHeader('Content-Type', 'text/html'); res.end(await vite.transformIndexHtml('/', html));
  }); },
}] });
const deadline = setTimeout(() => { console.error("Topbar test deadline exceeded"); process.exit(1); }, 90_000);
let browser;
try {
  await server.listen();
  const executablePath = process.env.PLAYWRIGHT_EXECUTABLE_PATH;
  browser = await chromium.launch(
    executablePath
      ? { headless: true, executablePath }
      : { headless: true, channel: "chromium" },
  );
  const page = await browser.newPage({ reducedMotion: "reduce" });
  page.setDefaultTimeout(8_000);
  const errors: string[] = [];
  page.on('pageerror', error => errors.push(error.message));
  await page.route('**/api/**', route => {
    const path = new URL(route.request().url()).pathname;
    return route.fulfill({ json: path === '/api/auth/me' ? { id: 1, username: 'fixture' } : path === '/api/projects' ? [{id:1,identifier:'TST',name:'Test'}] : [] });
  });
  const measurements = [];
  for (const width of [360, 640, 700, 767, 1200]) {
    await page.setViewportSize({ width, height: 900 });
    await page.goto(server.resolvedUrls!.local[0]);
    await page.getByRole('button', { name: 'Filter', exact: true }).waitFor();
    await page.evaluate(() => document.fonts.ready);
    const list = await page.getByRole('button', { name: 'List', exact: true }).boundingBox();
    const filter = await page.getByRole('button', { name: 'Filter', exact: true }).boundingBox();
    const create = await page.getByRole('button', { name: 'More create options', exact: true }).boundingBox();
    const ys = [list!, filter!, create!].map(box => box.y + box.height / 2);
    const spread = Math.max(...ys) - Math.min(...ys);
    measurements.push({ width, centerSpread: spread, documentWidth: await page.evaluate(() => document.documentElement.scrollWidth) });
    assert.ok(spread <= 1, `Toolbar wrapped at ${width}px: ${JSON.stringify(measurements)}`);
    assert.equal(await page.evaluate(() => document.documentElement.scrollWidth), width);
    if (width < 768) {
      await page.getByRole('button', { name: 'View options', exact: true }).click();
      assert.equal(await page.getByRole('button', { name: 'View options', exact: true }).getAttribute('aria-expanded'), 'true');
      await page.locator('#issue-view-options').getByRole('button', { name: 'Display', exact: true }).waitFor();
      await page.waitForFunction(() => document.querySelector('#issue-view-options')?.contains(document.activeElement));
      await page.getByRole('button', { name: 'View options', exact: true }).click();
    }
    await page.getByRole('button', { name: 'More create options', exact: true }).click();
    await page.getByRole('menuitem', { name: /^Quick create/ }).click();
    assert.equal(await page.evaluate(() => (window as any).createCount), 1);
    await page.getByRole('button', { name: 'New issue', exact: true }).click();
    assert.equal(await page.evaluate(() => (window as any).createCount), 2);
    await page.getByRole('button', { name: 'Search issues', exact: true }).click();
    const search = page.getByPlaceholder('Search issues...');
    await search.fill('example');
    const box = (await search.boundingBox())!;
    assert.ok(box.x >= 0 && box.x + box.width <= width);
    await search.press('Escape');
    await search.waitFor({ state: 'hidden' });
    if (width === 640 && process.env.E2E_SCREENSHOT) await page.screenshot({ path: process.env.E2E_SCREENSHOT });
  }
  assert.deepEqual(errors, []);
  console.log(JSON.stringify(measurements));
} finally { await browser?.close(); await server.close(); clearTimeout(deadline); }

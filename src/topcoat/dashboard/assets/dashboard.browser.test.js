const {test} = require('node:test');
const assert = require('node:assert/strict');
const fs = require('node:fs');
const path = require('node:path');

test('headless home and overview render private work and respond to keyboard, live updates and session transitions',
  {skip: !process.env.PLAYWRIGHT_EXECUTABLE_PATH}, async t => {
    const {chromium} = await import(path.resolve(__dirname, '../../../..', 'e2e/node_modules/playwright/index.mjs'));
    const browser = await chromium.launch({headless: true, executablePath: process.env.PLAYWRIGHT_EXECUTABLE_PATH});
    const page = await browser.newPage();
    page.setDefaultTimeout(5000);
    const errors = [];
    page.on('pageerror', error => errors.push(error.stack || error.message));
    const script = fs.readFileSync(`${__dirname}/dashboard.js`, 'utf8');
    const syncScript = fs.readFileSync(`${__dirname}/../../assets/sync.js`, 'utf8');
    const css = fs.readFileSync(`${__dirname}/dashboard.css`, 'utf8');
    const frame = (mode, identifier = '') => `<!doctype html><html lang="en"><head><title>Dashboard fixture</title><style>${css}</style></head>
      <body><section class="tc-dashboard" data-topcoat-dashboard="${mode}" data-project-identifier="${identifier}" aria-busy="true">
        <p data-dashboard-status role="status" aria-live="polite">Loading your dashboard…</p>
        <div data-dashboard-errors role="status" aria-live="polite"></div>
        <div data-dashboard-content><h1>Loading</h1></div></section></body></html>`;
    async function mount(mode, identifier = '') {
      await page.route('http://lific.test/**', route => route.fulfill({contentType: 'text/html', body: frame(mode, identifier)}));
      await page.goto(`http://lific.test/${identifier ? `${identifier}/overview` : ''}`);
      await page.evaluate(() => {
        window.requests = [];
        window.revoked = false;
        window.failProjects = false;
        window.done = 5;
        window.role = 'maintainer';
        window.lificSession = {state: {publicProject: null, user: {id: 1, username: 'Reader'}},
          loadRole(id) {return this.request(`/projects/${id}/my-role`);},
          async request(path) {
            requests.push(path);
            const good = data => ({ok: true, data});
            if (path === '/projects') return failProjects ? {ok: false, status: 500, error: 'Projects offline'}
              : good([{id: 1, identifier: 'LIF', name: 'Lific <b>project</b>', description: 'Description', updated_at: '2026-10-02'},
                {id: 2, identifier: 'SEM', name: 'Semantic', updated_at: '2026-10-01'}]);
            if (path === '/project-groups') return good([{id: 1, name: 'Work', sort_order: 0, project_ids: [2, 1]}]);
            if (path === '/pages') return good([{id: 9, project_id: 1, title: 'Pinned <i>page</i>', pinned: true, updated_at: '2026-10-02'}]);
            if (path.includes('/my-role')) return good({role: window.role, enforced: true, is_admin: false});
            if (path.includes('/issue-counts')) return revoked ? {ok: false, status: 403, error: 'Access revoked'} : good({total: 10, done});
            if (path.includes('/activity')) return good({items: [{id: 1, project_id: 1, actor_username: 'Reader', action: 'update', entity_type: 'issue', entity_label: 'LIF-1', ts: '2026-10-02'}]});
            if (path.startsWith('/issues')) return good([{id: path.includes('status=todo') ? 2 : 1, project_id: 1,
              identifier: path.includes('status=todo') ? 'LIF-2' : 'LIF-1', title: 'Issue <script>bad</script>',
              status: path.includes('status=todo') ? 'todo' : 'active', priority: 'high', created_at: '2026-10-01', updated_at: '2026-10-02'}]);
            throw new Error(`Unexpected request ${path}`);
          }};
        window.lificSync = {state: {activityBaseline: 12}, setActiveProject(id) {window.activeProject = id;}};
        localStorage.setItem('lific_recents', JSON.stringify([{type: 'page', project: 'LIF', routeId: '4', title: 'Recent page', ts: 1}]));
      });
      await page.addScriptTag({content: script});
      await page.waitForFunction(() => lificDashboard.controller.state.status === 'ready');
    }
    try {
      await t.test('home preserves safe titles, group project order, pinned/recent numeric links and quick actions', async () => {
        await mount('home');
        assert.equal(await page.getByRole('heading', {name: 'My active issues (2)'}).count(), 1);
        assert.equal(await page.getByRole('link', {name: 'Pinned <i>page</i>'}).getAttribute('href'), '/LIF/pages/9');
        assert.equal(await page.getByRole('link', {name: 'Recent page · LIF'}).getAttribute('href'), '/LIF/pages/4');
        assert.equal(await page.getByRole('link', {name: 'New issue'}).getAttribute('href'), '/LIF/issues/new');
        const projects = page.getByRole('heading', {name: 'Projects', exact: true}).locator('..');
        assert.deepEqual(await projects.locator('a').allTextContents(), ['Lific <b>project</b> · LIF', 'Semantic · SEM']);
        assert.equal(await page.locator('[data-dashboard-content] b, [data-dashboard-content] i, [data-dashboard-content] script').count(), 0);
        await page.setViewportSize({width: 375, height: 812});
        assert.equal(await page.evaluate(() => document.documentElement.scrollWidth <= innerWidth), true);
      });

      await t.test('refresh retains keyboard focus and reads the shared activity baseline', async () => {
        const link = page.getByRole('link', {name: 'Pinned <i>page</i>'});
        await link.focus();
        await page.evaluate(async () => {dispatchEvent(new CustomEvent('lific:sync-change')); await lificDashboard.refresh();});
        assert.equal(await link.evaluate(node => document.activeElement === node), true);
        assert.match(await page.locator('[data-dashboard-activity-rate]').textContent(), /12 updates\/day/);
      });

      await t.test('overview deep links show counts, live metrics, attention and role-sensitive controls', async () => {
        await mount('overview', 'LIF');
        assert.equal(await page.getByRole('heading', {name: 'Lific <b>project</b>'}).count(), 1);
        assert.equal(await page.getByRole('progressbar').getAttribute('aria-label'), '5 of 10 issues done');
        assert.equal(await page.getByRole('link', {name: 'Project settings'}).count(), 0);
        const link = page.getByRole('link', {name: 'New issue'});
        await link.focus();
        await page.evaluate(() => {done = 7; dispatchEvent(new CustomEvent('lific:realtime', {detail: {type: 'issue.updated', project_id: 1}}));});
        await page.waitForFunction(() => document.querySelector('progress')?.value === 7);
        assert.equal(await link.evaluate(node => document.activeElement === node), true);
        await page.evaluate(async () => {role = 'viewer'; await lificDashboard.refresh();});
        assert.equal(await page.getByRole('link', {name: 'New issue'}).count(), 0);
        await page.evaluate(async () => {role = 'lead'; await lificDashboard.refresh();});
        assert.equal(await page.getByRole('link', {name: 'Project settings'}).getAttribute('href'), '/LIF/settings');
      });

      await t.test('overview access revocation clears private data and retry reloads it', async () => {
        await page.evaluate(async () => {revoked = true; await lificDashboard.refresh();});
        assert.equal(await page.getByRole('progressbar').count(), 0);
        assert.equal(await page.getByRole('heading', {name: 'Needs attention'}).count(), 0);
        assert.equal(await page.getByText('Access revoked').count(), 1);
        await page.evaluate(() => {revoked = false;});
        await page.getByRole('button', {name: 'Try again'}).click();
        await page.getByRole('progressbar').waitFor();
      });

      await t.test('real sync client immediately invalidates overview on resync before any baseline exists', async () => {
        await page.evaluate(() => {
          localStorage.setItem('lific_token', 'test-token');
          window.WebSocket = class {
            constructor() {window.testSocket = this; this.listeners = {}; this.frames = []; this.readyState = 0;
              queueMicrotask(() => {this.readyState = 1; this.emit('open');});}
            addEventListener(name, callback) {this.listeners[name] = callback;}
            emit(name, detail) {this.listeners[name]?.(detail);}
            send(frame) {this.frames.push(JSON.parse(frame));}
            close() {this.readyState = 3;}
          };
        });
        await page.addScriptTag({content: syncScript});
        await page.waitForFunction(() => window.testSocket?.readyState === 1);
        const invalidated = await page.evaluate(() => {
          window.originalResyncRequest = lificSession.request;
          lificSession.request = (path, options) => path === '/projects'
            ? new Promise(resolve => {window.resolveResyncProjects = resolve;}) : originalResyncRequest(path, options);
          done = 8;
          testSocket.emit('message', {data: JSON.stringify({type: 'resync.required'})});
          return {status: lificDashboard.controller.state.status, metric: !!document.querySelector('progress'),
            requestsBaseline: testSocket.frames.filter(frame => frame.type === 'activity.baseline.request').length};
        });
        assert.equal(invalidated.status, 'loading');
        assert.equal(invalidated.metric, false);
        assert.ok(invalidated.requestsBaseline >= 2);
        await page.evaluate(async () => {resolveResyncProjects(await originalResyncRequest('/projects')); lificSession.request = originalResyncRequest;});
        await page.waitForFunction(() => document.querySelector('progress')?.value === 8);
      });

      await t.test('cached history restores continue to refresh after pagehide/pageshow', async () => {
        await page.evaluate(() => {
          dispatchEvent(new PageTransitionEvent('pagehide', {persisted: true}));
          done = 9;
          dispatchEvent(new PageTransitionEvent('pageshow', {persisted: true}));
        });
        await page.waitForFunction(() => document.querySelector('progress')?.value === 9);
      });

      await t.test('public/account transitions erase private cards and stop private requests', async () => {
        const before = await page.evaluate(() => requests.length);
        await page.evaluate(() => {
          lificSession.state.publicProject = 'LIF';
          dispatchEvent(new CustomEvent('lific:scope-change'));
          dispatchEvent(new CustomEvent('lific:realtime', {detail: {type: 'issue.updated', project_id: 1}}));
        });
        assert.equal(await page.locator('[data-dashboard-content]').textContent(), '');
        assert.equal(await page.evaluate(() => requests.length), before);
        assert.equal(await page.evaluate(() => lificDashboard.controller.activityRate().value), 0);
      });

      await t.test('a newly authenticated account sees the loading frame while its dashboard is fetched', async () => {
        await page.evaluate(() => {
          window.originalRequest = lificSession.request;
          lificSession.request = (path, options) => path === '/projects'
            ? new Promise(resolve => {window.resolveProjects = resolve;}) : originalRequest(path, options);
          lificSession.state.publicProject = null;
          lificSession.state.user = {id: 2, username: 'Second reader'};
          dispatchEvent(new CustomEvent('lific:account-change'));
        });
        assert.equal(await page.locator('[data-topcoat-dashboard]').getAttribute('aria-busy'), 'true');
        assert.equal(await page.locator('.tc-dashboard__skeleton').count(), 1);
        await page.evaluate(async () => {resolveProjects(await originalRequest('/projects')); lificSession.request = originalRequest;});
        await page.getByRole('progressbar').waitFor();
      });
      assert.deepEqual(errors, []);
    } finally {await browser.close();}
  });

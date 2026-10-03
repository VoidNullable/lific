const {test} = require('node:test');
const assert = require('node:assert/strict');
const fs = require('node:fs');
const path = require('node:path');

test('headless dashboard uses the real session role cache and Topcoat mascot asset',
  {skip: !process.env.PLAYWRIGHT_EXECUTABLE_PATH}, async t => {
    const {chromium} = await import(path.resolve(__dirname, '../../../..', 'e2e/node_modules/playwright/index.mjs'));
    const browser = await chromium.launch({headless: true, executablePath: process.env.PLAYWRIGHT_EXECUTABLE_PATH});
    const page = await browser.newPage();
    page.setDefaultTimeout(5000);
    const sessionSource = fs.readFileSync(path.resolve(__dirname, '../../session.rs'), 'utf8');
    const sessionScript = sessionSource.split('pub(crate) const BROWSER_SCRIPT: &str = r#"')[1].split('"#;')[0];
    const dashboardScript = fs.readFileSync(`${__dirname}/dashboard.js`, 'utf8');
    const pageErrors = [];
    page.on('pageerror', error => pageErrors.push(error.message));
    let role = 'lead';
    let mode = 'overview';
    const requests = [];
    const html = () => `<!doctype html><html lang="en"><head><title>Session dashboard fixture</title></head>
      <body data-lific-require-session="true" data-lific-project-id="1">
        <button data-lific-capability="manage">Shared manage action</button>
        <section data-topcoat-dashboard="${mode}" data-project-identifier="${mode === 'overview' ? 'LIF' : ''}">
          <p data-dashboard-status></p><div data-dashboard-errors></div><div data-dashboard-content></div>
        </section></body></html>`;
    try {
      await page.route('http://lific.test/**', async route => {
        const pathname = new URL(route.request().url()).pathname;
        requests.push(pathname);
        let data;
        if (pathname === '/api/auth/me') data = {id: 1, username: 'Reader', is_admin: false};
        else if (pathname === '/api/projects') data = [{id: 1, identifier: 'LIF', name: 'Lific', updated_at: '2026-10-02'}];
        else if (pathname === '/api/projects/1/my-role') data = {role, enforced: true, is_admin: false};
        else if (pathname === '/api/projects/1/issue-counts') data = {done: 2, total: 4};
        else if (pathname === '/api/projects/1/activity') data = {items: []};
        else if (pathname.startsWith('/api/')) data = [];
        else if (pathname === '/__topcoat-dashboard-mascot.png' && fs.existsSync(`${__dirname}/sleeping-lizzy.png`)) {
          await route.fulfill({contentType: 'image/png', body: fs.readFileSync(`${__dirname}/sleeping-lizzy.png`)}); return;
        } else if (pathname.endsWith('.png')) {await route.fulfill({status: 404, body: 'Not found'}); return;}
        else {await route.fulfill({contentType: 'text/html', body: html()}); return;}
        await route.fulfill({contentType: 'application/json', body: JSON.stringify(data)});
      });

      await t.test('a fresh viewer response replaces a cached lead role in shared controls and subsequent cache reads', async () => {
        await page.goto('http://lific.test/LIF/overview');
        await page.evaluate(() => localStorage.setItem('lific_token', 'test-token'));
        await page.addScriptTag({content: sessionScript});
        await page.waitForFunction(() => !lificSession.state.loading && lificSession.state.role.role === 'lead');
        await page.addScriptTag({content: dashboardScript});
        await page.getByRole('link', {name: 'Project settings'}).waitFor();
        role = 'viewer';
        await page.evaluate(() => lificDashboard.refresh());
        assert.equal(await page.getByRole('link', {name: 'Project settings'}).count(), 0);
        assert.equal(await page.getByRole('button', {name: 'Shared manage action'}).isVisible(), false);
        assert.equal(await page.evaluate(() => lificSession.state.role.role), 'viewer');
        const before = requests.length;
        await page.evaluate(() => lificSession.loadRole(1));
        assert.equal(await page.evaluate(() => lificSession.state.role.role), 'viewer');
        assert.equal(requests.length, before);
        assert.equal(await page.locator('[data-project-identity]').count(), 1);
      });

      await t.test('quiet home loads its mascot from an asset included in the Topcoat frontend', async () => {
        mode = 'home';
        await page.goto('http://lific.test/');
        await page.addScriptTag({content: sessionScript});
        await page.waitForFunction(() => !lificSession.state.loading);
        await page.addScriptTag({content: dashboardScript});
        await page.waitForFunction(() => document.querySelector('[data-dashboard-content] img')?.complete);
        const image = await page.locator('[data-dashboard-content] img').evaluate(image => ({src: image.getAttribute('src'), width: image.naturalWidth}));
        assert.equal(image.src, '/__topcoat-dashboard-mascot.png');
        assert.ok(image.width > 0);
      });
      assert.deepEqual(pageErrors, []);
    } finally {await browser.close();}
  });

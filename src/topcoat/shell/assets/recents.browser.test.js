const {test} = require('node:test');
const assert = require('node:assert/strict');
const fs = require('node:fs');
const path = require('node:path');

test('headless browser recents retain keyboard focus during refresh and isolate project/public transitions',
  {skip: !process.env.PLAYWRIGHT_EXECUTABLE_PATH}, async () => {
    const {chromium} = await import(path.resolve(__dirname, '../../../..', 'e2e/node_modules/playwright/index.mjs'));
    const browser = await chromium.launch({headless: true, executablePath: process.env.PLAYWRIGHT_EXECUTABLE_PATH});
    try {
      const page = await browser.newPage();
      page.setDefaultTimeout(5000);
      const failures = [];
      page.on('pageerror', error => failures.push(error.message));
      const css = fs.readFileSync(`${__dirname}/recents.css`, 'utf8');
      const html = `<!doctype html><html lang="en"><head><title>Recents fixture</title><style>${css}</style></head><body>
        <section data-topcoat-recents aria-label="Recent resources">
          <button type="button" data-recents-toggle aria-expanded="false" aria-controls="tc-sidebar-recents-list">Recent issues</button>
          <div id="tc-sidebar-recents-list" data-recents-content hidden aria-busy="false">
            <p data-recents-status role="status" aria-live="polite"></p>
            <ul data-recents-list></ul>
            <p data-recents-error role="status" aria-live="polite" hidden></p>
          </div>
        </section></body></html>`;
      await page.route('http://lific.test/**', route => route.fulfill({contentType: 'text/html', body: html}));
      await page.goto('http://lific.test/LIF/issues/LIF-1');
      await page.addScriptTag({content: fs.readFileSync(`${__dirname}/recents.js`, 'utf8')});
      await page.evaluate(() => {
        window.requests = [];
        window.replies = [];
        window.issue = (id, project_id = 7, identifier = `LIF-${id}`) => ({id, project_id, identifier, title: `Issue <${id}>`});
        window.session = {state: {user: {id: 1}, publicProject: null}, request(path) {
          window.requests.push(path);
          return window.requests.length === 1 ? Promise.resolve({ok: true, data: [window.issue(1)]})
            : new Promise(resolve => window.replies.push(resolve));
        }};
        window.recents = LificTopcoatRecents.attach(document.querySelector('[data-topcoat-recents]'), {
          session: window.session, catalog: {generation: 1, projects: [{id: 7, identifier: 'LIF'}, {id: 8, identifier: 'OTHER'}]},
        });
      });
      await page.waitForFunction(() => document.querySelectorAll('[data-recents-list] a').length === 1);
      const toggle = page.getByRole('button', {name: 'Recent issues'});
      await toggle.focus();
      await page.keyboard.press('Space');
      assert.equal(await toggle.getAttribute('aria-expanded'), 'true');
      const link = page.getByRole('link', {name: 'LIF-1: Issue <1>'});
      await link.focus();
      await page.evaluate(() => {window.refreshing = window.recents.refresh();});
      assert.equal(await page.locator('[data-recents-content]').getAttribute('aria-busy'), 'true');
      assert.equal(await page.locator('[data-recents-status]').textContent(), 'Loading recent issues…');
      assert.equal(await link.evaluate(element => document.activeElement === element), true);
      assert.equal(await link.locator('b').count(), 0);
      await page.evaluate(async () => {window.replies[0]({ok: true, data: [window.issue(2)]}); await window.refreshing;});
      await page.evaluate(() => {
        history.replaceState({}, '', '/LIF/issues/LIF-2');
        dispatchEvent(new PopStateEvent('popstate'));
        history.replaceState({}, '', '/OTHER/issues');
        dispatchEvent(new PopStateEvent('popstate'));
      });
      assert.equal(await page.locator('[data-recents-list] a').count(), 0);
      await page.evaluate(() => window.replies[1]({ok: true, data: [window.issue(3)]}));
      assert.equal(await page.locator('[data-recents-list] a').count(), 0);
      await page.evaluate(() => window.replies[2]({ok: true, data: [window.issue(9, 8, 'OTHER-9')]}));
      await page.waitForFunction(() => document.querySelector('[data-recents-list] a')?.getAttribute('href') === '/OTHER/issues/OTHER-9');
      const privateRequests = await page.evaluate(() => window.requests.length);
      await page.evaluate(() => {window.session.state.publicProject = 'LIF'; dispatchEvent(new CustomEvent('lific:scope-change'));});
      assert.equal(await page.locator('[data-recents-list] a').count(), 0);
      assert.equal(await page.locator('[data-topcoat-recents]').isVisible(), false);
      assert.equal(await page.evaluate(() => window.requests.length), privateRequests);
      assert.deepEqual(failures, []);
    } finally {await browser.close();}
  });

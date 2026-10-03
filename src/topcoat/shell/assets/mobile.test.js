/* Headless browser contract tests. No Vite or backend build is involved. */
import assert from 'node:assert/strict';
import test from 'node:test';
import {readFile} from 'node:fs/promises';
import {createServer} from 'node:http';
import {chromium} from '../../../../e2e/node_modules/playwright/index.mjs';

const script = await readFile(new URL('./mobile.js', import.meta.url), 'utf8');
const css = await readFile(new URL('./mobile.css', import.meta.url), 'utf8');
const catalog = {generation: 1, projects: [
  {id: 1, identifier: 'ONE', name: 'One', emoji: '1'},
  {id: 2, identifier: 'TWO', name: 'Two', emoji: null},
], groups: [{id: 1, name: 'Work', sort_order: 0, project_ids: [1, 2]}]};
function fixture(publicProject = '') {
  const data = JSON.stringify(catalog).replaceAll('"', '&quot;');
  return `<!doctype html><html><head><meta name="viewport" content="width=device-width"><style>
    :root{--tc-surface:#fff;--tc-bg:#eee;--tc-text:#222;--tc-muted:#555;--tc-radius:.375rem;--tc-border:#aaa;--tc-focus:#5746a0;--tc-accent:#5746a0;--tc-accent-text:#fff;--tc-font:system-ui} body{margin:0}
    ${css}</style></head><body style="overflow:auto"><main id="background"><button id="open" class="tc-mobile-trigger" data-mobile-open="" aria-label="Open navigation" aria-controls="tc-mobile-navigation" aria-expanded="false">Navigation</button><button id="outside">Outside</button></main><aside id="already-inert" inert>Existing inert content</aside>
    <div class="tc-mobile" id="tc-mobile-navigation" data-mobile-navigation data-mobile-catalog="${data}" ${publicProject ? `data-mobile-public-project="${publicProject}"` : ''} data-mobile-active-project="ONE" data-mobile-active-page="issues" data-open="false" data-level="root" role="dialog" aria-label="Navigation" aria-modal="true" aria-hidden="true" inert tabindex="-1">
      <div class="tc-mobile__pane tc-mobile__root" data-mobile-root inert aria-hidden="true"><header class="tc-mobile__header"><span>Lific</span><button data-mobile-close>Close</button></header><nav class="tc-mobile__scroll" aria-label="Projects">${publicProject ? '' : '<a href="/" data-mobile-destination>Home</a>'}<h2>Projects</h2><div data-mobile-project-list></div><p data-mobile-empty hidden>No projects available.</p>${publicProject ? '' : '<a href="/projects/new" data-mobile-destination>New project</a>'}</nav><footer class="tc-mobile__footer">${publicProject ? 'Read only' : '<a href="/settings" data-mobile-destination>Settings</a>'}</footer></div>
      <div class="tc-mobile__pane tc-mobile__project" data-mobile-project inert aria-hidden="true"><header class="tc-mobile__header"><button data-mobile-back>Projects</button><button data-mobile-close>Close</button></header><div class="tc-mobile__project-heading"><h2 data-mobile-project-name>Project</h2><p data-mobile-project-identifier></p></div><nav class="tc-mobile__scroll" data-mobile-destinations aria-label="Project destinations"></nav><p data-mobile-unavailable hidden>This project is no longer available.</p></div>
    </div><script>
      if (!history.state) history.replaceState({unrelated:{value:42}}, '', location.href);
      window.navigationRequests=[];
      window.addEventListener('lific:navigate', event=>{
        window.navigationRequests.push({...event.detail, baseDepth:history.state?.lificMobileNav?.depth});
        history[event.detail.history==='replace'?'replaceState':'pushState']({...history.state,fixtureRoute:event.detail.href}, '', event.detail.href);
        window.lificMobileNavigation.routeChanged();
      });
    </script><script src="/mobile.js"></script></body></html>`;
}

test('mobile browser history, modal behavior, catalog and public scope', {timeout: 90000}, async t => {
  const server = createServer((request, response) => {
    if (request.url === '/mobile.js') { response.setHeader('Content-Type', 'text/javascript'); response.end(script); }
    else { response.setHeader('Content-Type', 'text/html'); response.end(fixture(request.url.startsWith('/public/') ? 'ONE' : '')); }
  });
  await new Promise(resolve => server.listen(0, '127.0.0.1', resolve));
  const origin = `http://127.0.0.1:${server.address().port}`;
  const browser = await chromium.launch({headless: true, executablePath: process.env.PLAYWRIGHT_EXECUTABLE_PATH});
  const errors = [];
  async function page(path = '/ONE/issues') {
    const page = await browser.newPage({viewport: {width: 390, height: 844}, reducedMotion: 'reduce'});
    page.setDefaultTimeout(5000);
    page.on('pageerror', error => errors.push(error.message));
    await page.goto(origin + path);
    return page;
  }
  const entry = page => page.evaluate(() => history.state?.lificMobileNav);
  const depth = async (page, value) => page.waitForFunction(value => history.state?.lificMobileNav?.depth === value, value);
  const visible = async (page, value) => page.waitForFunction(value => document.querySelector('[data-mobile-navigation]').dataset.open === String(value), value);
  try {
    await t.test('Back, Forward and reload restore root and project panes without adding entries', async () => {
      const p = await page();
      await p.evaluate(() => lificMobileNavigation.openAt(true));
      assert.equal(await p.locator('[data-mobile-navigation]').getAttribute('data-open'), 'false');
      await p.locator('#open').click();
      await depth(p, 1);
      const session = (await entry(p)).session;
      await p.locator('[data-mobile-project-trigger="ONE"]').click();
      await depth(p, 2);
      assert.deepEqual(await p.evaluate(() => history.state.unrelated), {value: 42});
      const length = await p.evaluate(() => history.length);
      await p.reload();
      await visible(p, true);
      assert.equal((await entry(p)).depth, 2);
      assert.equal((await entry(p)).session, session);
      assert.equal(await p.evaluate(() => history.length), length);
      assert.equal(await p.locator('[data-mobile-project-name]').textContent(), 'One');
      await p.goBack(); await depth(p, 1);
      assert.equal(await p.evaluate(() => document.activeElement.dataset.mobileProjectTrigger), 'ONE');
      await p.goBack(); await depth(p, 0); await visible(p, false);
      assert.equal(await p.evaluate(() => document.activeElement.id), 'open');
      await p.goForward(); await depth(p, 1); await visible(p, true);
      await p.goForward(); await depth(p, 2);
      await p.keyboard.press('Escape'); await depth(p, 1);
      await p.keyboard.press('Escape'); await depth(p, 0); await visible(p, false);
      await p.close();
    });

    await t.test('switching projects with openAt does not focus the previous project trigger on Back', async () => {
      const p = await page();
      await p.locator('#open').click(); await depth(p, 1);
      await p.locator('[data-mobile-project-trigger="ONE"]').click(); await depth(p, 2);
      await p.evaluate(() => lificMobileNavigation.openAt('TWO'));
      await depth(p, 2);
      assert.equal(await p.locator('[data-mobile-project-name]').textContent(), 'Two');
      await p.keyboard.press('Escape'); await depth(p, 1);
      assert.notEqual(await p.evaluate(() => document.activeElement.dataset.mobileProjectTrigger), 'ONE');
      await p.close();
    });

    await t.test('reopening at a different project does not focus a trigger from the closed drawer', async () => {
      const p = await page();
      await p.locator('#open').click(); await depth(p, 1);
      await p.locator('[data-mobile-project-trigger="ONE"]').click(); await depth(p, 2);
      await p.evaluate(() => lificMobileNavigation.close()); await depth(p, 0); await visible(p, false);
      await p.evaluate(() => lificMobileNavigation.openAt('TWO')); await depth(p, 2);
      await p.keyboard.press('Escape'); await depth(p, 1);
      assert.notEqual(await p.evaluate(() => document.activeElement.dataset.mobileProjectTrigger), 'ONE');
      await p.close();
    });

    await t.test('drawer desktop behavior follows the CSS 48rem breakpoint with a changed root font size', async () => {
      const p = await page();
      await p.evaluate(() => { document.documentElement.style.fontSize = '24px'; });
      await p.setViewportSize({width: 767, height: 844});
      assert.equal(await p.locator('#open').evaluate(element => getComputedStyle(element).display), 'inline-flex');
      await p.evaluate(() => lificMobileNavigation.openAt()); await depth(p, 1);
      await p.evaluate(() => lificMobileNavigation.close()); await depth(p, 0);
      await p.setViewportSize({width: 768, height: 844});
      assert.equal(await p.locator('#open').evaluate(element => getComputedStyle(element).display), 'none');
      await p.evaluate(() => lificMobileNavigation.openAt());
      assert.equal((await entry(p)).depth, 0);
      await p.close();
    });

    await t.test('destination selection unwinds both drawer entries before requesting one page navigation', async () => {
      const p = await page();
      await p.evaluate(() => lificMobileNavigation.openAt('TWO'));
      await depth(p, 2);
      await p.locator('[data-mobile-destinations] a[href="/TWO/issues"]').click();
      await p.waitForFunction(() => location.pathname === '/TWO/issues');
      assert.deepEqual(await p.evaluate(() => navigationRequests), [{href: '/TWO/issues', history: 'push', baseDepth: 0}]);
      await visible(p, false);
      await p.goBack();
      assert.equal(new URL(p.url()).pathname, '/ONE/issues');
      assert.equal((await entry(p)).depth, 0);
      await visible(p, false);
      await p.close();
    });

    await t.test('external navigation cancels a queued destination and forged history is ignored', async () => {
      const p = await page();
      await p.evaluate(() => {
        lificMobileNavigation.openAt('ONE');
        lificMobileNavigation.navigateTo('/TWO/issues');
        lificMobileNavigation.routeChanged();
      });
      await depth(p, 0);
      assert.deepEqual(await p.evaluate(() => navigationRequests), []);
      await p.evaluate(() => {
        history.replaceState({...history.state, lificMobileNav: {version:1,session:'forged',href:location.href,depth:2,project:'ONE'}}, '', location.href);
      });
      await p.reload(); await visible(p, false);
      await p.locator('#open').click(); await depth(p, 1);
      assert.notEqual((await entry(p)).session, 'forged');
      await p.close();
    });

    await t.test('focus stays in the active pane and background inert/scroll state restores', async () => {
      const p = await page();
      await p.locator('#open').click(); await visible(p, true);
      assert.equal(await p.locator('#background').evaluate(element => element.inert), true);
      assert.equal(await p.locator('#already-inert').evaluate(element => element.inert), true);
      assert.equal(await p.evaluate(() => document.body.style.overflow), 'hidden');
      await p.evaluate(() => {
        const late = document.createElement('button'); late.id='late'; document.body.append(late);
      });
      await p.waitForFunction(() => document.querySelector('#late').inert);
      await p.keyboard.press('Shift+Tab');
      assert.equal(await p.evaluate(() => document.activeElement.getAttribute('href')), '/settings');
      await p.keyboard.press('Tab');
      assert.equal(await p.evaluate(() => document.activeElement.hasAttribute('data-mobile-close')), true);
      await p.locator('[data-mobile-project-trigger="ONE"]').click(); await depth(p, 2);
      assert.equal(await p.locator('[data-mobile-root]').evaluate(element => element.inert), true);
      assert.equal(await p.locator('[data-mobile-project]').evaluate(element => element.inert), false);
      assert.equal(await p.locator('[data-mobile-navigation]').evaluate(element => getComputedStyle(element).transitionDuration), '0s');
      await p.evaluate(() => lificMobileNavigation.close()); await depth(p, 0);
      assert.equal(await p.locator('#background').evaluate(element => element.inert), false);
      assert.equal(await p.locator('#already-inert').evaluate(element => element.inert), true);
      assert.equal(await p.locator('#late').evaluate(element => element.inert), false);
      assert.equal(await p.evaluate(() => document.body.style.overflow), 'auto');
      await p.close();
    });

    await t.test('read-only catalog updates preserve focus, ignore stale generations and handle removed projects', async () => {
      const p = await page();
      await p.locator('#open').click();
      await p.locator('[data-mobile-project-trigger="ONE"]').focus();
      const updated = {...catalog, generation: 2, projects: catalog.projects.map(item => ({...item, name: item.identifier === 'ONE' ? 'One <updated>' : item.name}))};
      assert.equal(await p.evaluate(catalog => lificMobileNavigation.setCatalog(catalog), updated), true);
      assert.equal(await p.evaluate(() => document.activeElement.dataset.mobileProjectTrigger), 'ONE');
      assert.equal(await p.locator('[data-mobile-project-trigger="ONE"] .tc-mobile__project-text > span').first().textContent(), 'One <updated>');
      assert.equal(await p.locator('[data-mobile-project-trigger="ONE"] updated').count(), 0);
      assert.equal(await p.evaluate(catalog => lificMobileNavigation.setCatalog(catalog), catalog), false);
      await p.locator('[data-mobile-project-trigger="ONE"]').click(); await depth(p, 2);
      await p.evaluate(catalog => lificMobileNavigation.setCatalog(catalog), {...updated, generation: 3, projects: [catalog.projects[1]]});
      assert.equal(await p.locator('[data-mobile-unavailable]').isVisible(), true);
      assert.equal(await p.locator('[data-mobile-destinations] a').count(), 0);
      await p.keyboard.press('Escape'); await depth(p, 1);
      assert.equal(await p.evaluate(() => document.activeElement.hasAttribute('data-mobile-close')), true);
      await p.close();
    });

    await t.test('touch gestures pop project, dismiss root and leave vertical scroll/cancel unchanged', async () => {
      const p = await page();
      await p.evaluate(() => lificMobileNavigation.openAt('ONE')); await depth(p, 2);
      // Use Chromium's native touch input so pointer capture is exercised.
      await p.evaluate(() => {
        window.touchTrace = [];
        for (const type of ['pointerdown','pointermove','pointerup','pointercancel','lostpointercapture','click']) {
          document.querySelector('[data-mobile-navigation]').addEventListener(type, event => touchTrace.push({type,x:event.clientX,y:event.clientY,target:event.target.tagName,level:history.state.lificMobileNav.depth,dragging:document.querySelector('[data-mobile-navigation]').dataset.dragging}));
        }
      });
      const cdp = await p.context().newCDPSession(p);
      async function swipe(from, to) {
        await cdp.send('Input.dispatchTouchEvent', {type:'touchStart', touchPoints:[{x:from[0],y:from[1]}]});
        await cdp.send('Input.dispatchTouchEvent', {type:'touchMove', touchPoints:[{x:to[0],y:to[1]}]});
        await cdp.send('Input.dispatchTouchEvent', {type:'touchEnd', touchPoints:[]});
      }
      await swipe([30,200], [220,205]);
      try { await depth(p, 1); } catch (error) { throw new Error(JSON.stringify(await p.evaluate(() => ({touchTrace, state:history.state}))), {cause:error}); }
      await swipe([220,200], [225,350]); assert.equal((await entry(p)).depth, 1);
      await cdp.send('Input.dispatchTouchEvent', {type:'touchStart',touchPoints:[{x:220,y:200}]});
      await cdp.send('Input.dispatchTouchEvent', {type:'touchMove',touchPoints:[{x:160,y:200}]});
      await cdp.send('Input.dispatchTouchEvent', {type:'touchCancel',touchPoints:[]});
      assert.equal((await entry(p)).depth, 1);
      assert.equal(await p.locator('[data-mobile-navigation]').getAttribute('data-dragging'), null);
      await swipe([280,200], [70,205]);
      try { await depth(p, 0); } catch (error) { throw new Error(JSON.stringify(await p.evaluate(() => touchTrace)), {cause:error}); }
      await visible(p, false);
      await p.close();
    });

    await t.test('public project shows only published destinations and rejects private or foreign navigation', async () => {
      const p = await page('/public/ONE/issues');
      await p.locator('#open').click();
      assert.equal(await p.locator('[data-mobile-project-trigger]').count(), 1);
      assert.equal(await p.locator('[data-mobile-root] a').count(), 0);
      await p.locator('[data-mobile-project-trigger="ONE"]').click(); await depth(p, 2);
      assert.deepEqual(await p.locator('[data-mobile-destinations] a').evaluateAll(links => links.map(link => link.getAttribute('href'))), ['/public/ONE/issues', '/public/ONE/pages']);
      for (const href of ['/ONE/issues', '/public/TWO/issues', '/public/ONE/modules', '/PUBLIC/ONE/issues', '//evil.test/']) {
        assert.equal(await p.evaluate(href => lificMobileNavigation.navigateTo(href), href), false);
      }
      assert.equal(await p.evaluate(catalog => lificMobileNavigation.setCatalog(catalog), {...catalog, generation: 9}), false);
      await p.locator('[data-mobile-destinations] a[href="/public/ONE/pages"]').click();
      await p.waitForFunction(() => location.pathname === '/public/ONE/pages');
      assert.deepEqual(await p.evaluate(() => navigationRequests), [{href:'/public/ONE/pages',history:'push',baseDepth:0}]);
      await p.close();
    });

    await t.test('public reload refuses a foreign project pane stored in the drawer namespace', async () => {
      const p = await page('/public/ONE/issues');
      await p.evaluate(() => history.replaceState({...history.state, lificMobileNav: {
        version: 1, session: crypto.randomUUID(), href: location.href, depth: 2, project: 'TWO',
      }}, '', location.href));
      await p.reload(); await visible(p, false);
      await p.locator('#open').click(); await depth(p, 1);
      assert.equal(await p.locator('[data-mobile-project-trigger="TWO"]').count(), 0);
      assert.deepEqual(await p.evaluate(() => history.state.unrelated), {value:42});
      await p.close();
    });

    await t.test('desktop resize closes and unwinds restored drawer history', async () => {
      const p = await page();
      await p.evaluate(() => lificMobileNavigation.openAt('ONE')); await depth(p, 2);
      await p.setViewportSize({width: 1200, height: 800});
      await depth(p, 0); await visible(p, false);
      assert.equal(await p.locator('#background').evaluate(element => element.inert), false);
      await p.close();
    });
    assert.deepEqual(errors, []);
  } finally {
    await browser.close();
    await new Promise(resolve => server.close(resolve));
  }
});

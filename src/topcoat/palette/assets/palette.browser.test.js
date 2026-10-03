const {test} = require('node:test');
const assert = require('node:assert/strict');
const fs = require('node:fs');
const path = require('node:path');

test('headless palette keyboard, search, action registration and scope behavior', {skip: !process.env.PLAYWRIGHT_EXECUTABLE_PATH}, async t => {
  const {chromium} = await import(path.resolve(__dirname, '../../../..', 'e2e/node_modules/playwright/index.mjs'));
  const browser = await chromium.launch({headless: true, executablePath: process.env.PLAYWRIGHT_EXECUTABLE_PATH});
  const page = await browser.newPage();
  page.setDefaultTimeout(5000);
  const errors = [];
  page.on('pageerror', error => errors.push(error.message));
  const html = `<!doctype html><html lang="en"><head><meta charset="utf-8"><title>Palette fixture</title><style>${fs.readFileSync(`${__dirname}/palette.css`, 'utf8')}</style></head><body>
    <button data-palette-open>Jump to…</button><button data-sidebar-toggle>Collapse sidebar</button><input id="outside" aria-label="Outside">
    <dialog data-topcoat-palette class="tc-palette" aria-label="Jump or act">
      <div class="tc-palette__input-row"><span data-palette-mode hidden></span>
        <input data-palette-input aria-label="Search or run an action" role="combobox" aria-autocomplete="list" aria-expanded="true" aria-controls="tc-palette-results" autocomplete="off">
        <button data-palette-close aria-label="Close search">Close</button></div>
      <div id="tc-palette-results" data-palette-results role="listbox" aria-label="Results"></div>
      <p data-palette-status role="status" aria-live="polite"></p>
      <p data-palette-error role="status" aria-live="polite" hidden></p>
      <footer>↑↓ move · Enter open · Ctrl/⌘ Enter new tab</footer>
    </dialog><dialog data-palette-help class="tc-shortcut-help" aria-label="Keyboard shortcuts">
      <h2>Keyboard shortcuts</h2><button data-shortcut-close aria-label="Close shortcuts">Close</button><div data-shortcut-list></div>
    </dialog></body></html>`;
  try {
    await page.route('http://lific.test/**', route => route.fulfill({contentType: 'text/html', body: html}));
    await page.goto('http://lific.test/LIF/issues');
    await page.evaluate(() => {
      window.requests = []; window.failSearch = false; window.deferSearch = false; window.pending = [];
      localStorage.setItem('lific_recents', JSON.stringify([{type: 'page', project: 'LIF', routeId: '10', identifier: 'LIF-DOC-1', title: 'Recently read', ts: 1}]));
      window.lificSession = {state: {user: {id: 1}, publicProject: null}, affordances: () => ({edit: true, manage: false}),
        async request(path) {
          window.requests.push(path);
          if (path === '/projects') return {ok: true, data: [{id: 7, identifier: 'LIF', name: 'Lific'}, {id: 8, identifier: 'OTHER', name: 'Other'}]};
          if (path.startsWith('/modules')) return {ok: true, data: [{id: 5, project_id: 7, name: 'Module needle'}]};
          if (path.startsWith('/folders')) return {ok: true, data: [{id: 6, project_id: 7, name: 'Folder needle'}]};
          if (path.startsWith('/plans')) return {ok: true, data: [{id: 7, project_id: 7, title: 'Plan needle', identifier: 'LIF-PLAN-7'}]};
          if (path.startsWith('/pages')) return {ok: true, data: [{id: 10, project_id: 7, title: 'Page reference', identifier: 'LIF-DOC-1', sequence: 1}]};
          if (path.startsWith('/issues/resolve/')) return {ok: true, data: {title: 'Resolved reference', identifier: path.split('/').pop(), status: 'active'}};
          if (window.deferSearch) return new Promise(resolve => window.pending.push({path, resolve}));
          if (window.failSearch) return {ok: false, error: 'Search unavailable', status: 500};
          return {ok: true, data: new URL(`http://lific.test${path}`).searchParams.get('query') === 'needle' ? [
            {result_type: 'issue', project_id: 7, id: 1, identifier: 'LIF-1', title: 'Issue needle', snippet: '<b>Unsafe</b> **needle**'},
            {result_type: 'page', project_id: 7, id: 10, identifier: 'LIF-DOC-1', title: 'Page needle', snippet: 'Page'},
            {result_type: 'issue', project_id: 99, id: 9, identifier: 'SECRET-9', title: 'Hidden foreign project'},
          ] : []};
        }};
      window.lificSync = {peekProject: () => null, subscribe: () => () => {}};
      window.navigations = [];
      window.sidebarToggles = 0;
      document.querySelector('[data-sidebar-toggle]').addEventListener('click', () => window.sidebarToggles++);
      addEventListener('lific:navigate', event => window.navigations.push(event.detail));
    });
    await page.addScriptTag({content: fs.readFileSync(`${__dirname}/palette.js`, 'utf8')});
    const input = page.getByRole('combobox');
    const dialog = page.locator('[data-topcoat-palette]');

    await t.test('Ctrl+K/Ctrl+P toggle, focus restores and mobile overlay suppresses invocation', async () => {
      await page.getByRole('button', {name: 'Jump to…'}).focus();
      await page.keyboard.press('Control+k');
      await dialog.waitFor({state: 'visible'});
      assert.equal(await input.evaluate(element => document.activeElement === element), true);
      await page.getByRole('option', {name: /Recently read/}).waitFor();
      await page.keyboard.press('Escape');
      assert.equal(await dialog.isVisible(), false);
      assert.equal(await page.getByRole('button', {name: 'Jump to…'}).evaluate(element => document.activeElement === element), true);
      await page.keyboard.press('Control+p');
      assert.equal(await dialog.isVisible(), true);
      await page.keyboard.press('Control+p');
      assert.equal(await dialog.isVisible(), false);
      await page.evaluate(() => {const other = document.createElement('dialog'); other.dataset.mobileNavigation = ''; document.body.append(other); other.showModal();});
      await page.keyboard.press('Control+k');
      assert.equal(await dialog.isVisible(), false);
      await page.evaluate(() => document.querySelector('[data-mobile-navigation]').remove());
    });

    await t.test('all searchable domains, safe snippet text and keyboard selection', async () => {
      await page.keyboard.press('Control+k');
      await input.fill('needle');
      await page.getByRole('option', {name: /Issue needle/}).waitFor();
      for (const title of ['Page needle', 'Plan needle', 'Module needle', 'Folder needle']) await page.getByRole('option', {name: new RegExp(title)}).waitFor();
      assert.equal(await page.getByText('Hidden foreign project').count(), 0);
      assert.equal(await page.locator('[data-palette-results] b').count(), 0);
      await page.keyboard.press('ArrowDown');
      const selectedId = await input.getAttribute('aria-activedescendant');
      const route = await page.locator(`#${selectedId}`).getAttribute('data-palette-route');
      await page.keyboard.press('Enter');
      assert.equal(await dialog.isVisible(), false);
      assert.equal(await page.evaluate(() => window.navigations.at(-1).href), route);
    });

    await t.test('global sidebar shortcut and help honor typing/modal focus and show route-owned bindings', async () => {
      const help = page.getByRole('dialog', {name: 'Keyboard shortcuts'});
      await page.getByRole('button', {name: 'Collapse sidebar'}).focus();
      await page.keyboard.press('Control+Backslash');
      assert.equal(await page.evaluate(() => window.sidebarToggles), 1);
      await page.getByRole('textbox', {name: 'Outside'}).focus();
      await page.keyboard.press('Control+Backslash');
      await page.keyboard.press('?');
      assert.equal(await page.evaluate(() => window.sidebarToggles), 1);
      assert.equal(await help.isVisible(), false);
      await page.evaluate(() => window.lificPalette.register('help', {shortcuts: [{scope: 'Issue detail', keys: 'C', label: 'Add comment'}]}));
      await page.getByRole('button', {name: 'Collapse sidebar'}).focus();
      await page.keyboard.press('?');
      await help.waitFor({state: 'visible'});
      await help.getByText('Add comment', {exact: true}).waitFor();
      await page.keyboard.press('Control+k');
      assert.equal(await dialog.isVisible(), false);
      await page.keyboard.press('Escape');
      assert.equal(await help.isVisible(), false);
      assert.equal(await page.getByRole('button', {name: 'Collapse sidebar'}).evaluate(element => document.activeElement === element), true);
    });

    await t.test('empty/no-results/failures are announced and stale search cannot overwrite new query', async () => {
      await page.keyboard.press('Control+k');
      await input.fill('nomatch');
      await page.waitForFunction(() => document.querySelector('[data-palette-status]').textContent.includes('Nothing matches'));
      await page.evaluate(() => {window.failSearch = true;});
      await input.fill('broken');
      await page.getByText('Search unavailable', {exact: true}).waitFor();
      await page.evaluate(() => {window.failSearch = false; window.deferSearch = true;});
      await input.fill('old');
      await page.waitForFunction(() => window.pending.some(item => item.path.includes('old')));
      await input.fill('new');
      await page.waitForFunction(() => window.pending.some(item => item.path.includes('new')));
      await page.evaluate(() => window.pending.find(item => item.path.includes('old')).resolve({ok: true, data: [{result_type: 'issue', project_id: 7, id: 1, identifier: 'LIF-1', title: 'Stale result'}]}));
      assert.equal(await page.getByText('Stale result').count(), 0);
      await page.evaluate(() => {window.pending.find(item => item.path.includes('new')).resolve({ok: true, data: []}); window.deferSearch = false;});
      await page.keyboard.press('Escape');
    });

    await t.test('shared theme tokens and enlarged text keep phone search inside its viewport', async () => {
      await page.setViewportSize({width: 390, height: 844});
      await page.evaluate(() => {
        document.documentElement.style.cssText = 'font-size:24px; --tc-surface:rgb(31,32,33); --tc-text:rgb(220,221,222);';
      });
      await page.keyboard.press('Control+k');
      const colors = await dialog.evaluate(element => ({background: getComputedStyle(element).backgroundColor, text: getComputedStyle(element).color}));
      assert.deepEqual(colors, {background: 'rgb(31, 32, 33)', text: 'rgb(220, 221, 222)'});
      const bounds = await dialog.boundingBox();
      assert.equal(bounds.x, 0);
      assert.equal(bounds.width, 390);
      const close = await page.getByRole('button', {name: 'Close search'}).boundingBox();
      assert.ok(close.x >= 0 && close.x + close.width <= 390);
      await page.getByRole('button', {name: 'Close search'}).click();
      await page.evaluate(() => {document.documentElement.style.cssText = '';});
      await page.setViewportSize({width: 1280, height: 720});
    });

    await t.test('route owners register permitted actions, submenus and prompts with Escape/Backspace behavior', async () => {
      await page.evaluate(() => {
        window.runs = [];
        window.unregister = window.lificPalette.register('detail', {actions: [
          {id: 'rename', title: 'Rename issue', requires: 'edit', prompt: {initial: 'Old title', submit: value => window.runs.push(value)}},
          {id: 'status', title: 'Set status…', requires: 'edit', children: () => [{title: 'Active', run: () => window.runs.push('active')}]},
          {id: 'admin', title: 'Manage permissions', requires: 'manage', run: () => window.runs.push('forbidden')},
        ]});
      });
      await page.keyboard.press('Control+k');
      assert.equal(await page.getByRole('option', {name: 'Manage permissions'}).count(), 0);
      await input.fill('rename');
      await page.keyboard.press('Enter');
      assert.equal(await input.inputValue(), 'Old title');
      await input.fill('New title');
      await page.keyboard.press('Enter');
      assert.deepEqual(await page.evaluate(() => window.runs), ['New title']);
      await page.keyboard.press('Control+k');
      await input.fill('set status');
      await page.keyboard.press('Enter');
      await page.getByRole('option', {name: 'Active'}).waitFor();
      await page.keyboard.press('Backspace');
      await page.getByRole('option', {name: 'Rename issue'}).waitFor();
      await input.fill('set status');
      await page.keyboard.press('Enter');
      await page.keyboard.press('Escape');
      assert.equal(await dialog.isVisible(), true);
      await page.keyboard.press('Escape');
      assert.equal(await dialog.isVisible(), false);
      await page.evaluate(() => window.unregister());
    });

    await t.test('compact references wait for exact resolution and Ctrl+Enter opens a direct new tab', async () => {
      await page.evaluate(() => {window.opened = []; window.open = (...args) => window.opened.push(args);});
      await page.keyboard.press('Control+k');
      await input.fill('34');
      await page.keyboard.press('Enter');
      await page.waitForFunction(() => window.navigations.at(-1)?.href === '/LIF/issues/LIF-34');
      await page.keyboard.press('Control+k');
      await input.fill('doc 1');
      await page.keyboard.press('Control+Enter');
      await page.waitForFunction(() => window.opened.length === 1);
      assert.deepEqual(await page.evaluate(() => window.opened[0]), ['/LIF/pages/10', '_blank', 'noopener']);
    });

    await t.test('unregistering or replacing a route owner cancels its open prompt and submenu callbacks', async () => {
      await page.evaluate(() => {
        window.staleRuns = [];
        window.removeOwner = window.lificPalette.register('ephemeral', {actions: [
          {id: 'rename', title: 'Ephemeral rename', prompt: {submit: value => window.staleRuns.push(value)}},
          {id: 'status', title: 'Ephemeral status', children: () => [{title: 'Active', run: () => window.staleRuns.push('active')}]},
        ]});
      });
      await page.keyboard.press('Control+k');
      await input.fill('ephemeral rename');
      await page.keyboard.press('Enter');
      await input.fill('stale value');
      await page.evaluate(() => window.removeOwner());
      await page.keyboard.press('Enter');
      assert.deepEqual(await page.evaluate(() => window.staleRuns), []);
      assert.equal(await dialog.isVisible(), false);
      await page.evaluate(() => {
        window.removeOwner = window.lificPalette.register('ephemeral', {actions: [{id: 'status', title: 'Ephemeral status', children: () => [{title: 'Active', run: () => window.staleRuns.push('active')}]}]});
      });
      await page.keyboard.press('Control+k');
      await input.fill('ephemeral status');
      await page.keyboard.press('Enter');
      await page.getByRole('option', {name: 'Active'}).waitFor();
      await page.evaluate(() => window.removeOwner());
      await page.keyboard.press('Enter');
      assert.deepEqual(await page.evaluate(() => window.staleRuns), []);
      await page.evaluate(() => {
        window.lificPalette.register('ephemeral', {actions: [{id: 'rename', title: 'Ephemeral rename', prompt: {submit: value => window.staleRuns.push(value)}}]});
      });
      await page.keyboard.press('Control+k');
      await input.fill('ephemeral rename');
      await page.keyboard.press('Enter');
      await input.fill('stale replacement');
      await page.evaluate(() => window.lificPalette.register('ephemeral', {actions: []}));
      await page.keyboard.press('Enter');
      assert.deepEqual(await page.evaluate(() => window.staleRuns), []);
      assert.equal(await dialog.isVisible(), false);
      await page.evaluate(() => {
        window.lificPalette.register('ephemeral', {actions: [{id: 'rename', title: 'Ephemeral rename', prompt: {submit: value => window.staleRuns.push(value)}}]});
      });
      await page.keyboard.press('Control+k');
      await input.fill('ephemeral rename');
      await page.keyboard.press('Enter');
      await input.fill('stale route');
      await page.evaluate(() => {history.replaceState({}, '', '/OTHER/issues'); dispatchEvent(new PopStateEvent('popstate'));});
      await page.keyboard.press('Enter');
      assert.deepEqual(await page.evaluate(() => window.staleRuns), []);
      assert.equal(await dialog.isVisible(), false);
      await page.evaluate(() => {history.replaceState({}, '', '/LIF/issues'); dispatchEvent(new PopStateEvent('popstate'));});
    });

    await t.test('a project catalog event supersedes a delayed project fetch and its cached answer', async () => {
      await page.evaluate(() => {
        window.lificPalette.dispose();
        window.catalogRequest = window.lificSession.request;
        window.lificSession.request = path => path === '/projects' ? new Promise(resolve => {window.staleProjects = resolve;}) : window.catalogRequest(path);
        window.lificPalette = window.LificTopcoatPalette.mount();
      });
      await page.keyboard.press('Control+k');
      await page.waitForFunction(() => typeof window.staleProjects === 'function');
      await page.evaluate(() => dispatchEvent(new CustomEvent('lific:project-catalog', {detail: {
        generation: 99, projects: [{id: 9, identifier: 'FRESH', name: 'Fresh project'}], groups: [],
      }})));
      await page.getByRole('option', {name: /Fresh project/}).waitFor();
      await page.evaluate(() => window.staleProjects({ok: true, data: [{id: 7, identifier: 'LIF', name: 'Lific'}]}));
      await input.fill('lific');
      assert.equal(await page.getByRole('option', {name: /Lific/}).count(), 0);
      await input.fill('fresh');
      await page.getByRole('option', {name: /Fresh project/}).waitFor();
      await page.keyboard.press('Escape');
      await page.evaluate(() => {window.lificSession.request = window.catalogRequest;});
    });

    await t.test('cold opening retains typed query and Enter intent while the project catalog loads', async () => {
      await page.evaluate(() => {
        window.lificPalette.dispose();
        window.originalRequest = window.lificSession.request;
        window.lificSession.request = path => path === '/projects' ? new Promise(resolve => {window.resolveProjects = resolve;}) : window.originalRequest(path);
        window.lificPalette = window.LificTopcoatPalette.mount();
        window.navigations = [];
      });
      await page.keyboard.press('Control+k');
      await input.fill('34');
      await page.keyboard.press('Enter');
      await page.evaluate(() => window.resolveProjects({ok: true, data: [{id: 7, identifier: 'LIF', name: 'Lific'}]}));
      await page.waitForFunction(() => window.navigations.at(-1)?.href === '/LIF/issues/LIF-34');
      await page.evaluate(() => {window.lificSession.request = window.originalRequest;});
    });

    await t.test('public/account transitions clear private results actions and stop private requests', async () => {
      await page.keyboard.press('Control+k');
      await input.fill('needle');
      await page.getByRole('option', {name: /Issue needle/}).waitFor();
      await page.evaluate(() => {window.lificSession.state.publicProject = 'LIF'; dispatchEvent(new CustomEvent('lific:scope-change'));});
      const requests = await page.evaluate(() => window.requests.length);
      assert.equal(await dialog.isVisible(), false);
      await page.keyboard.press('Control+k');
      assert.equal(await dialog.isVisible(), false);
      assert.equal(await page.evaluate(() => window.requests.length), requests);
      assert.equal(await page.locator('[data-palette-results]').textContent(), '');
      await page.evaluate(() => {window.lificSession.state.publicProject = null; window.lificSession.state.user = {id: 2}; dispatchEvent(new CustomEvent('lific:account-change'));});
      await page.keyboard.press('Control+k');
      await dialog.waitFor({state: 'visible'});
      assert.equal(await page.getByRole('option', {name: 'Rename issue'}).count(), 0);
      await page.keyboard.press('Escape');
    });
    await t.test('a scope change stops remaining reference batches and rejects in-flight private answers', async () => {
      await page.evaluate(() => {
        window.lificPalette.dispose();
        window.probes = []; window.probeReplies = [];
        const request = window.lificSession.request;
        window.lificSession.request = path => {
          if (path === '/projects') return Promise.resolve({ok: true, data: Array.from({length: 6}, (_, i) => ({id: i + 1, identifier: `PROJ${i}`, name: `Project ${i}`}))});
          if (path.startsWith('/issues/resolve/')) {window.probes.push(path); return new Promise(resolve => window.probeReplies.push(resolve));}
          return request(path);
        };
        window.lificPalette = window.LificTopcoatPalette.mount();
      });
      await page.keyboard.press('Control+k');
      await input.fill('34');
      await page.keyboard.press('Enter');
      await page.waitForFunction(() => window.probes.length === 4);
      await page.evaluate(() => {
        window.lificSession.state.publicProject = 'LIF'; dispatchEvent(new CustomEvent('lific:scope-change'));
        window.probeReplies.forEach((reply, i) => reply({ok: true, data: {identifier: `PROJ${i}-34`, title: 'Private stale answer'}}));
      });
      await page.waitForTimeout(100);
      assert.equal(await page.evaluate(() => window.probes.length), 4);
      assert.equal(await page.locator('[data-palette-results]').textContent(), '');
    });
    await t.test('a catalog success suppresses an older project-load error', async () => {
      await page.evaluate(() => {
        window.lificPalette.dispose();
        window.lificSession.state.publicProject = null;
        const request = window.lificSession.request;
        window.lificSession.request = path => path === '/projects' ? new Promise((_, reject) => {window.rejectOldProjects = reject;}) : request(path);
        window.lificPalette = window.LificTopcoatPalette.mount();
      });
      await page.keyboard.press('Control+k');
      await page.waitForFunction(() => typeof window.rejectOldProjects === 'function');
      await page.evaluate(() => {
        dispatchEvent(new CustomEvent('lific:project-catalog', {detail: {
          generation: 100, projects: [{id: 11, identifier: 'FRESH', name: 'Fresh project'}], groups: [],
        }}));
        window.rejectOldProjects(new Error('Old project load failed'));
      });
      await page.waitForTimeout(20);
      assert.equal(await page.locator('[data-palette-error]').textContent(), '');
      await page.getByRole('option', {name: /Fresh project/}).waitFor();
      await page.keyboard.press('Escape');
    });
    assert.deepEqual(errors, []);
  } finally {await browser.close();}
});

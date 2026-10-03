const {test} = require('node:test');
const assert = require('node:assert/strict');
const fs = require('node:fs');
const path = require('node:path');

test('headless editor renders markdown as safe text and preserves a draft after conflict',
  {skip: !process.env.PLAYWRIGHT_EXECUTABLE_PATH}, async () => {
    const {chromium} = await import(path.resolve(__dirname, '../../../../../e2e/node_modules/playwright/index.mjs'));
    const browser = await chromium.launch({headless: true, executablePath: process.env.PLAYWRIGHT_EXECUTABLE_PATH});
    try {
      const page = await browser.newPage();
      page.setDefaultTimeout(4000);
      await page.setContent(`<main><section class="tc-issue-editor" data-topcoat-issue-editor>
        <div class="tc-issue-editor__toolbar"><button type="button" data-editor-edit>Edit</button>
          <button type="button" data-editor-preview-toggle>Preview</button>
          <button type="button" data-editor-save>Save</button></div>
        <p data-editor-status role="status"></p><p data-editor-error role="alert" hidden></p>
        <section data-editor-conflict hidden><p data-editor-conflict-message></p><pre data-editor-server-value></pre></section>
        <textarea data-editor-input aria-label="Issue description"></textarea>
        <article data-editor-preview hidden></article></section></main>`);
      await page.addScriptTag({content: fs.readFileSync(`${__dirname}/editor.js`, 'utf8')});
      await page.evaluate(() => {
        window.saves = [];
        window.addEventListener('lific:issue-detail-intent', event => {
          const {route, action} = event.detail;
          if (action.type === 'edit_description') return;
          window.saves.push(action);
          window.dispatchEvent(new CustomEvent('lific:issue-detail-conflict', {detail: {
            route: {...route, generation: route.generation + 1}, current_description: 'stale route', expected_seq: 99, edit_revision: action.edit_revision,
          }}));
          window.dispatchEvent(new CustomEvent('lific:issue-detail-conflict', {detail: {
            route, current_description: 'stale revision', expected_seq: 99, edit_revision: action.edit_revision + 1,
          }}));
          window.dispatchEvent(new CustomEvent('lific:issue-detail-conflict', {detail: {
            route, current_description: '<img src=x onerror=alert(1)>', expected_seq: 8, edit_revision: action.edit_revision,
          }}));
        });
        window.editor = lificIssueEditor.mount(document.querySelector('[data-topcoat-issue-editor]'), {
          route: {issue_id: 31, generation: 1}, text: 'before', saved_description: 'before', dirty: false,
          expected_seq: 4, capabilities: {edit: true}, debounce_ms: 10000,
        });
      });
      await page.locator('[data-editor-edit]').click();
      const input = page.locator('[data-editor-input]');
      await input.fill('mine **draft**\n\n<svg onload=alert(1)>');
      await page.locator('[data-editor-save]').click();
      await page.waitForFunction(() => window.saves.length === 1);
      await page.locator('[data-editor-conflict]').waitFor({state: 'visible'});
      assert.equal(await page.locator('[data-editor-conflict]').isVisible(), true);
      assert.equal(await input.inputValue(), 'mine **draft**\n\n<svg onload=alert(1)>');
      assert.equal(await page.locator('[data-editor-preview] svg, [data-editor-preview] img').count(), 0);
      assert.equal(await page.locator('[data-editor-preview]').textContent().then(text => text.includes('<svg onload=alert(1)>')), true);
      assert.equal(await page.locator('[data-editor-server-value]').textContent(), '<img src=x onerror=alert(1)>');
      assert.equal(await page.locator('[data-editor-preview] strong').textContent(), 'draft');
    } finally {await browser.close();}
  });

test('headless editor debounces rapid edits and flushes the next write in sequence',
  {skip: !process.env.PLAYWRIGHT_EXECUTABLE_PATH}, async () => {
    const {chromium} = await import(path.resolve(__dirname, '../../../../../e2e/node_modules/playwright/index.mjs'));
    const browser = await chromium.launch({headless: true, executablePath: process.env.PLAYWRIGHT_EXECUTABLE_PATH});
    try {
      const page = await browser.newPage();
      page.setDefaultTimeout(4000);
      await page.setContent(`<section data-topcoat-issue-editor>
        <div><button data-editor-edit>Edit</button><button data-editor-preview-toggle>Preview</button><button data-editor-save>Save</button></div>
        <p data-editor-status></p><p data-editor-error hidden></p>
        <section data-editor-conflict hidden><p data-editor-conflict-message></p><pre data-editor-server-value></pre></section>
        <textarea data-editor-input></textarea><article data-editor-preview hidden></article></section>`);
      await page.addScriptTag({content: fs.readFileSync(`${__dirname}/editor.js`, 'utf8')});
      await page.evaluate(() => {
        window.saves = [];
        window.addEventListener('lific:issue-detail-intent', event => {
          if (event.detail.action.type === 'save_description') saves.push(event.detail.action);
        });
        window.finishSave = (action, seq) => dispatchEvent(new CustomEvent('lific:issue-detail-applied', {detail: {
          route: {issue_id: 31, generation: 1}, kind: 'editor', description: action.description,
          expected_seq: seq, edit_revision: action.edit_revision,
        }}));
        lificIssueEditor.mount(document.querySelector('[data-topcoat-issue-editor]'), {
          route: {issue_id: 31, generation: 1}, text: '', saved_description: '', dirty: false,
          expected_seq: 4, capabilities: {edit: true}, debounce_ms: 60,
        });
      });
      await page.locator('[data-editor-edit]').click();
      const input = page.locator('[data-editor-input]');
      await input.fill('r'); await input.fill('rapid draft');
      await page.waitForFunction(() => window.saves.length === 1);
      assert.equal(await page.evaluate(() => saves[0].description), 'rapid draft');
      await input.fill('next draft');
      await page.locator('[data-editor-save]').click();
      await page.evaluate(() => finishSave(saves[0], 5));
      await page.waitForFunction(() => window.saves.length === 2);
      assert.equal(await page.evaluate(() => saves[1].description), 'next draft');
      assert.equal(await page.evaluate(() => saves[1].expected_seq), 5);
      await page.evaluate(() => finishSave(saves[1], 6));
      await page.waitForFunction(() => document.querySelector('[data-editor-status]').textContent === 'Saved');
    } finally {await browser.close();}
  });

test('headless editor shows an autosave failure without another user action',
  {skip: !process.env.PLAYWRIGHT_EXECUTABLE_PATH}, async () => {
    const {chromium} = await import(path.resolve(__dirname, '../../../../../e2e/node_modules/playwright/index.mjs'));
    const browser = await chromium.launch({headless: true, executablePath: process.env.PLAYWRIGHT_EXECUTABLE_PATH});
    try {
      const page = await browser.newPage();
      await page.setContent(`<section data-topcoat-issue-editor>
        <button data-editor-edit>Edit</button><button data-editor-preview-toggle>Preview</button><button data-editor-save>Save</button>
        <p data-editor-status></p><p data-editor-error hidden></p>
        <section data-editor-conflict hidden><p data-editor-conflict-message></p><pre data-editor-server-value></pre></section>
        <textarea data-editor-input></textarea><article data-editor-preview hidden></article></section>`);
      await page.addScriptTag({content: fs.readFileSync(`${__dirname}/editor.js`, 'utf8')});
      await page.evaluate(() => {
        window.addEventListener('lific:issue-detail-intent', event => {
          const {route, action} = event.detail;
          if (action.type !== 'save_description') return;
          setTimeout(() => dispatchEvent(new CustomEvent('lific:issue-detail-error', {detail: {
            route, edit_revision: action.edit_revision, error: 'Offline during autosave',
          }})), 0);
        });
        lificIssueEditor.mount(document.querySelector('[data-topcoat-issue-editor]'), {
          route: {issue_id: 31, generation: 1}, text: '', saved_description: '', expected_seq: 4,
          capabilities: {edit: true}, debounce_ms: 10,
        });
      });
      await page.locator('[data-editor-edit]').click();
      await page.locator('[data-editor-input]').fill('draft');
      await page.locator('[data-editor-error]').waitFor({state: 'visible'});
      assert.equal(await page.locator('[data-editor-error]').textContent(), 'Offline during autosave');
      assert.equal(await page.locator('[data-editor-input]').inputValue(), 'draft');
    } finally {await browser.close();}
  });

test('headless editor shows an autosave conflict without another user action',
  {skip: !process.env.PLAYWRIGHT_EXECUTABLE_PATH}, async () => {
    const {chromium} = await import(path.resolve(__dirname, '../../../../../e2e/node_modules/playwright/index.mjs'));
    const browser = await chromium.launch({headless: true, executablePath: process.env.PLAYWRIGHT_EXECUTABLE_PATH});
    try {
      const page = await browser.newPage();
      await page.setContent(`<section data-topcoat-issue-editor>
        <button data-editor-edit>Edit</button><button data-editor-preview-toggle>Preview</button><button data-editor-save>Save</button>
        <p data-editor-status></p><p data-editor-error hidden></p>
        <section data-editor-conflict hidden><p data-editor-conflict-message></p><pre data-editor-server-value></pre></section>
        <textarea data-editor-input></textarea><article data-editor-preview hidden></article></section>`);
      await page.addScriptTag({content: fs.readFileSync(`${__dirname}/editor.js`, 'utf8')});
      await page.evaluate(() => {
        window.addEventListener('lific:issue-detail-intent', event => {
          const {route, action} = event.detail;
          if (action.type !== 'save_description') return;
          setTimeout(() => dispatchEvent(new CustomEvent('lific:issue-detail-conflict', {detail: {
            route, edit_revision: action.edit_revision, current_description: 'server draft', expected_seq: 5,
          }})), 0);
        });
        lificIssueEditor.mount(document.querySelector('[data-topcoat-issue-editor]'), {
          route: {issue_id: 31, generation: 1}, text: 'before', saved_description: 'before', expected_seq: 4,
          capabilities: {edit: true}, debounce_ms: 10,
        });
      });
      await page.locator('[data-editor-edit]').click();
      await page.locator('[data-editor-input]').fill('draft');
      await page.locator('[data-editor-conflict]').waitFor({state: 'visible'});
      assert.equal(await page.locator('[data-editor-server-value]').textContent(), 'server draft');
      assert.equal(await page.locator('[data-editor-input]').inputValue(), 'draft');
    } finally {await browser.close();}
  });

test('headless editor renders safe image markdown and scopes attachment URLs',
  {skip: !process.env.PLAYWRIGHT_EXECUTABLE_PATH}, async () => {
    const {chromium} = await import(path.resolve(__dirname, '../../../../../e2e/node_modules/playwright/index.mjs'));
    const browser = await chromium.launch({headless: true, executablePath: process.env.PLAYWRIGHT_EXECUTABLE_PATH});
    try {
      const page = await browser.newPage();
      await page.setContent('<base href="https://lific.local/"><article id="preview"></article>');
      await page.addScriptTag({content: fs.readFileSync(`${__dirname}/editor.js`, 'utf8')});
      await page.evaluate(() => {
        window.lificSession = {resolve: path => ({kind: 'public', url: `/public/api/projects/LIF${path}`})};
        lificIssueEditor.renderMarkdown(document.querySelector('#preview'),
          '![Screenshot](/api/attachments/9) ![Unsafe](javascript:alert(1))');
      });
      const image = page.locator('#preview img');
      assert.equal(await image.count(), 1);
      assert.equal(await image.getAttribute('alt'), 'Screenshot');
      assert.equal(await image.getAttribute('loading'), 'lazy');
      assert.equal(await image.getAttribute('referrerpolicy'), 'no-referrer');
      assert.match(await image.getAttribute('src'), /\/public\/api\/projects\/LIF\/attachments\/9$/);
      assert.equal(await page.locator('#preview').textContent().then(text => text.includes('javascript:alert(1)')), true);
    } finally {await browser.close();}
  });

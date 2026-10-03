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
          <button type="button" data-editor-save>Save</button><button type="button" data-editor-cancel>Cancel</button></div>
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

test('headless editor serializes explicit saves and preserves edits during the active write',
  {skip: !process.env.PLAYWRIGHT_EXECUTABLE_PATH}, async () => {
    const {chromium} = await import(path.resolve(__dirname, '../../../../../e2e/node_modules/playwright/index.mjs'));
    const browser = await chromium.launch({headless: true, executablePath: process.env.PLAYWRIGHT_EXECUTABLE_PATH});
    try {
      const page = await browser.newPage();
      page.setDefaultTimeout(4000);
      await page.setContent(`<section data-topcoat-issue-editor>
        <div><button data-editor-edit>Edit</button><button data-editor-preview-toggle>Preview</button><button data-editor-save>Save</button><button data-editor-cancel>Cancel</button></div>
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
      await page.locator('[data-editor-save]').click();
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

test('headless editor preserves the caret when the route observes an already-dirty draft',
  {skip: !process.env.PLAYWRIGHT_EXECUTABLE_PATH}, async () => {
    const {chromium} = await import(path.resolve(__dirname, '../../../../../e2e/node_modules/playwright/index.mjs'));
    const browser = await chromium.launch({headless: true, executablePath: process.env.PLAYWRIGHT_EXECUTABLE_PATH});
    try {
      const page = await browser.newPage();
      await page.setContent(`<section data-topcoat-issue-editor>
        <button data-editor-edit>Edit</button><button data-editor-preview-toggle>Preview</button><button data-editor-save>Save</button><button data-editor-cancel>Cancel</button>
        <p data-editor-status></p><p data-editor-error hidden></p>
        <section data-editor-conflict hidden><p data-editor-conflict-message></p><pre data-editor-server-value></pre></section>
        <textarea data-editor-input></textarea><article data-editor-preview hidden></article></section>`);
      await page.addScriptTag({content: fs.readFileSync(`${__dirname}/editor.js`, 'utf8')});
      await page.evaluate(() => {
        window.addEventListener('lific:issue-detail-intent', event => {
          const {route, action} = event.detail;
          if (action.type === 'edit_description') window.editor.update({route,text:action.description,saved_description:'base',expected_seq:4});
        });
        window.editor = lificIssueEditor.mount(document.querySelector('[data-topcoat-issue-editor]'), {
          route: {issue_id: 31, generation: 1}, text: 'base', saved_description: 'base',
          expected_seq: 4, capabilities: {edit: true}, debounce_ms: 10000,
        });
      });
      await page.locator('[data-editor-edit]').click();
      const input = page.locator('[data-editor-input]');
      await input.press('End'); await input.press('X');
      await input.evaluate(node => node.setSelectionRange(2, 2));
      await input.press('Y');
      assert.equal(await input.inputValue(), 'baYseX');
      assert.equal(await input.evaluate(node => node.selectionStart), 3);
    } finally {await browser.close();}
  });

test('headless editor preserves its draft after an explicit save fails',
  {skip: !process.env.PLAYWRIGHT_EXECUTABLE_PATH}, async () => {
    const {chromium} = await import(path.resolve(__dirname, '../../../../../e2e/node_modules/playwright/index.mjs'));
    const browser = await chromium.launch({headless: true, executablePath: process.env.PLAYWRIGHT_EXECUTABLE_PATH});
    try {
      const page = await browser.newPage();
      await page.setContent(`<section data-topcoat-issue-editor>
        <button data-editor-edit>Edit</button><button data-editor-preview-toggle>Preview</button><button data-editor-save>Save</button><button data-editor-cancel>Cancel</button>
        <p data-editor-status></p><p data-editor-error hidden></p>
        <section data-editor-conflict hidden><p data-editor-conflict-message></p><pre data-editor-server-value></pre></section>
        <textarea data-editor-input></textarea><article data-editor-preview hidden></article></section>`);
      await page.addScriptTag({content: fs.readFileSync(`${__dirname}/editor.js`, 'utf8')});
      await page.evaluate(() => {
        window.addEventListener('lific:issue-detail-intent', event => {
          const {route, action} = event.detail;
          if (action.type !== 'save_description') return;
          setTimeout(() => dispatchEvent(new CustomEvent('lific:issue-detail-error', {detail: {
            route, edit_revision: action.edit_revision, error: 'Offline during save',
          }})), 0);
        });
        lificIssueEditor.mount(document.querySelector('[data-topcoat-issue-editor]'), {
          route: {issue_id: 31, generation: 1}, text: '', saved_description: '', expected_seq: 4,
          capabilities: {edit: true}, debounce_ms: 10,
        });
      });
      await page.locator('[data-editor-edit]').click();
      await page.locator('[data-editor-input]').fill('draft');
      await page.locator('[data-editor-save]').click();
      await page.locator('[data-editor-error]').waitFor({state: 'visible'});
      assert.equal(await page.locator('[data-editor-error]').textContent(), 'Offline during save');
      assert.equal(await page.locator('[data-editor-input]').inputValue(), 'draft');
    } finally {await browser.close();}
  });

test('headless editor resolves an explicit save conflict when the draft matches the server and permits uploads',
  {skip: !process.env.PLAYWRIGHT_EXECUTABLE_PATH}, async () => {
    const {chromium} = await import(path.resolve(__dirname, '../../../../../e2e/node_modules/playwright/index.mjs'));
    const browser = await chromium.launch({headless: true, executablePath: process.env.PLAYWRIGHT_EXECUTABLE_PATH});
    try {
      const page = await browser.newPage();
      await page.setContent(`<section data-topcoat-issue-editor>
        <button data-editor-edit>Edit</button><button data-editor-preview-toggle>Preview</button><button data-editor-save>Save</button><button data-editor-cancel>Cancel</button>
        <p data-editor-status></p><p data-editor-error hidden></p>
        <section data-editor-conflict hidden><p data-editor-conflict-message></p><pre data-editor-server-value></pre></section>
        <section data-editor-attachments><form data-attachment-upload>
          <input type="file" data-attachment-files><button type="submit">Upload</button>
          <p data-attachment-status></p></form></section>
        <textarea data-editor-input></textarea><article data-editor-preview hidden></article></section>`);
      await page.addScriptTag({content: fs.readFileSync(`${__dirname}/editor.js`, 'utf8')});
      await page.evaluate(() => {
        window.uploadSubmissions = 0;
        window.LificTopcoatAttachments = {
          createClient() {return {};},
          attach(root) {
            root.querySelector('form').addEventListener('submit', event => {event.preventDefault(); uploadSubmissions++;});
            return {dispose() {}};
          },
        };
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
      await page.locator('[data-editor-save]').click();
      await page.locator('[data-editor-conflict]').waitFor({state: 'visible'});
      assert.equal(await page.locator('[data-editor-server-value]').textContent(), 'server draft');
      assert.equal(await page.locator('[data-editor-input]').inputValue(), 'draft');
      await page.locator('[data-attachment-upload]').evaluate(form => form.requestSubmit());
      assert.equal(await page.evaluate(() => uploadSubmissions), 0);
      await page.locator('[data-editor-input]').fill('server draft');
      assert.equal(await page.locator('[data-editor-conflict]').isVisible(), false);
      assert.equal(await page.locator('[data-editor-status]').textContent(), 'Saved');
      await page.locator('[data-attachment-upload]').evaluate(form => form.requestSubmit());
      assert.equal(await page.evaluate(() => uploadSubmissions), 1);
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
        window.lificSession = {state: {publicProject: 'LIF'}, resolve: path => ({kind: 'public', url: `/public/api/projects/LIF${path}`})};
        lificIssueEditor.renderMarkdown(document.querySelector('#preview'),
          '![Screenshot](/api/attachments/9) ![External](https://tracker.invalid/pixel.png) ![Unsafe](javascript:alert(1))\n[Download](/api/attachments/9)');
      });
      const image = page.locator('#preview img');
      assert.equal(await image.count(), 1);
      assert.equal(await image.getAttribute('alt'), 'Screenshot');
      assert.equal(await image.getAttribute('loading'), 'lazy');
      assert.equal(await image.getAttribute('referrerpolicy'), 'no-referrer');
      assert.match(await image.getAttribute('src'), /\/public\/api\/projects\/LIF\/attachments\/9$/);
      assert.equal(await page.locator('#preview').textContent().then(text => text.includes('javascript:alert(1)')), true);
      assert.match(await page.locator('#preview a').getAttribute('href'), /\/public\/api\/projects\/LIF\/attachments\/9$/);
      assert.equal(await page.locator('#preview a').textContent(), 'Download');
    } finally {await browser.close();}
  });

test('headless editor preserves GFM tables, task checkboxes, and nested lists',
  {skip: !process.env.PLAYWRIGHT_EXECUTABLE_PATH}, async () => {
    const {chromium} = await import(path.resolve(__dirname, '../../../../../e2e/node_modules/playwright/index.mjs'));
    const browser = await chromium.launch({headless: true, executablePath: process.env.PLAYWRIGHT_EXECUTABLE_PATH});
    try {
      const page = await browser.newPage();
      await page.setContent('<article id="preview"></article>');
      await page.addScriptTag({content: fs.readFileSync(`${__dirname}/editor.js`, 'utf8')});
      await page.evaluate(() => lificIssueEditor.renderMarkdown(document.querySelector('#preview'), [
        '| Item | State |', '| :--- | ---: |', '| **Desk** | ready |', '',
        '- [x] finished', '- [ ] pending', '  - child', '    1. grandchild', '  - sibling',
      ].join('\n')));
      assert.equal(await page.locator('table thead th').count(), 2);
      assert.equal(await page.locator('table tbody tr').count(), 1);
      assert.equal(await page.locator('table tbody strong').textContent(), 'Desk');
      assert.equal(await page.locator('ul input[type=checkbox]').count(), 2);
      assert.equal(await page.locator('ul input[type=checkbox]').nth(0).isChecked(), true);
      assert.equal(await page.locator('ul input[type=checkbox]').nth(1).isChecked(), false);
      assert.equal(await page.locator('ul ul li').count(), 3);
      assert.equal(await page.locator('ul ul ol li').textContent(), 'grandchild');
    } finally {await browser.close();}
  });

test('headless editor advances past malformed tables with mismatched column counts',
  {skip: !process.env.PLAYWRIGHT_EXECUTABLE_PATH}, async () => {
    const {chromium} = await import(path.resolve(__dirname, '../../../../../e2e/node_modules/playwright/index.mjs'));
    const browser = await chromium.launch({headless: true, executablePath: process.env.PLAYWRIGHT_EXECUTABLE_PATH});
    try {
      const page = await browser.newPage();
      page.setDefaultTimeout(2000);
      await page.setContent('<article id="preview"></article>');
      await page.addScriptTag({content: fs.readFileSync(`${__dirname}/editor.js`, 'utf8')});
      await page.evaluate(() => lificIssueEditor.renderMarkdown(document.querySelector('#preview'), 'a | b\n--- | --- | ---\ncell | row'));
      assert.equal(await page.locator('#preview').textContent(), 'a | b--- | --- | ---cell | row');
    } finally {await browser.close();}
  });

test('headless editor formatting shortcuts preserve selection and suppress the palette on Shift+K',
  {skip: !process.env.PLAYWRIGHT_EXECUTABLE_PATH}, async () => {
    const {chromium} = await import(path.resolve(__dirname, '../../../../../e2e/node_modules/playwright/index.mjs'));
    const browser = await chromium.launch({headless: true, executablePath: process.env.PLAYWRIGHT_EXECUTABLE_PATH});
    try {
      const page = await browser.newPage();
      await page.setContent(`<section data-topcoat-issue-editor>
        <button data-editor-edit>Edit</button><button data-editor-preview-toggle>Preview</button><button data-editor-save>Save</button><button data-editor-cancel>Cancel</button>
        <p data-editor-status></p><p data-editor-error hidden></p>
        <section data-editor-conflict hidden><p data-editor-conflict-message></p><pre data-editor-server-value></pre></section>
        <textarea data-editor-input></textarea><article data-editor-preview hidden></article></section>`);
      await page.addScriptTag({content: fs.readFileSync(`${__dirname}/editor.js`, 'utf8')});
      await page.evaluate(() => {
        window.paletteCalls = 0;
        window.addEventListener('keydown', event => {if ((event.ctrlKey || event.metaKey) && event.shiftKey && event.key.toLowerCase() === 'k') paletteCalls++;});
        lificIssueEditor.mount(document.querySelector('[data-topcoat-issue-editor]'), {
          route: {issue_id: 31, generation: 1}, text: 'word', saved_description: 'word', capabilities: {edit: true}, debounce_ms: 10000,
        });
      });
      await page.locator('[data-editor-edit]').click();
      const input = page.locator('[data-editor-input]');
      await input.evaluate(node => node.setSelectionRange(0, 4));
      await input.press('Control+B');
      assert.equal(await input.inputValue(), '**word**');
      assert.equal(await input.evaluate(node => node.selectionStart === 2 && node.selectionEnd === 6), true);
      await input.press('Control+I');
      assert.equal(await input.inputValue(), '***word***');
      await input.press('Control+Shift+K');
      assert.equal(await input.inputValue(), '***[word](URL)***');
      assert.equal(await input.evaluate(node => node.value.slice(node.selectionStart, node.selectionEnd)), 'URL');
      assert.equal(await page.evaluate(() => paletteCalls), 0);
    } finally {await browser.close();}
  });

test('headless editor inserts uploaded attachment markdown at the selection and gates saves',
  {skip: !process.env.PLAYWRIGHT_EXECUTABLE_PATH}, async () => {
    const {chromium} = await import(path.resolve(__dirname, '../../../../../e2e/node_modules/playwright/index.mjs'));
    const browser = await chromium.launch({headless: true, executablePath: process.env.PLAYWRIGHT_EXECUTABLE_PATH});
    try {
      const page = await browser.newPage();
      await page.setContent(`<section data-topcoat-issue-editor>
        <button data-editor-edit>Edit</button><button data-editor-preview-toggle>Preview</button><button data-editor-save>Save</button><button data-editor-cancel>Cancel</button>
        <section data-editor-attachments><form data-attachment-upload>
          <input type="file" data-attachment-files multiple><button type="submit">Upload</button>
          <button type="button" data-attachment-cancel hidden>Cancel</button><progress data-attachment-progress hidden></progress>
          <p data-attachment-status role="status"></p></form></section>
        <p data-editor-status></p><p data-editor-error hidden></p>
        <section data-editor-conflict hidden><p data-editor-conflict-message></p><pre data-editor-server-value></pre></section>
        <textarea data-editor-input></textarea><article data-editor-preview hidden></article></section>`);
      await page.addScriptTag({content: fs.readFileSync(`${__dirname}/editor.js`, 'utf8')});
      await page.evaluate(() => {
        window.lificSession = {state: {publicProject: null}, resolve: path => ({kind: 'private', url: `/api${path}`})};
        window.uploadContract = null; window.saves = [];
        window.LificTopcoatAttachments = {
          createClient({session}) {return {session};},
          attach(root, options) {
            const form = root.querySelector('form'), input = root.querySelector('[data-attachment-files]');
            const progress = root.querySelector('[data-attachment-progress]');
            window.uploadContract = options;
            form.addEventListener('submit', async event => {
              event.preventDefault(); input.disabled = true; progress.hidden = false;
              await new Promise(resolve => {window.finishUpload = resolve;});
              await options.onUploaded({id: 17}, '![sample.png](/api/attachments/17)');
              input.disabled = false; progress.hidden = true;
            });
            return {dispose() {}};
          },
        };
        window.addEventListener('lific:issue-detail-intent', event => {
          const {route, action} = event.detail;
          if (action.type !== 'save_description') return;
          saves.push(action);
          setTimeout(() => dispatchEvent(new CustomEvent('lific:issue-detail-applied', {detail: {
            route, kind: 'editor', description: action.description, expected_seq: action.expected_seq + 1,
            edit_revision: action.edit_revision,
          }})), 0);
        });
        window.editor = lificIssueEditor.mount(document.querySelector('[data-topcoat-issue-editor]'), {
          route: {issue_id: 31, generation: 1}, text: 'one two', saved_description: 'one two',
          expected_seq: 4, capabilities: {edit: true}, debounce_ms: 10000,
        });
      });
      assert.equal(await page.evaluate(() => uploadContract.target.entity_type), 'issue');
      assert.equal(await page.evaluate(() => uploadContract.target.entity_id), 31);
      await page.locator('[data-editor-edit]').click();
      const input = page.locator('[data-editor-input]');
      await input.evaluate(node => {node.focus(); node.setSelectionRange(4, 7); node.dispatchEvent(new Event('select'));});
      await page.locator('[data-attachment-files]').setInputFiles({name: 'sample.png', mimeType: 'image/png', buffer: Buffer.from('image')});
      await page.locator('[data-attachment-upload]').evaluate(form => form.requestSubmit());
      await page.locator('[data-attachment-progress]').waitFor({state: 'visible'});
      assert.equal(await page.locator('[data-editor-save]').isDisabled(), true);
      await input.focus(); await input.press('Control+S');
      assert.equal(await page.evaluate(() => saves.length), 0);
      await page.evaluate(() => finishUpload());
      await page.waitForFunction(() => document.querySelector('[data-editor-input]').value === 'one \n![sample.png](/api/attachments/17)');
      await page.locator('[data-attachment-files]').evaluate(async input => {
        while (document.querySelector('[data-editor-save]').disabled) await new Promise(resolve => setTimeout(resolve, 10));
      });
      assert.equal(await page.locator('[data-editor-save]').isDisabled(), false);
      assert.equal(await input.evaluate(node => node.selectionStart === node.selectionEnd), true);
      await page.locator('[data-editor-save]').click();
      await page.waitForFunction(() => saves.length === 1);
      assert.match(await page.evaluate(() => saves[0].description), /!\[sample\.png\]\(\/api\/attachments\/17\)/);
    } finally {await browser.close();}
  });


test('headless editor saves explicitly and Cancel or Escape discard the draft',
  {skip: !process.env.PLAYWRIGHT_EXECUTABLE_PATH}, async () => {
    const {chromium} = await import(path.resolve(__dirname, '../../../../../e2e/node_modules/playwright/index.mjs'));
    const browser = await chromium.launch({headless: true, executablePath: process.env.PLAYWRIGHT_EXECUTABLE_PATH});
    try {
      const page = await browser.newPage();
      await page.setContent(`<section data-topcoat-issue-editor>
        <button data-editor-edit>Edit</button><button data-editor-preview-toggle>Preview</button>
        <button data-editor-save>Save</button><button data-editor-cancel>Cancel</button>
        <section data-editor-attachments><form data-attachment-upload><input type="file" data-attachment-files><p data-attachment-status></p></form></section>
        <p data-editor-status></p><p data-editor-error hidden></p>
        <section data-editor-conflict hidden><p data-editor-conflict-message></p><pre data-editor-server-value></pre></section>
        <textarea data-editor-input></textarea><article data-editor-preview hidden></article></section>`);
      await page.addScriptTag({content: fs.readFileSync(`${__dirname}/editor.js`, 'utf8')});
      await page.evaluate(() => {
        window.saves = []; window.attachmentCancels = 0;
        window.LificTopcoatAttachments = {
          createClient() {return {};},
          attach(root) {return {
            cancel() {attachmentCancels++; root.dispatchEvent(new CustomEvent('lific:attachment-busy', {bubbles:true, detail:{busy:false}}));},
            dispose() {},
          };},
        };
        window.addEventListener('lific:issue-detail-intent', event => {
          const {route, action} = event.detail;
          if (action.type !== 'save_description') return;
          saves.push(action.description);
          dispatchEvent(new CustomEvent('lific:issue-detail-applied', {detail: {
            route, kind: 'editor', description: action.description, expected_seq: 5, edit_revision: action.edit_revision,
          }}));
        });
        window.editor = lificIssueEditor.mount(document.querySelector('[data-topcoat-issue-editor]'), {
          route: {issue_id: 31, generation: 1}, text: 'saved', saved_description: 'saved', expected_seq: 4,
          capabilities: {edit: true},
        });
      });
      const input = page.locator('[data-editor-input]');
      await page.locator('[data-editor-edit]').click();
      await input.fill('draft');
      await page.waitForTimeout(700);
      assert.deepEqual(await page.evaluate(() => saves), []);
      await page.locator('[data-editor-attachments]').evaluate(root => root.dispatchEvent(new CustomEvent('lific:attachment-busy', {bubbles:true, detail:{busy:true}})));
      assert.equal(await page.locator('[data-editor-save]').isDisabled(), true);
      await input.press('Control+S');
      assert.deepEqual(await page.evaluate(() => saves), []);
      await page.locator('[data-editor-cancel]').click();
      assert.equal(await page.evaluate(() => attachmentCancels), 1);
      assert.equal(await input.inputValue(), 'saved');
      assert.equal(await input.isVisible(), false);
      await page.locator('[data-editor-edit]').click();
      await input.fill('discard with Escape');
      await input.press('Escape');
      assert.equal(await input.inputValue(), 'saved');
      assert.equal(await input.isVisible(), false);
      assert.deepEqual(await page.evaluate(() => saves), []);
      await page.locator('[data-editor-edit]').click();
      await input.fill('commit');
      await page.locator('[data-editor-save]').click();
      await page.waitForFunction(() => document.querySelector('[data-editor-input]').hidden);
      assert.deepEqual(await page.evaluate(() => saves), ['commit']);
      await page.locator('[data-editor-edit]').click();
      await input.fill('shortcut');
      await input.press('Control+S');
      await page.waitForFunction(() => document.querySelector('[data-editor-input]').hidden);
      assert.deepEqual(await page.evaluate(() => saves), ['commit', 'shortcut']);
    } finally {await browser.close();}
  });

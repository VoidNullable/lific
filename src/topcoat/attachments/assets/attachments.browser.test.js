const {test} = require('node:test');
const assert = require('node:assert/strict');
const fs = require('node:fs');
const path = require('node:path');
test('headless attachment composer uploads complete targets and presents server errors without losing retry file', {skip: !process.env.PLAYWRIGHT_EXECUTABLE_PATH}, async () => {
  const {chromium} = await import(path.resolve(__dirname, '../../../..', 'e2e/node_modules/playwright/index.mjs'));
  const browser = await chromium.launch({headless: true, executablePath: process.env.PLAYWRIGHT_EXECUTABLE_PATH});
  try {
    const page = await browser.newPage();
    page.setDefaultTimeout(5000);
    const uploads = [];
    await page.route('http://lific.test/**', async route => {
      const request = route.request();
      if (request.url().endsWith('/api/attachments')) {
        uploads.push({headers: request.headers(), body: request.postData()});
        await route.fulfill(uploads.length === 1 ? {status: 403, contentType: 'application/json', body: JSON.stringify({error: 'Cannot attach to this comment.'})} : {status: 200, contentType: 'application/json', body: JSON.stringify({id: 9, filename: 'notes.txt', mime: 'text/plain', size: 5, url: '/api/attachments/9'})});
      } else await route.fulfill({contentType: 'text/html', body: '<!doctype html><html><head><title>Attachments</title></head><body><section data-topcoat-attachments><form data-attachment-upload><label>Attach files <input type="file" data-attachment-files></label><button type="submit">Upload</button><button type="button" data-attachment-cancel hidden>Cancel</button><progress data-attachment-progress max="1" value="0" hidden></progress><p data-attachment-status role="status" aria-live="polite"></p></form></section></body></html>'});
    });
    await page.goto('http://lific.test/LIF/issues/LIF-7');
    await page.addScriptTag({content: fs.readFileSync(`${__dirname}/attachments.js`, 'utf8')});
    await page.evaluate(() => {
      localStorage.setItem('lific_token', 'private-token');
      window.session = {state: {user: {id: 1}, publicProject: null}, resolve: (path, method) => ({kind: 'private', url: `/api${path}`}), request() {}, clearSession() {}};
      window.uploaded = [];
      window.attachments = LificTopcoatAttachments.attach(document.querySelector('[data-topcoat-attachments]'), {client: LificTopcoatAttachments.createClient({session: window.session}), target: {entity_type: 'comment', entity_id: 42}, onUploaded: row => window.uploaded.push(row)});
    });
    await page.locator('input[type=file]').setInputFiles({name: 'notes.txt', mimeType: 'text/plain', buffer: Buffer.from('hello')});
    await page.getByRole('button', {name: 'Upload', exact: true}).click();
    await page.waitForFunction(() => document.querySelector('[data-attachment-status]').textContent === 'Cannot attach to this comment.');
    assert.equal(await page.locator('input[type=file]').evaluate(input => input.files[0].name), 'notes.txt');
    await page.getByRole('button', {name: 'Upload', exact: true}).click();
    await page.waitForFunction(() => window.uploaded.length === 1);
    assert.equal(uploads.length, 2);
    assert.equal(uploads[0].headers.authorization, 'Bearer private-token');
    assert.match(uploads[0].headers['content-type'], /^multipart\/form-data; boundary=/);
    assert.match(uploads[0].body, /name="entity_type"\r\n\r\ncomment/);
    assert.match(uploads[0].body, /name="entity_id"\r\n\r\n42/);
    assert.equal(await page.locator('[data-attachment-status]').textContent(), 'Uploaded notes.txt.');
    await page.evaluate(() => window.attachments.dispose());
  } finally {await browser.close();}
});
test('headless upload queue survives same-account notifications and retries only unfinished files after cancel', {skip: !process.env.PLAYWRIGHT_EXECUTABLE_PATH}, async () => {
  const {chromium} = await import(path.resolve(__dirname, '../../../..', 'e2e/node_modules/playwright/index.mjs'));
  const browser = await chromium.launch({headless: true, executablePath: process.env.PLAYWRIGHT_EXECUTABLE_PATH});
  try {
    const page = await browser.newPage();
    page.setDefaultTimeout(5000);
    await page.setContent('<section data-topcoat-attachments><form data-attachment-upload><input type="file" multiple data-attachment-files><button type="submit">Upload</button><button type="button" data-attachment-cancel hidden>Cancel</button><progress data-attachment-progress hidden></progress><p data-attachment-status role="status"></p></form></section>');
    await page.addScriptTag({content: fs.readFileSync(`${__dirname}/attachments.js`, 'utf8')});
    await page.evaluate(() => {
      window.calls = []; window.uploaded = []; window.aborted = 0;
      const client = {audience: () => 'same', upload(file) {
        window.calls.push(file.name);
        let finish;
        const result = file.name === 'one.txt' ? Promise.resolve({ok: true, data: {id: 1, filename: file.name, mime: 'text/plain'}}) : new Promise(resolve => {finish = resolve;});
        return {result, abort() {window.aborted++; finish?.({ok: false, canceled: true, error: 'Canceled'});}};
      }};
      window.component = LificTopcoatAttachments.attach(document.querySelector('[data-topcoat-attachments]'), {client, onUploaded: row => uploaded.push(row)});
    });
    await page.locator('input').setInputFiles([{name: 'one.txt', mimeType: 'text/plain', buffer: Buffer.from('one')}, {name: 'two.txt', mimeType: 'text/plain', buffer: Buffer.from('two')}]);
    await page.getByRole('button', {name: 'Upload', exact: true}).click();
    await page.waitForFunction(() => window.calls.length === 2 && window.uploaded.length === 1);
    await page.evaluate(() => dispatchEvent(new CustomEvent('lific:account-change')));
    assert.equal(await page.evaluate(() => window.aborted), 0);
    await page.getByRole('button', {name: 'Cancel'}).click();
    await page.getByRole('button', {name: 'Upload', exact: true}).click();
    await page.waitForFunction(() => window.calls.length === 3);
    assert.deepEqual(await page.evaluate(() => window.calls), ['one.txt', 'two.txt', 'two.txt']);
    await page.evaluate(() => window.component.dispose());
  } finally {await browser.close();}
});
test('headless upload cancellation leaves pending thumbnail, text preview and delete operations live', {skip: !process.env.PLAYWRIGHT_EXECUTABLE_PATH}, async () => {
  const {chromium} = await import(path.resolve(__dirname, '../../../..', 'e2e/node_modules/playwright/index.mjs'));
  const browser = await chromium.launch({headless: true, executablePath: process.env.PLAYWRIGHT_EXECUTABLE_PATH});
  try {
    const page = await browser.newPage();
    page.setDefaultTimeout(5000);
    const errors = [];
    page.on('pageerror', error => errors.push(error.message));
    await page.setContent('<section data-topcoat-attachments><form data-attachment-upload><input type="file" data-attachment-files><button type="submit">Upload</button><button type="button" data-attachment-cancel hidden>Cancel</button><progress data-attachment-progress hidden></progress><p data-attachment-status role="status"></p></form><ul><li data-attachment-id="8"><img data-attachment-image="8" alt="Image"><p data-attachment-message></p></li><li data-attachment-id="9" data-attachment-kind="text"><button data-attachment-preview>Preview</button><pre data-attachment-content hidden></pre><p data-attachment-message></p></li><li data-attachment-id="10"><button data-attachment-delete="10">Delete</button><p data-attachment-message></p></li></ul></section>');
    await page.addScriptTag({content: fs.readFileSync(`${__dirname}/attachments.js`, 'utf8')});
    await page.evaluate(() => {
      window.pending = {};
      const client = {audience: () => 'same', url: id => `/api/attachments/${id}`, thumbnail() {return new Promise(resolve => {pending.thumbnail = resolve;});}, text() {return new Promise(resolve => {pending.text = resolve;});}, remove() {return new Promise(resolve => {pending.remove = resolve;});}, upload() {let finish; return {result: new Promise(resolve => {finish = resolve;}), abort() {finish({ok: false, canceled: true, error: 'Canceled'});}};}};
      window.component = LificTopcoatAttachments.attach(document.querySelector('section'), {client, onDeleted: async () => {throw Error('host refresh failed');}});
    });
    await page.getByRole('button', {name: 'Preview', exact: true}).click();
    await page.getByRole('button', {name: 'Delete', exact: true}).click();
    await page.locator('input').setInputFiles({name: 'new.txt', mimeType: 'text/plain', buffer: Buffer.from('new')});
    await page.getByRole('button', {name: 'Upload', exact: true}).click();
    await page.getByRole('button', {name: 'Cancel', exact: true}).click();
    await page.evaluate(() => {pending.thumbnail({ok: true, blob: new Blob(['thumbnail'], {type: 'image/webp'})}); pending.text({ok: true, text: 'Preview survives.'}); pending.remove({ok: true});});
    await page.waitForFunction(() => document.querySelector('img').src.startsWith('blob:') && document.querySelector('pre').textContent === 'Preview survives.' && !document.querySelector('[data-attachment-id="10"]'));
    assert.equal(await page.getByRole('button', {name: 'Preview', exact: true}).isEnabled(), true);
    assert.match(await page.locator('[data-attachment-status]').textContent(), /host refresh failed/);
    assert.deepEqual(errors, []);
    await page.evaluate(() => component.dispose());
  } finally {await browser.close();}
});
test('headless disposal aborts pending thumbnail, text and delete fetches and restores controls', {skip: !process.env.PLAYWRIGHT_EXECUTABLE_PATH}, async () => {
  const {chromium} = await import(path.resolve(__dirname, '../../../..', 'e2e/node_modules/playwright/index.mjs'));
  const browser = await chromium.launch({headless: true, executablePath: process.env.PLAYWRIGHT_EXECUTABLE_PATH});
  try {
    const page = await browser.newPage();
    page.setDefaultTimeout(5000);
    await page.setContent('<section><li data-attachment-id="8"><img data-attachment-image="8"><p data-attachment-message></p></li><li data-attachment-id="9" data-attachment-kind="text"><button data-attachment-preview>Preview</button><pre data-attachment-content hidden></pre><p data-attachment-message></p></li><li data-attachment-id="10"><button data-attachment-delete="10">Delete</button><p data-attachment-message></p></li></section>');
    await page.addScriptTag({content: fs.readFileSync(`${__dirname}/attachments.js`, 'utf8')});
    await page.evaluate(() => {
      window.aborted = [];
      const pending = (name, {signal}) => new Promise((resolve, reject) => signal.addEventListener('abort', () => {aborted.push(name); reject(signal.reason);}));
      const client = {audience: () => 'same', thumbnail: (id, options) => pending('thumbnail', options), text: (id, options) => pending('text', options), remove: (id, options) => pending('delete', options)};
      window.component = LificTopcoatAttachments.attach(document.querySelector('section'), {client});
    });
    await page.getByRole('button', {name: 'Preview', exact: true}).click();
    await page.getByRole('button', {name: 'Delete', exact: true}).click();
    await page.evaluate(() => component.dispose());
    await page.waitForFunction(() => aborted.length === 3);
    assert.deepEqual((await page.evaluate(() => aborted)).sort(), ['delete', 'text', 'thumbnail']);
    assert.equal(await page.getByRole('button', {name: 'Preview', exact: true}).isEnabled(), true);
    assert.equal(await page.getByRole('button', {name: 'Delete', exact: true}).isEnabled(), true);
  } finally {await browser.close();}
});
test('headless native originals use session cookie while thumbnail and structured preview requests use bearer', {skip: !process.env.PLAYWRIGHT_EXECUTABLE_PATH}, async () => {
  const {chromium} = await import(path.resolve(__dirname, '../../../..', 'e2e/node_modules/playwright/index.mjs'));
  const http = require('node:http');
  const requests = [];
  const server = http.createServer((request, response) => {
    const pathname = new URL(request.url, 'http://localhost').pathname;
    requests.push({pathname, headers: request.headers});
    if (pathname.endsWith('/thumbnail')) {response.writeHead(404, {'Content-Type': 'application/json'}); response.end('{"error":"no thumbnail"}');}
    else if (pathname.endsWith('/preview')) {response.writeHead(200, {'Content-Type': 'application/json'}); response.end('{"kind":"none"}');}
    else if (pathname === '/api/attachments/8') {response.writeHead(200, {'Content-Type': 'image/png'}); response.end(Buffer.from('iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAQAAAC1HAwCAAAAC0lEQVR42mNk+A8AAQUBAScY42YAAAAASUVORK5CYII=', 'base64'));}
    else if (pathname === '/api/attachments/10') {response.writeHead(206, {'Content-Type': 'video/mp4', 'Accept-Ranges': 'bytes', 'Content-Range': 'bytes 0-3/4'}); response.end(Buffer.from([0, 0, 0, 0]));}
    else if (pathname === '/api/attachments/11') {response.writeHead(200, {'Content-Type': 'text/plain', 'Content-Disposition': 'attachment; filename="server-name.txt"'}); response.end('downloaded');}
    else {response.writeHead(200, {'Content-Type': 'text/html'}); response.end('<!doctype html><section><li data-attachment-id="8"><a href="/api/attachments/8">Original image</a><img data-attachment-image="8" alt="Thumbnail fallback"><p data-attachment-message></p></li><li data-attachment-id="9" data-attachment-kind="zip"><button data-attachment-preview>Preview</button><pre data-attachment-content hidden></pre><p data-attachment-message></p></li><video src="/api/attachments/10" preload="metadata" controls></video><a href="/api/attachments/11" download="server-name.txt">Download report</a></section>');}
  });
  await new Promise(resolve => server.listen(0, '127.0.0.1', resolve));
  const base = `http://127.0.0.1:${server.address().port}`;
  const browser = await chromium.launch({headless: true, executablePath: process.env.PLAYWRIGHT_EXECUTABLE_PATH});
  try {
    const context = await browser.newContext();
    await context.addCookies([{name: 'lific_token', value: 'cookie-token', url: base, httpOnly: true, sameSite: 'Lax'}]);
    const page = await context.newPage();
    page.setDefaultTimeout(5000);
    await page.goto(`${base}/LIF/issues/LIF-7`);
    await page.addScriptTag({content: fs.readFileSync(`${__dirname}/attachments.js`, 'utf8')});
    await page.evaluate(() => {
      localStorage.setItem('lific_token', 'bearer-token');
      const session = {state: {user: {id: 1}, publicProject: null}, resolve: path => ({kind: 'private', url: `/api${path}`}), async request(path, {signal} = {}) {const response = await fetch(`/api${path}`, {signal, headers: {Authorization: 'Bearer bearer-token'}}); return {ok: response.ok, data: await response.json()};}, clearSession() {}};
      window.component = LificTopcoatAttachments.attach(document.querySelector('section'), {client: LificTopcoatAttachments.createClient({session})});
    });
    await page.waitForFunction(() => document.querySelector('img').complete && document.querySelector('img').naturalWidth === 1);
    await page.getByRole('button', {name: 'Preview'}).click();
    await page.waitForFunction(() => document.querySelector('pre').textContent.includes('none'));
    const downloadPromise = page.waitForEvent('download');
    await page.getByRole('link', {name: 'Download report'}).click();
    const download = await downloadPromise;
    assert.equal(download.suggestedFilename(), 'server-name.txt');
    assert.equal(await download.failure(), null);
    assert.equal(fs.readFileSync(await download.path(), 'utf8'), 'downloaded');
    assert.equal(requests.find(request => request.pathname.endsWith('/thumbnail')).headers.authorization, 'Bearer bearer-token');
    assert.equal(requests.find(request => request.pathname.endsWith('/preview')).headers.authorization, 'Bearer bearer-token');
    for (const id of [8, 10, 11]) {
      const original = requests.find(request => request.pathname === `/api/attachments/${id}`);
      assert.match(original.headers.cookie, /lific_token=cookie-token/);
      assert.equal(original.headers.authorization, undefined);
    }
    assert.match(requests.find(request => request.pathname === '/api/attachments/10').headers.range, /^bytes=/);
    await page.evaluate(() => component.dispose());
  } finally {await browser.close(); server.closeAllConnections(); await new Promise(resolve => server.close(resolve));}
});

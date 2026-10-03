const {test} = require('node:test');
const assert = require('node:assert/strict');
const fs = require('node:fs');
const vm = require('node:vm');
function fixture(publicProject = null) {
  const requests = [], xhrs = [];
  let token = 'secret';
  const session = {state: {publicProject, user: {id: 1}}, resolve(path, method = 'GET') {
    if (this.state.publicProject && method !== 'GET') return {kind: 'refused'};
    return {kind: this.state.publicProject ? 'public' : 'private', url: this.state.publicProject ? `/public/api/projects/PUB${path}` : `/api${path}`};
  }, request: async (path, options) => {requests.push({path, options}); return {ok: true, data: []};}, clearSession() {token = null;}};
  const win = new EventTarget();
  win.localStorage = {getItem: () => token};
  class XHR {
    constructor() {this.upload = {}; this.headers = {}; xhrs.push(this);}
    open(method, url) {Object.assign(this, {method, url});}
    setRequestHeader(key, value) {this.headers[key] = value;}
    send(form) {this.form = form;}
    abort() {this.onabort();}
    reply(status, body) {Object.assign(this, {status, responseText: JSON.stringify(body)}); this.onload();}
  }
  const context = {globalThis: {}, FormData, Headers, URLSearchParams, TextDecoder, Blob, AbortController, XMLHttpRequest: XHR};
  vm.runInNewContext(fs.readFileSync(`${__dirname}/attachments.js`, 'utf8'), context);
  const api = context.globalThis.LificTopcoatAttachments;
  const client = api.createClient({session, win, createXHR: () => new XHR(), fetch: async (url, options) => {requests.push({url, options}); return response;}});
  let response;
  return {api, client, session, win, requests, xhrs, setResponse(value) {response = value;}};
}
const file = () => new File(['hello'], 'report.txt', {type: 'text/plain'});
test('multipart issue/page/comment targets carry one complete link and browser-owned boundary', async () => {
  for (const entity_type of ['issue', 'page', 'comment']) {
    const f = fixture();
    const transfer = f.client.upload(file(), {target: {entity_type, entity_id: 42}});
    const xhr = f.xhrs[0];
    assert.equal(xhr.form.get('file').name, 'report.txt');
    assert.equal(xhr.form.get('entity_type'), entity_type);
    assert.equal(xhr.form.get('entity_id'), '42');
    assert.deepEqual(xhr.headers, {Authorization: 'Bearer secret'});
    xhr.reply(200, {id: 7, filename: 'report.txt', mime: 'text/plain', size: 5, url: '/api/attachments/7'});
    assert.equal((await transfer.result).data.id, 7);
  }
});
test('public upload/delete refuse before constructing transport and half targets never upload', async () => {
  const f = fixture('PUB');
  assert.equal((await f.client.upload(file()).result).status, 403);
  assert.equal((await f.client.remove(8)).status, 403);
  assert.equal(f.xhrs.length, 0);
  assert.equal(f.requests.length, 0);
  const privateClient = fixture();
  assert.throws(() => privateClient.client.upload(file(), {target: {entity_type: 'issue'}}), /target/);
  assert.equal(privateClient.xhrs.length, 0);
});
test('server forbidden/content rejection and progress/cancel remain separate outcomes', async () => {
  for (const [status, error] of [[403, 'cannot link this comment'], [400, 'rejected content'], [413, 'file too large']]) {
    const f = fixture();
    const progress = [];
    const transfer = f.client.upload(file(), {onProgress: event => progress.push(event)});
    f.xhrs[0].upload.onprogress({loaded: 3, total: 5, lengthComputable: true});
    assert.equal(progress[0].loaded, 3);
    f.xhrs[0].reply(status, {error});
    const result = await transfer.result;
    assert.equal(result.status, status);
    assert.equal(result.error, error);
    assert.equal(result.canceled, false);
  }
  const f = fixture(), transfer = f.client.upload(file());
  transfer.abort();
  assert.equal((await transfer.result).canceled, true);
});
test('original, thumbnail and structured previews resolve separately in private and public scopes', async () => {
  for (const scope of [null, 'PUB']) {
    const f = fixture(scope), base = scope ? '/public/api/projects/PUB' : '/api';
    assert.equal(f.client.url(9, 'original'), `${base}/attachments/9`);
    assert.equal(f.client.url(9, 'thumbnail'), `${base}/attachments/9/thumbnail`);
    await f.client.preview(9);
    assert.equal(f.requests[0].path, '/attachments/9/preview');
    assert.equal(f.api.viewerKind({mime: 'audio/webm', filename: 'clip.webm'}), 'audio');
    assert.equal(f.api.viewerKind({mime: 'text/plain', filename: 'data.csv'}), 'csv');
    assert.equal(f.api.viewerKind({mime: 'application/octet-stream', filename: 'large.log', size_bytes: 20 * 1024 * 1024}), 'file');
  }
});
test('streamed byte-range download preserves metadata and applies backpressure without whole-body reads', async () => {
  const f = fixture();
  let reads = 0, writing = false;
  const chunks = [new Uint8Array([4, 5]), new Uint8Array([6, 7])];
  f.setResponse({ok: true, status: 206, headers: new Headers({'Content-Disposition': "attachment; filename*=UTF-8''r%C3%A9sum%C3%A9.txt", 'Content-Type': 'text/plain', 'Content-Range': 'bytes 4-7/100', 'Accept-Ranges': 'bytes'}), body: {getReader() {return {async read() {assert.equal(writing, false); reads++; return chunks.length ? {done: false, value: chunks.shift()} : {done: true};}, releaseLock() {}, cancel() {}};}}});
  const received = [];
  const result = await f.client.streamDownload(8, {range: 'bytes=4-7', async open(metadata) {
    assert.equal(metadata.filename, 'résumé.txt');
    assert.equal(metadata.contentRange, 'bytes 4-7/100');
    return {async write(chunk) {writing = true; await Promise.resolve(); received.push(...chunk); writing = false;}, close() {}, abort() {}};
  }});
  assert.equal(result.status, 206);
  assert.equal(reads, 3);
  assert.deepEqual(received, [4, 5, 6, 7]);
  assert.equal(f.requests[0].options.headers.get('Range'), 'bytes=4-7');
  assert.equal(f.requests[0].options.headers.get('Authorization'), 'Bearer secret');
});
test('auth failure never opens a download destination; public streams omit credentials', async () => {
  const f = fixture();
  f.setResponse({ok: false, status: 401, json: async () => ({error: 'expired'}), headers: new Headers()});
  const result = await f.client.streamDownload(1, {open() {throw Error('must not save error payload');}});
  assert.equal(result.status, 401);
  assert.equal(result.error, 'expired');
  const p = fixture('PUB');
  p.setResponse({ok: false, status: 404, json: async () => ({error: 'not found'}), headers: new Headers()});
  await p.client.streamDownload(1, {open() {}});
  assert.equal(p.requests[0].options.credentials, 'omit');
  assert.equal(p.requests[0].options.headers.has('Authorization'), false);
});
test('account changes cancel active uploads and streamed partial writes abort destinations', async () => {
  const f = fixture(), transfer = f.client.upload(file());
  f.session.state.user = {id: 2};
  f.win.dispatchEvent(new Event('lific:account-change'));
  assert.equal((await transfer.result).code, 'audience_changed');
  const d = fixture();
  let canceled = false, aborted = false, closed = false;
  d.setResponse({ok: true, status: 200, headers: new Headers(), body: {getReader() {return {async read() {return {done: false, value: new Uint8Array([1])};}, cancel() {canceled = true; return Promise.resolve();}, releaseLock() {}};}}});
  const result = await d.client.streamDownload(1, {open() {return {write() {d.session.state.user = {id: 2};}, close() {closed = true;}, abort() {aborted = true;}};}});
  assert.equal(result.code, 'audience_changed');
  assert.equal(canceled, true);
  assert.equal(aborted, true);
  assert.equal(closed, false);
});
test('inline originals enforce a measured cap and preserve safe markdown labels', async () => {
  const f = fixture();
  let canceled = false;
  f.setResponse({ok: true, status: 200, headers: new Headers({'Content-Type': 'text/plain'}), body: {getReader() {return {async read() {return {done: false, value: new Uint8Array(11 * 1024 * 1024)};}, cancel() {canceled = true; return Promise.resolve();}, releaseLock() {}};}}});
  const result = await f.client.text(8);
  assert.equal(result.ok, false);
  assert.match(result.error, /large/);
  assert.equal(canceled, true);
  assert.equal(f.api.markdown({id: 8, filename: 'a]b.txt', mime: 'text/plain'}), '[a\\]b.txt](/api/attachments/8)');
});
test('range refusal and destination write failures never become saved success responses', async () => {
  const f = fixture();
  f.setResponse({ok: false, status: 416, headers: new Headers({'Content-Range': 'bytes */40'}), json: async () => {throw Error('empty');}});
  const result = await f.client.streamDownload(1, {range: 'bytes=50-', open() {throw Error('must not open');}});
  assert.equal(result.status, 416);
  assert.equal(result.contentRange, 'bytes */40');
  const d = fixture();
  let canceled = false, aborted = false;
  d.setResponse({ok: true, status: 200, headers: new Headers(), body: {getReader() {return {async read() {return {done: false, value: new Uint8Array([1])};}, cancel() {canceled = true; return Promise.resolve();}, releaseLock() {}};}}});
  const failure = await d.client.streamDownload(1, {open() {return {write() {throw Error('disk full');}, abort() {aborted = true;}, close() {throw Error('must not close');}};}});
  assert.equal(failure.ok, false);
  assert.equal(failure.error, 'disk full');
  assert.equal(canceled, true);
  assert.equal(aborted, true);
});
test('abort during blocked destination write cancels the reader and aborts the sink immediately', async () => {
  const f = fixture();
  let canceled = false, aborted = false, finishWrite, writing;
  const startedWriting = new Promise(resolve => {writing = resolve;});
  f.setResponse({ok: true, status: 200, headers: new Headers(), body: {getReader() {return {async read() {return {done: false, value: new Uint8Array([1])};}, cancel() {canceled = true; return Promise.resolve();}, releaseLock() {}};}}});
  const controller = new AbortController();
  const pending = f.client.streamDownload(1, {signal: controller.signal, open() {return {write() {writing(); return new Promise(resolve => {finishWrite = resolve;});}, abort() {aborted = true; finishWrite?.();}, close() {throw Error('must not close');}};}});
  await startedWriting;
  controller.abort();
  await Promise.resolve();
  assert.equal(canceled, true);
  assert.equal(aborted, true);
  assert.equal((await pending).canceled, true);
  assert.equal(f.requests[0].options.signal.aborted, true);
});
test('thumbnail bytes retain WebP content type and bearer while public thumbnails omit credentials', async () => {
  for (const scope of [null, 'PUB']) {
    const f = fixture(scope);
    let sent = false;
    f.setResponse({ok: true, status: 200, headers: new Headers({'Content-Type': 'image/webp'}), body: {getReader() {return {async read() {if (sent) return {done: true}; sent = true; return {done: false, value: new Uint8Array([1,2])};}, cancel() {return Promise.resolve();}, releaseLock() {}};}}});
    const result = await f.client.thumbnail(8);
    assert.equal(result.blob.type, 'image/webp');
    assert.equal(result.blob.size, 2);
    assert.equal(f.requests[0].url.endsWith('/attachments/8/thumbnail'), true);
    assert.equal(f.requests[0].options.headers.get('Authorization'), scope ? null : 'Bearer secret');
    assert.equal(f.requests[0].options.credentials, scope ? 'omit' : 'same-origin');
  }
});
test('throwing destination abort cannot replace the original transfer failure', async () => {
  const f = fixture();
  f.setResponse({ok: true, status: 200, headers: new Headers(), body: {getReader() {return {async read() {return {done: false, value: new Uint8Array([1])};}, cancel() {return Promise.resolve();}, releaseLock() {}};}}});
  const result = await f.client.streamDownload(1, {open() {return {write() {throw Error('write failed');}, abort() {throw Error('cleanup failed');}, close() {throw Error('must not close');}};}});
  assert.equal(result.ok, false);
  assert.equal(result.error, 'write failed');
});
test('image offers preserve resize thresholds, formats, learned caps and safe alt references', () => {
  const {api} = fixture();
  assert.equal(api.decideDownscale({width: 2560, height: 2000, bytes: 20e6, mime: 'image/png'}), null);
  assert.equal(api.decideDownscale({width: 6000, height: 3000, bytes: 5e6, mime: 'image/gif'}), null);
  const offer = api.decideDownscale({width: 6000, height: 3000, bytes: 5e6, mime: 'image/jpeg'});
  assert.equal(offer.width, 2560); assert.equal(offer.height, 1280); assert.equal(offer.reason, 'dimensions');
  assert.equal(api.decideDownscale({width: 3000, height: 2000, bytes: 900, mime: 'image/png'}, 1000).reason, 'size');
  assert.equal(api.parseUploadCap('file too large: 12345 bytes (max 10485760)'), 10485760);
  assert.equal(api.replaceImageAlt('before ![shot.png](/api/attachments/8) after ![other](/api/attachments/9)', 8, ' A [chart]\nwith\ttwo lines '), 'before ![A chart with two lines](/api/attachments/8) after ![other](/api/attachments/9)');
  assert.equal(api.replaceImageAlt('![removed](/api/attachments/9)', 8, 'wrong'), '![removed](/api/attachments/9)');
});
test('crop handles preserve bounds and minimum crop size and large-paste thresholds are strict', () => {
  const {api}=fixture();
  const crop={x:10,y:20,w:100,h:80};
  assert.equal(api.resizeCrop(crop,'nw',{x:200,y:-5},{w:500,h:500}).w,16);
  assert.equal(api.resizeCrop(crop,'nw',{x:200,y:-5},{w:500,h:500}).y,0);
  assert.equal(api.resizeCrop(crop,'se',{x:600,y:600},{w:500,h:500}).w,490);
  assert.equal(api.isBigPaste('x'.repeat(6000)),false);assert.equal(api.isBigPaste('x'.repeat(6001)),true);
  assert.equal(api.isBigPaste(Array(60).fill('x').join('\n')),false);assert.equal(api.isBigPaste(Array(61).fill('x').join('\n')),true);
});

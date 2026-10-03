import test from 'node:test';
import assert from 'node:assert/strict';
import vm from 'node:vm';
import fs from 'node:fs';
const context = {URLSearchParams, encodeURIComponent, console};
vm.runInNewContext(fs.readFileSync(new URL('./issue-create.js', import.meta.url), 'utf8'), context);
const {model, controller, attachmentClient} = context.LificTopcoatIssueCreate;

test('module query accepts only an exact digit-only value present in the project', () => {
  const modules = [{id: 7, name: 'Engine'}];
  assert.equal(model({modules, search: '?module=7'}).moduleId, 7);
  for (const search of ['?module=7x', '?module=-7', '?module=0', '?module=8']) {
    assert.equal(model({modules, search}).moduleId, null);
  }
});

test('status query accepts only supported issue statuses', () => {
  assert.equal(model({search: '?status=done'}).status, 'done');
  assert.equal(model({search: '?status=wat'}).status, 'backlog');
});

test('create waits for a title and all attachment uploads', async () => {
  let posted;
  const env = {api: {request: async (path, options) => {posted = {path, options}; return {ok: true, data: {identifier: 'ENG-12'}};}}, navigate() {}, pendingUploads: () => 1};
  const c = controller(env, 'ENG');
  c.state.phase = 'ready';
  c.state.project = {id: 9, identifier: 'ENG'};
  c.state.title = '  example  ';
  assert.equal(await c.create(), false);
  assert.equal(posted, undefined);
  env.pendingUploads = () => 0;
  assert.equal(await c.create(), true);
  assert.equal(posted.path, '/issues');
  assert.deepEqual(JSON.parse(posted.options.body), {project_id: 9, title: 'example', description: '', status: 'backlog', priority: 'none', labels: []});
});

test('failed create preserves draft and reports the API error', async () => {
  const c = controller({api: {request: async () => ({ok: false, error: 'No access'})}, navigate() {throw new Error('must not navigate');}}, 'ENG');
  c.state.phase = 'ready';
  c.state.project = {id: 9, identifier: 'ENG'};
  c.state.title = ' Draft ';
  c.state.description = 'keep me';
  assert.equal(await c.create(), false);
  assert.equal(c.state.title, ' Draft ');
  assert.equal(c.state.description, 'keep me');
  assert.equal(c.state.error, 'No access');
});

test('discard aborts outstanding upload transfers and returns to project issues', () => {
  const aborted = [];
  let route;
  const c = controller({api: {}, navigate: value => {route = value;}}, 'ENG');
  c.transfers.add({abort: () => aborted.push(true)});
  c.discard();
  assert.deepEqual(aborted, [true]);
  assert.equal(route, '/ENG/issues');
});

test('attachment adapter pairs the upload client with the shared markdown renderer', () => {
  const markdown = value => `[${value.filename}](/api/attachments/${value.id})`;
  const adapter = attachmentClient({createClient: () => ({upload: () => 'transfer'}),markdown}, {});
  assert.equal(adapter.upload(), 'transfer');
  assert.equal(adapter.markdown({id: 4,filename: 'notes.txt'}), '[notes.txt](/api/attachments/4)');
  assert.equal(attachmentClient({}, {}), null);
});

test('project loading resolves metadata and denies viewers before rendering a form', async () => {
  const calls = [];
  const c = controller({api: {request: async path => {
    calls.push(path);
    if (path === '/projects') return {ok: true, data: [{id: 9, identifier: 'ENG'}]};
    if (path.endsWith('/my-role')) return {ok: true, data: {role: 'viewer', enforced: true, is_admin: false}};
    if (path.startsWith('/modules')) return {ok: true, data: [{id: 7, name: 'Engine'}]};
    return {ok: true, data: []};
  }}, navigate() {}}, 'ENG');
  assert.equal(await c.load(), false);
  assert.equal(c.state.phase, 'denied');
  assert.deepEqual(calls, ['/projects', '/projects/9/my-role', '/modules?project_id=9', '/labels?project_id=9']);
});

test('uploaded markdown replaces the textarea selection and leaves the caret after the snippet', async () => {
  const c = controller({
    api: {}, navigate() {},
    attachments: {
      upload: () => ({result: Promise.resolve({ok: true, data: {id: 22, filename: 'screen.png'}}), abort() {}}),
      markdown: item => `![${item.filename}](/api/attachments/${item.id})`,
    },
  }, 'ENG');
  c.state.phase = 'ready';
  c.state.description = 'BeforeXafter';
  await c.upload([{name: 'screen.png'}], {start: 6, end: 7});
  const snippet = '![screen.png](/api/attachments/22)';
  assert.equal(c.state.description, `Before\n${snippet}\nafter`);
  assert.equal(c.state.caret, 6 + 1 + snippet.length);
});

test('inline label creation posts its color, adds and selects the label, and sorts choices', async () => {
  let request;
  const c = controller({
    api: {request: async (path, options) => {request = {path, options}; return {ok: true, data: {id: 3, name: 'launch', color: '#123456'}};}},
    navigate() {},
  }, 'ENG');
  c.state.phase = 'ready';
  c.state.project = {id: 9, identifier: 'ENG'};
  c.state.labelOptions = [{id: 2, name: 'bug', color: '#ff0000'}];
  assert.equal(await c.createLabel(' launch ', '#123456'), true);
  assert.equal(request.path, '/labels');
  assert.deepEqual(JSON.parse(request.options.body), {project_id: 9, name: 'launch', color: '#123456'});
  assert.equal(JSON.stringify(c.state.labels), '["launch"]');
  assert.equal(JSON.stringify(c.state.labelOptions.map(label => label.name)), '["bug","launch"]');
});

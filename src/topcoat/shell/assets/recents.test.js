const {test} = require('node:test');
const assert = require('node:assert/strict');
const fs = require('node:fs');
const vm = require('node:vm');

const context = {globalThis: {}, URLSearchParams};
vm.runInNewContext(fs.readFileSync(`${__dirname}/recents.js`, 'utf8'), context);
const {createClient} = context.globalThis.LificTopcoatRecents;
const project = {id: 7, identifier: 'LIF'};
const ok = data => ({ok: true, data});
const issue = (id, title = `Issue ${id}`) => ({id, project_id: 7, identifier: `LIF-${id}`, title});
const resource = (id, updated_at, overrides = {}) => ({id, project_id: 7,
  updated_at, title: `Resource ${id}`, name: `Module ${id}`, status: 'active', ...overrides});
const tick = () => new Promise(resolve => setImmediate(resolve));

function element(doc = null, tagName = 'div') {
  const listeners = new Map(), attributes = new Map();
  return {
    tagName,
    hidden: false, textContent: '', dataset: {}, children: [],
    setAttribute(key, value) {attributes.set(key, String(value));},
    getAttribute(key) {return attributes.get(key);},
    removeAttribute(key) {attributes.delete(key);},
    addEventListener(key, callback) {listeners.set(key, callback);},
    removeEventListener(key) {listeners.delete(key);},
    emit(key, event = {}) {listeners.get(key)?.(event);},
    append(...children) {this.children.push(...children);},
    replaceChildren(...children) {this.children = children;},
    querySelectorAll() {return this.children.map(row => row.children[0]);},
    contains(target) {return this === target || this.children.some(child => child.contains?.(target));},
    closest(selector) {return selector === 'a' && this.tagName === 'a' ? this : null;},
    focus() {if (doc) doc.activeElement = this;},
  };
}

function domFixture(request, options = {}) {
  const doc = {activeElement: null};
  const root = element(doc), toggle = element(doc), content = element(doc), status = element(doc), list = element(doc), error = element(doc);
  const nodes = {'[data-recents-toggle]': toggle, '[data-recents-content]': content,
    '[data-recents-status]': status, '[data-recents-list]': list, '[data-recents-error]': error};
  root.querySelector = selector => nodes[selector];
  const win = element();
  win.location = {pathname: '/LIF/issues/LIF-1', search: '', hash: ''};
  const storage = options.storage ?? new Map();
  win.sessionStorage = {
    getItem(key) {if (options.noStorage) throw Error('Unavailable'); return storage.get(key) ?? null;},
    setItem(key, value) {if (options.noStorage) throw Error('Unavailable'); storage.set(key, value);},
  };
  root.ownerDocument = {defaultView: win, activeElement: null,
    createElement: tagName => element(root.ownerDocument, tagName)};
  Object.defineProperty(root.ownerDocument, 'activeElement', {
    get() {return doc.activeElement;}, set(value) {doc.activeElement = value;},
  });
  const session = {request, state: {user: {id: 1}, publicProject: options.public ? 'LIF' : null}};
  const attached = context.globalThis.LificTopcoatRecents.attach(root, {session,
    catalog: {generation: 1, projects: [project, {id: 8, identifier: 'OTHER'}]}, win});
  return {root, toggle, content, status, list, error, win, session, storage, ...attached};
}

test('recent issues request the same five updated rows and retain identifier links', async () => {
  const paths = [];
  const client = createClient({request: async path => {paths.push(path); return ok(Array.from({length: 6}, (_, i) => issue(i + 1)));}});
  await client.activate({project, section: 'issues', public: false});
  assert.deepEqual(paths, ['/issues?project_id=7&order_by=updated&order=desc&limit=5']);
  assert.equal(client.peek().rows.length, 5);
  assert.equal(client.peek().rows[0].href, '/LIF/issues/LIF-1');
  assert.equal(client.peek().rows[0].label, 'Issue 1');
  assert.equal(client.peek().rows[0].identifier, 'LIF-1');
});

test('modules retain stable update ordering and limit the sidebar to five names', async () => {
  const paths = [];
  const client = createClient({request: async path => {
    paths.push(path);
    return ok([resource(1, '2025-01-01'), resource(3, '2025-03-01'),
      resource(2, '2025-03-01'), resource(4, '2025-02-01'),
      resource(5, '2025-01-05'), resource(6, '2025-01-04')]);
  }});
  await client.activate({project, section: 'modules'});
  assert.deepEqual(paths, ['/modules?project_id=7']);
  assert.deepEqual(Array.from(client.peek().rows, row => row.label), ['Module 3', 'Module 2', 'Module 4', 'Module 5', 'Module 6']);
  assert.equal(client.peek().rows[0].href, '/LIF/modules/3');
});

test('pages combine three bounded lifecycle queries by updated time then numeric id', async () => {
  const paths = [];
  const client = createClient({request: async path => {
    paths.push(path);
    const status = new URL(`https://lific.test${path}`).searchParams.get('status');
    return ok(status === 'draft' ? [resource(1, '2025-01-01', {status}), resource(7, '2025-03-01', {status})]
      : status === 'active' ? [resource(3, '2025-03-01', {status}), resource(4, '2025-02-01', {status})]
      : [resource(9, '2025-03-01', {status}), resource(6, '2025-01-04', {status})]);
  }});
  await client.activate({project, section: 'pages'});
  assert.deepEqual(paths, ['draft', 'active', 'complete'].map(status =>
    `/pages?project_id=7&status=${status}&order_by=updated&order=desc&limit=5`));
  assert.deepEqual(Array.from(client.peek().rows, row => row.href), ['/LIF/pages/9', '/LIF/pages/7', '/LIF/pages/3', '/LIF/pages/4', '/LIF/pages/6']);
});

test('plans preserve server order and over-fetch ten candidates before excluding archived', async () => {
  const paths = [];
  const client = createClient({request: async path => {
    paths.push(path);
    return ok([resource(8, '2025-01-01', {status: 'archived'}),
      ...[2, 4, 3, 1, 7, 6].map(id => resource(id, '2025-01-01'))]);
  }});
  await client.activate({project, section: 'plans'});
  assert.deepEqual(paths, ['/plans?project_id=7&limit=10']);
  assert.deepEqual(Array.from(client.peek().rows, row => row.href), ['/LIF/plans/2', '/LIF/plans/4', '/LIF/plans/3', '/LIF/plans/1', '/LIF/plans/7']);
});

test('same-project refresh keeps prior rows and disclosure, including return from another section', async () => {
  let reply;
  let first = true;
  const client = createClient({request: async () => {
    if (first) {first = false; return ok([issue(1)]);}
    return new Promise(resolve => {reply = resolve;});
  }});
  await client.activate({project, section: 'issues'});
  client.setOpen(true);
  await client.activate({project, section: null});
  assert.equal(client.peek().visible, false);
  const refreshing = client.activate({project, section: 'issues'});
  assert.equal(client.peek().rows[0]?.label, 'Issue 1');
  assert.equal(client.peek().open, true);
  assert.equal(client.peek().loading, true);
  reply(ok([issue(2)]));
  await refreshing;
  assert.equal(client.peek().rows[0].label, 'Issue 2');
  assert.equal(client.peek().loading, false);
});

test('activation captures project identity before awaiting responses', async () => {
  let reply;
  const mutable = {...project};
  const client = createClient({request: () => new Promise(resolve => {reply = resolve;})});
  const loading = client.activate({project: mutable, section: 'issues'});
  mutable.id = 42;
  mutable.identifier = 'OTHER';
  reply(ok([issue(1)]));
  await loading;
  assert.equal(client.peek().rows[0]?.href, '/LIF/issues/LIF-1');
  assert.equal(client.peek().project.identifier, 'LIF');
});

test('switching projects clears immediately and older detail-route replies cannot overwrite rows', async () => {
  const replies = [];
  const client = createClient({request: () => new Promise(resolve => replies.push(resolve))});
  const first = client.activate({project, section: 'issues'});
  replies[0](ok([issue(1)]));
  await first;
  const oldDetail = client.activate({project, section: 'issues'});
  const currentDetail = client.activate({project, section: 'issues'});
  replies[2](ok([issue(3)]));
  await currentDetail;
  replies[1](ok([issue(2)]));
  await oldDetail;
  assert.equal(client.peek().rows[0].label, 'Issue 3');
  const oldProject = client.activate({project, section: 'issues'});
  const other = client.activate({project: {id: 8, identifier: 'OTHER'}, section: 'issues'});
  assert.equal(client.peek().rows.length, 0);
  replies[3](ok([issue(4)]));
  await oldProject;
  assert.equal(client.peek().rows.length, 0);
  replies[4](ok([{id: 9, project_id: 8, identifier: 'OTHER-9', title: 'Other issue'}]));
  await other;
  assert.equal(client.peek().rows[0].href, '/OTHER/issues/OTHER-9');
});

test('public scope and account invalidation drop private links and reject pending replies', async () => {
  let count = 0;
  let reply;
  const client = createClient({request: () => {
    count++;
    return count === 1 ? Promise.resolve(ok([issue(1)])) : new Promise(resolve => {reply = resolve;});
  }});
  await client.activate({project, section: 'issues'});
  const privateLoading = client.activate({project, section: 'issues'});
  await client.activate({project, section: 'issues', public: true});
  assert.equal(client.peek().visible, false);
  assert.equal(client.peek().rows.length, 0);
  assert.equal(count, 2);
  reply(ok([issue(2)]));
  await privateLoading;
  assert.equal(client.peek().rows.length, 0);
  const nextAccount = client.activate({project, section: 'issues'});
  client.invalidate();
  reply(ok([issue(3)]));
  await nextAccount;
  assert.equal(client.peek().visible, false);
  assert.equal(client.peek().rows.length, 0);
});

test('mounted recents disclose accessible links, announce refresh, preserve focused rows and clear on project switch', async () => {
  let count = 0, reply;
  const f = domFixture(() => {
    count++;
    return count === 1 ? Promise.resolve(ok([issue(1, 'Keep <this> literal')])) : new Promise(resolve => {reply = resolve;});
  });
  await tick();
  assert.equal(f.list.children.length, 1);
  assert.equal(f.toggle.getAttribute('aria-expanded'), 'false');
  const link = f.list.children[0].children[0];
  assert.equal(link.getAttribute('href'), '/LIF/issues/LIF-1');
  assert.equal(link.getAttribute('aria-current'), 'page');
  assert.equal(link.getAttribute('aria-label'), 'LIF-1: Keep <this> literal');
  link.focus();
  f.toggle.emit('click');
  assert.equal(f.content.hidden, false);
  assert.equal(f.toggle.getAttribute('aria-expanded'), 'true');
  const refreshing = f.refresh();
  assert.equal(f.content.getAttribute('aria-busy'), 'true');
  assert.equal(f.status.textContent, 'Loading recent issues…');
  assert.equal(f.list.children[0].children[0], link);
  reply(ok([issue(1, 'Keep <this> literal')]));
  await refreshing;
  assert.equal(f.content.getAttribute('aria-busy'), 'false');
  assert.equal(f.list.children[0].children[0], f.root.ownerDocument.activeElement);
  assert.equal(f.root.ownerDocument.activeElement.getAttribute('href'), '/LIF/issues/LIF-1');
  f.win.location.pathname = '/OTHER/issues';
  f.win.emit('popstate');
  assert.equal(f.list.children.length, 0);
  assert.equal(f.content.hidden, false);
  f.dispose();
  reply(ok([{id: 9, project_id: 8, identifier: 'OTHER-9', title: 'Other issue'}]));
  await tick();
  assert.equal(f.list.children.length, 0);
});

test('mounted recents return focus to the disclosure when refresh removes the focused row', async () => {
  let count = 0, reply;
  const f = domFixture(() => {
    count++;
    return count === 1 ? Promise.resolve(ok([issue(1)])) : new Promise(resolve => {reply = resolve;});
  });
  await tick();
  f.list.children[0].children[0].focus();
  const refreshing = f.refresh();
  reply(ok([issue(2)]));
  await refreshing;
  assert.equal(f.root.ownerDocument.activeElement, f.toggle);
  f.dispose();
});

test('mounted public and anonymous scopes emit neither private requests nor private links', async () => {
  let count = 0;
  const f = domFixture(async () => {count++; return ok([issue(1)]);}, {public: true});
  await tick();
  assert.equal(count, 0);
  assert.equal(f.root.hidden, true);
  assert.equal(f.list.children.length, 0);
  f.session.state.publicProject = null;
  f.session.state.user = null;
  f.win.emit('lific:scope-change');
  await tick();
  assert.equal(count, 0);
  assert.equal(f.root.hidden, true);
  f.dispose();
});

test('disclosure survives native document navigation without storing resource rows', async () => {
  const storage = new Map();
  const f = domFixture(async () => ok([issue(1)]), {storage});
  await tick();
  f.toggle.emit('click');
  f.dispose();
  const next = domFixture(async () => ok([issue(2)]), {storage});
  await tick();
  assert.equal(next.toggle.getAttribute('aria-expanded'), 'true');
  assert.equal(next.content.hidden, false);
  assert.deepEqual([...storage], [['lific:sidebar:recents-open', '1']]);
  next.dispose();
  const blocked = domFixture(async () => ok([issue(1)]), {noStorage: true});
  await tick();
  blocked.toggle.emit('click');
  assert.equal(blocked.content.hidden, false);
  blocked.dispose();
});

test('account changes discard the previous catalog and accept the next account generation', async () => {
  let requests = 0;
  const f = domFixture(async () => {requests++; return ok([issue(1)]);});
  await tick();
  f.win.emit('lific:project-catalog', {detail: {generation: 10, projects: [project]}});
  await tick();
  assert.equal(requests, 1);
  f.session.state.user = {id: 2};
  f.win.emit('lific:account-change');
  assert.equal(f.root.hidden, true);
  assert.equal(f.list.children.length, 0);
  assert.equal(requests, 1);
  f.win.emit('lific:project-catalog', {detail: {generation: 1, projects: [project]}});
  await tick();
  assert.equal(requests, 2);
  assert.equal(f.root.hidden, false);
  f.dispose();
});

test('transient refresh errors keep successful rows while revoked access removes them', async () => {
  for (const status of [503, 403]) {
    let first = true;
    const client = createClient({request: async () => {
      if (first) {first = false; return ok([issue(1)]);}
      return {ok: false, status, error: 'Could not load'};
    }});
    await client.activate({project, section: 'issues'});
    await client.activate({project, section: 'issues'});
    assert.equal(client.peek().loading, false);
    assert.equal(client.peek().error, 'Could not load');
    assert.equal(client.peek().rows.length, status === 503 ? 1 : 0);
  }
});

test('a partial page lifecycle failure preserves the last complete combined result', async () => {
  let fail = false;
  const client = createClient({request: async path => {
    const status = new URL(`https://lific.test${path}`).searchParams.get('status');
    if (fail && status === 'active') return {ok: false, status: 503, error: 'Offline'};
    return ok([resource({draft: 1, active: 2, complete: 3}[status], '2025-01-01', {status})]);
  }});
  await client.activate({project, section: 'pages'});
  const previous = client.peek().rows;
  fail = true;
  await client.activate({project, section: 'pages'});
  assert.equal(client.peek().rows, previous);
  assert.equal(client.peek().error, 'Offline');
});

test('a page lifecycle access failure takes precedence over a transient sibling failure', async () => {
  let fail = false;
  const client = createClient({request: async path => {
    const status = new URL(`https://lific.test${path}`).searchParams.get('status');
    if (!fail) return ok([resource({draft: 1, active: 2, complete: 3}[status], '2025-01-01', {status})]);
    if (status === 'draft') return {ok: false, status: 503, error: 'Offline'};
    if (status === 'active') return {ok: false, status: 403, error: 'Forbidden'};
    return ok([resource(3, '2025-01-01', {status})]);
  }});
  await client.activate({project, section: 'pages'});
  assert.equal(client.peek().rows.length, 3);
  fail = true;
  await client.activate({project, section: 'pages'});
  assert.equal(client.peek().rows.length, 0);
  assert.equal(client.peek().error, 'Forbidden');
});

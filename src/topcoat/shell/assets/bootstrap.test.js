const {test} = require('node:test');
const assert = require('node:assert/strict');
const {mount, freezeCatalog, privateIdentity, createCatalogAdapter} = require('./bootstrap.js');

class Node {
  constructor(tag = 'div') {
    this.tagName = tag;
    this.children = [];
    this.dataset = {};
    this.attributes = {};
    this.hidden = false;
    this.parentElement = null;
  }
  append(...nodes) { for (const node of nodes) { node.parentElement = this; this.children.push(node); } }
  prepend(node) { node.parentElement = this; this.children.unshift(node); }
  replaceChildren(...nodes) { this.children = []; this.append(...nodes); }
  remove() {
    if (this.parentElement) this.parentElement.children = this.parentElement.children.filter(node => node !== this);
    this.parentElement = null;
  }
  setAttribute(name, value) { this.attributes[name] = String(value); }
  getAttribute(name) { return this.attributes[name] ?? null; }
  querySelector(selector) { return this.children.find(node => selector === '[data-topcoat-projects]'
    ? Object.hasOwn(node.dataset, 'topcoatProjects')
    : selector === '[data-topcoat-recents]' && Object.hasOwn(node.dataset, 'topcoatRecents')) ?? null; }
}

class EventWindow {
  constructor() {
    this.listeners = new Map(); this.document = null;
    this.location = {origin: 'https://lific.test', href: 'https://lific.test/LIF/issues', pathname: '/LIF/issues',
      assign: href => { this.assigned = href; }, replace: href => { this.replaced = href; }};
  }
  addEventListener(name, callback) {
    if (!this.listeners.has(name)) this.listeners.set(name, new Set());
    this.listeners.get(name).add(callback);
  }
  removeEventListener(name, callback) { this.listeners.get(name)?.delete(callback); }
  emit(name, detail = {}) {
    for (const callback of this.listeners.get(name) ?? []) callback({type: name, detail});
  }
}

function fixture(user = null, mobileGeneration = 0) {
  const win = new EventWindow();
  const desktop = new Node('nav');
  const shell = new Node('aside');
  shell.querySelector = selector => selector === '.tc-shell__desktop' ? desktop : null;
  const body = new Node('body');
  body.dataset.lificProjectId = '7';
  const mobilePanel = new Node('div');
  mobilePanel.dataset.mobileCatalog = JSON.stringify({generation: mobileGeneration, projects: [], groups: []});
  const doc = {
    body,
    createElement: tag => new Node(tag),
    querySelector: selector => selector === '.tc-shell' ? shell
      : selector === '[data-mobile-navigation]' ? mobilePanel : null,
  };
  win.document = doc;
  const requests = [];
  const session = {
    state: {user, publicProject: null},
    request: async (path, options = {}) => {
      requests.push({path, options});
      if (path === '/projects') return {ok: true, data: [{id: 7, identifier: 'LIF', name: 'Lific'}]};
      if (path === '/project-groups') return {ok: true, data: [{id: 3, name: 'Work', project_ids: [7], sort_order: 0}]};
      return {ok: true, data: {ok: true}};
    },
  };
  return {win, doc, desktop, session, requests};
}

test('catalog snapshots freeze their generation and rows', () => {
  const snapshot = freezeCatalog(4, [{id: 7}], [{id: 3, project_ids: [7]}]);
  assert.equal(Object.isFrozen(snapshot), true);
  assert.equal(Object.isFrozen(snapshot.projects), true);
  assert.equal(Object.isFrozen(snapshot.projects[0]), true);
  assert.equal(Object.isFrozen(snapshot.groups[0].project_ids), true);
});

test('catalog adapter loads both REST catalogs and maps every component command', async () => {
  const requests = [];
  let generation = 0;
  const session = {request: async (path, options = {}) => {
    requests.push({path, options});
    return path === '/projects'
      ? {ok: true, data: [{id: 7, identifier: 'LIF', name: 'Lific'}]}
      : path === '/project-groups'
        ? {ok: true, data: [{id: 3, name: 'Work', project_ids: [7], sort_order: 0}]}
        : {ok: true, data: {ok: true}};
  }};
  const adapter = createCatalogAdapter(session, () => ++generation, () => true);
  const snapshot = await adapter.fetch();
  assert.deepEqual(requests.slice(0, 2).map(request => request.path).sort(), ['/project-groups', '/projects']);
  assert.equal(snapshot.generation, 1);
  assert.equal(Object.isFrozen(snapshot.projects[0]), true);

  const commands = [
    [{type: 'reorder_projects', ids: [7]}, '/projects/reorder', 'PUT', {ids: [7]}],
    [{type: 'reorder_groups', ids: [3]}, '/project-groups/reorder', 'PUT', {ids: [3]}],
    [{type: 'assign_project', project_id: 7, group_id: 3}, '/project-groups/assign', 'PUT', {project_id: 7, group_id: 3}],
    [{type: 'delete_group', id: 3}, '/project-groups/3', 'DELETE', undefined],
    [{type: 'create_group', name: 'Later'}, '/project-groups', 'POST', {name: 'Later'}],
    [{type: 'rename_group', id: 3, name: 'Renamed'}, '/project-groups/3', 'PATCH', {name: 'Renamed'}],
  ];
  for (const [command, path, method, body] of commands) {
    const before = requests.length;
    const result = await adapter.command(command);
    const mutation = requests[before];
    assert.equal(mutation.path, path);
    assert.equal(mutation.options.method, method);
    assert.deepEqual(mutation.options.body === undefined ? undefined : JSON.parse(mutation.options.body), body);
    assert.equal(result.snapshot.generation, generation);
    assert.deepEqual(requests.slice(before + 1).map(request => request.path).sort(), ['/project-groups', '/projects']);
  }
  await assert.rejects(adapter.command({type: 'unknown'}), /Unsupported/);
});

test('private components mount only for an authenticated private session and clean up on scope change', () => {
  const f = fixture({id: 12, username: 'reader'});
  let projectOptions;
  let attachedRecents = 0;
  let destroyedProjects = 0;
  let disposedRecents = 0;
  const mobileCatalogs = [];
  f.win.lificMobileNavigation = {setCatalog: snapshot => mobileCatalogs.push(snapshot)};
  const projects = {attach(root, adapter) {
    projectOptions = {root, adapter};
    return {destroy() {destroyedProjects++;}};
  }};
  const recents = {attach(root, options) {
    attachedRecents++;
    assert.ok(root);
    assert.equal(root.getAttribute('aria-label'), 'Recent resources');
    assert.equal(root.hidden, true);
    assert.equal(root.dataset.projectIdentifier, 'LIF');
    assert.equal(root.dataset.projectId, '7');
    assert.equal(root.dataset.recentSection, 'issues');
    const [toggle, content] = root.children;
    assert.equal(toggle.getAttribute('aria-expanded'), 'false');
    assert.equal(toggle.getAttribute('aria-controls'), 'tc-sidebar-recents-list');
    assert.equal(content.id, 'tc-sidebar-recents-list');
    assert.equal(content.getAttribute('aria-busy'), 'false');
    const [status, list, error] = content.children;
    assert.equal(status.getAttribute('role'), 'status');
    assert.equal(status.getAttribute('aria-live'), 'polite');
    assert.equal(list.tagName, 'ul');
    assert.equal(error.getAttribute('role'), 'status');
    assert.equal(error.getAttribute('aria-live'), 'polite');
    assert.equal(error.hidden, true);
    assert.equal(options.session, f.session);
    return {dispose() {disposedRecents++;}};
  }};
  const app = mount({window: f.win, document: f.doc, session: f.session, projects, recents});
  assert.ok(app);
  assert.equal(attachedRecents, 1);
  assert.ok(projectOptions.root.parentElement);
  assert.equal(privateIdentity(f.session), 'private:12');

  f.session.state.publicProject = 'LIF';
  f.win.emit('lific:scope-change');
  assert.equal(destroyedProjects, 1);
  assert.equal(disposedRecents, 1);
  assert.equal(attachedRecents, 1);
  assert.equal(projectOptions.root.parentElement, null);
  assert.equal(f.desktop.querySelector('[data-topcoat-recents]'), null);
  assert.equal(mobileCatalogs.at(-1).projects.length, 0);

  f.session.state.publicProject = null;
  f.win.emit('lific:scope-change');
  assert.equal(attachedRecents, 2);
  app.dispose();
  assert.equal(destroyedProjects, 2);
  assert.equal(disposedRecents, 2);
  assert.equal(f.win.listeners.get('lific:account-change').size, 0);
});

test('anonymous and public sessions make no private UI or catalog requests', () => {
  for (const user of [null, {id: 1}]) {
    const f = fixture(user);
    if (user) f.session.state.publicProject = 'LIF';
    let attaches = 0;
    const app = mount({window: f.win, document: f.doc, session: f.session,
      projects: {attach() {attaches++;}}, recents: {attach() {attaches++;}}});
    assert.equal(attaches, 0);
    assert.equal(f.requests.length, 0);
    app.dispose();
  }
});

test('same-account session notifications preserve mounted components and pending state', () => {
  const f = fixture({id: 42});
  let projectAttaches = 0;
  let recentAttaches = 0;
  const projects = {attach() { projectAttaches++; return {destroy() {}}; }};
  const recents = {attach() { recentAttaches++; return {dispose() {}}; }};
  const app = mount({window: f.win, document: f.doc, session: f.session, projects, recents});
  const projectRoot = f.desktop.children.find(node => Object.hasOwn(node.dataset, 'topcoatProjects'));
  const recentsRoot = f.desktop.children.find(node => Object.hasOwn(node.dataset, 'topcoatRecents'));
  for (const event of ['lific:account-change', 'lific:session-change', 'lific:scope-change']) {
    f.win.emit(event);
  }
  assert.equal(projectAttaches, 1);
  assert.equal(recentAttaches, 1);
  assert.equal(f.desktop.children.includes(projectRoot), true);
  assert.equal(f.desktop.children.includes(recentsRoot), true);
  app.dispose();
});

test('an older account catalog response cannot be published after an account switch', async () => {
  const f = fixture({id: 1});
  let resolveProjects;
  const mobileCatalogs = [];
  f.win.lificMobileNavigation = {setCatalog: snapshot => mobileCatalogs.push(snapshot)};
  const projects = {attach(_root, adapter) {
    void adapter.fetch().then(snapshot => f.win.emit('lific:project-catalog', snapshot), () => {});
    return {destroy() {}};
  }};
  const recents = {attach() {return {dispose() {}};}};
  let first = true;
  f.session.request = path => {
    if (path === '/projects' && first) {
      first = false;
      return new Promise(resolve => {resolveProjects = resolve;});
    }
    return Promise.resolve(path === '/projects'
      ? {ok: true, data: [{id: 9, identifier: 'NEW', name: 'New'}]}
      : {ok: true, data: []});
  };
  const app = mount({window: f.win, document: f.doc, session: f.session, projects, recents});
  f.session.state.user = {id: 2};
  f.win.emit('lific:account-change');
  await new Promise(resolve => setImmediate(resolve));
  assert.equal(mobileCatalogs.at(-1).projects[0].identifier, 'NEW');
  resolveProjects({ok: true, data: [{id: 1, identifier: 'OLD', name: 'Old'}]});
  await new Promise(resolve => setImmediate(resolve));
  assert.equal(mobileCatalogs.at(-1).projects[0].identifier, 'NEW');
  app.dispose();
});

test('mobile route requests navigate same-origin and stay inside the active public project', () => {
  const f = fixture({id: 3});
  const app = mount({window: f.win, document: f.doc, session: f.session,
    projects: {attach() {return {destroy() {}};}}, recents: {attach() {return {dispose() {}};}}});
  f.win.emit('lific:navigate', {href: '/LIF/pages', history: 'push'});
  assert.equal(f.win.assigned, 'https://lific.test/LIF/pages');
  f.session.state.publicProject = 'LIF';
  f.win.location.pathname = '/public/LIF/issues';
  f.win.emit('lific:navigate', {href: '/public/LIF/pages', history: 'replace'});
  assert.equal(f.win.replaced, 'https://lific.test/public/LIF/pages');
  f.win.emit('lific:navigate', {href: '/public/OTHER/issues', history: 'push'});
  f.win.emit('lific:navigate', {href: 'https://evil.test/', history: 'push'});
  assert.equal(f.win.assigned, 'https://lific.test/LIF/pages');
  assert.equal(f.win.replaced, 'https://lific.test/public/LIF/pages');
  app.dispose();
});

test('active private project identifier comes from supported project routes', () => {
  for (const route of ['/abc_2/overview', '/abc_2/board', '/abc_2/settings']) {
    const f = fixture({id: 8});
    f.win.location.pathname = route;
    let options;
    const app = mount({window: f.win, document: f.doc, session: f.session,
      projects: {attach(_root, _adapter, value) {options = value; return {destroy() {}};}},
      recents: {attach() {return {dispose() {}};}}});
    assert.equal(options.activeIdentifier, 'ABC_2');
    app.dispose();
  }
});

test('an adapter command stops after its account changes and does not refresh the next account', async () => {
  let current = true;
  let finish;
  const requests = [];
  const adapter = createCatalogAdapter({request: (path) => {
    requests.push(path);
    return new Promise(resolve => {finish = resolve;});
  }}, () => 1, () => current);
  const command = adapter.command({type: 'delete_group', id: 4});
  current = false;
  finish({ok: true, data: {deleted: true}});
  await assert.rejects(command, /session changed/);
  assert.deepEqual(requests, ['/project-groups/4']);
  await assert.rejects(adapter.command({type: 'create_group', name: 'Private'}), /session changed/);
});

test('switching private accounts clears the old mobile catalog with a newer generation before mounting', () => {
  const f = fixture({id: 1}, 8);
  let mobileGeneration = 8;
  const mobileCatalogs = [];
  f.win.lificMobileNavigation = {setCatalog(snapshot) {
    if (snapshot.generation <= mobileGeneration) return false;
    mobileGeneration = snapshot.generation;
    mobileCatalogs.push(snapshot);
    return true;
  }};
  let attaches = 0;
  const projects = {attach() {
    attaches++;
    if (attaches === 2) assert.deepEqual(mobileCatalogs.at(-1).projects, []);
    return {destroy() {}};
  }};
  const app = mount({window: f.win, document: f.doc, session: f.session, projects,
    recents: {attach() {return {dispose() {}};}}});
  assert.equal(mobileCatalogs[0].generation, 9);
  f.win.emit('lific:project-catalog', {generation: 20,
    projects: [{id: 1, identifier: 'OLD', name: 'Old'}], groups: []});
  assert.equal(mobileCatalogs.at(-1).projects[0].identifier, 'OLD');
  f.session.state.user = {id: 2};
  f.win.emit('lific:account-change');
  assert.deepEqual(mobileCatalogs.at(-1).projects, []);
  assert.equal(mobileCatalogs.at(-1).generation, 21);
  app.dispose();
});

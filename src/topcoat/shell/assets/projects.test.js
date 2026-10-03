const {test} = require('node:test');
const assert = require('node:assert/strict');
const {CatalogController, attach, mergeProjectOrder, moveBy, moveBefore, moveProject, visibleProjects} = require('./projects.js');

const catalog = (generation, groups, projects) => ({generation, groups, projects});

class TestNode {
  constructor(tag, document) {
    this.tagName = tag;
    this.ownerDocument = document;
    this.children = [];
    this.dataset = {};
    this.attributes = {};
    this.hidden = false;
    this.value = '';
    this.parentElement = null;
    this._className = '';
    this._textContent = '';
  }
  set className(value) { this._className = value; }
  get className() { return this._className; }
  get classList() { return {contains: name => this._className.split(/\s+/).includes(name)}; }
  set textContent(value) { this._textContent = value; this.children = []; }
  get textContent() { return this._textContent + this.children.map(child => child.textContent).join(''); }
  append(...nodes) { for (const node of nodes) { node.parentElement = this; this.children.push(node); } }
  replaceChildren(...nodes) { this.children = []; this.append(...nodes); }
  setAttribute(name, value) { this.attributes[name] = String(value); }
  getAttribute(name) { return this.attributes[name] ?? null; }
  setSelectionRange(start, end, direction = 'none') {
    this.selectionStart = start; this.selectionEnd = end; this.selectionDirection = direction;
  }
  contains(target) { return this === target || this.children.some(child => child.contains(target)); }
  focus() { this.ownerDocument.activeElement = this; }
  addEventListener(name, listener) { (this.listeners ??= {})[name] = listener; }
  matches(selector) {
    if (selector === '[data-action="disclose"]') return this.dataset.action === 'disclose';
    if (selector === '[data-action="move-group"]') return this.dataset.action === 'move-group';
    if (selector === '[data-action="delete-group"]') return this.dataset.action === 'delete-group';
    if (selector === '[data-group-id]') return 'groupId' in this.dataset;
    if (selector === '[data-project-id]') return 'projectId' in this.dataset;
    if (selector === '.tc-projects__create') return this.className === 'tc-projects__create';
    if (selector === '[data-action="rename-group"]') return this.dataset.action === 'rename-group';
    return false;
  }
  closest(selector) {
    for (let node = this; node; node = node.parentElement) if (node.matches(selector)) return node;
    return null;
  }
  querySelectorAll(selector) {
    const found = [];
    const visit = node => { for (const child of node.children) {
      if (selector === 'input' && child.tagName === 'input'
        || selector === 'button, input' && ['button', 'input'].includes(child.tagName)
        || selector === 'button, input, a' && ['button', 'input', 'a'].includes(child.tagName)) found.push(child);
      visit(child);
    } };
    visit(this);
    return found;
  }
}

function testRoot(storage = {getItem: () => null, setItem() {}}) {
  const document = {createElement(tag) { return new TestNode(tag, document); }, activeElement: null,
    defaultView: {localStorage: storage}};
  const root = new TestNode('div', document);
  return root;
}

function findNode(root, predicate) {
  for (const child of root.children) {
    if (predicate(child)) return child;
    const nested = findNode(child, predicate);
    if (nested) return nested;
  }
  return null;
}

function submit(root, form) {
  const previous = global.FormData;
  global.FormData = class {
    constructor(target) { this.target = target; }
    get(name) {
      return findNode(this.target, node => node.name === name)?.value ?? null;
    }
  };
  let prevented = false;
  try { root.listeners.submit({target: form, preventDefault() { prevented = true; }}); }
  finally { global.FormData = previous; }
  return prevented;
}

test('project rows follow catalog project order within groups and ungrouped projects', () => {
  const snapshot = catalog(1,
    [{id: 9, name: 'Later', project_ids: [2, 1]}, {id: 4, name: 'First', project_ids: []}],
    [{id: 1, identifier: 'ONE', name: 'One'}, {id: 2, identifier: 'TWO', name: 'Two'},
      {id: 3, identifier: 'THREE', name: 'Three'}]);
  assert.deepEqual(visibleProjects(snapshot), [
    {groupId: 9, projectId: 1}, {groupId: 9, projectId: 2}, {groupId: null, projectId: 3},
  ]);
});

test('rendered group rows follow canonical catalog order', () => {
  const root = testRoot();
  attach(root, {current: () => catalog(1,
    [{id: 9, name: 'Group', project_ids: [2, 1]}],
    [{id: 1, identifier: 'ONE', name: 'One'}, {id: 2, identifier: 'TWO', name: 'Two'}])});
  const group = findNode(root, node => node.dataset.groupId === '9');
  assert.deepEqual(group.querySelectorAll('button, input, a').filter(node => node.tagName === 'a')
    .map(node => node.dataset.projectId), ['1', '2']);
});

test('group reorder keeps ungrouped projects in their canonical catalog positions', async () => {
  const controller = new CatalogController({command: async () => ({})});
  controller.snapshot = catalog(1,
    [{id: 8, name: 'A', project_ids: [3, 1]}],
    [{id: 1, name: 'One'}, {id: 2, name: 'Two'}, {id: 3, name: 'Three'}, {id: 4, name: 'Four'}]);
  const order = mergeProjectOrder(controller.snapshot, 8, [3, 1]);
  assert.deepEqual(order, [3, 2, 1, 4]);
  await controller.reorderProjects(order);
  assert.deepEqual(controller.snapshot.projects.map(project => project.id), [3, 2, 1, 4]);
});

test('a delayed older refresh cannot replace a newer catalog', async () => {
  const pending = [];
  const controller = new CatalogController({
    fetch: () => new Promise(resolve => pending.push(resolve)),
  });
  const older = controller.refresh();
  const newer = controller.refresh();
  pending[1](catalog(2, [], []));
  await newer;
  pending[0](catalog(1, [], []));
  await older;
  assert.equal(controller.snapshot.generation, 2);
});

test('a refresh started before an edit cannot replace the optimistic catalog', async () => {
  let finishRefresh;
  let finishCommand;
  const controller = new CatalogController({
    fetch: () => new Promise(resolve => { finishRefresh = resolve; }),
    command: () => new Promise(resolve => { finishCommand = resolve; }),
  });
  controller.snapshot = catalog(1, [], [{id: 1, name: 'One'}, {id: 2, name: 'Two'}]);
  const refresh = controller.refresh();
  const save = controller.reorderProjects([2, 1]);
  finishRefresh(catalog(2, [], [{id: 3, name: 'Stale'}, {id: 1, name: 'One'}, {id: 2, name: 'Two'}]));
  await refresh;
  assert.deepEqual(controller.snapshot.projects.map(project => project.id), [2, 1]);
  finishCommand({snapshot: catalog(3, [], [{id: 2, name: 'Two'}, {id: 1, name: 'One'}])});
  await save;
});

test('a failed mutation preserves a newer snapshot and still exposes the failure', async () => {
  let fail;
  const controller = new CatalogController({command: () => new Promise((_, reject) => { fail = reject; })});
  controller.snapshot = catalog(1, [], [{id: 1, name: 'One'}, {id: 2, name: 'Two'}]);
  const save = controller.reorderProjects([2, 1]);
  controller.accept(catalog(2, [], [{id: 1, name: 'Renamed'}, {id: 2, name: 'Two'}]));
  fail(new Error('save rejected'));
  await assert.rejects(save, /save rejected/);
  assert.deepEqual(controller.snapshot.projects.map(project => project.name), ['Renamed', 'Two']);
  assert.equal(controller.error, 'save rejected');
});

test('failed project reorder rolls back optimistic order and exposes an error', async () => {
  const controller = new CatalogController({command: async () => { throw new Error('offline'); }});
  controller.snapshot = catalog(1, [], [
    {id: 1, identifier: 'ONE', name: 'One'}, {id: 2, identifier: 'TWO', name: 'Two'},
  ]);
  await assert.rejects(controller.reorderProjects([2, 1]));
  assert.deepEqual(controller.snapshot.projects.map(project => project.id), [1, 2]);
  assert.equal(controller.error, 'offline');
});

test('project mutations do not overlap while a server write is pending', async () => {
  let finish;
  const controller = new CatalogController({command: () => new Promise(resolve => { finish = resolve; })});
  const first = controller.createGroup('First');
  await assert.rejects(controller.createGroup('Second'), /still saving/);
  finish({});
  await first;
  assert.equal(controller.pending, false);
});

test('deleting a group leaves its projects in the ungrouped catalog', async () => {
  const controller = new CatalogController({command: async () => ({})});
  controller.snapshot = catalog(1, [{id: 8, name: 'A', project_ids: [1]}], [
    {id: 1, identifier: 'ONE', name: 'One'},
  ]);
  await controller.deleteGroup(8);
  assert.deepEqual(controller.snapshot.groups, []);
  assert.deepEqual(visibleProjects(controller.snapshot), [{groupId: null, projectId: 1}]);
});

test('server snapshots with an old generation are ignored', () => {
  const controller = new CatalogController();
  assert.equal(controller.accept(catalog(3, [], [])), true);
  assert.equal(controller.accept(catalog(2, [], [])), false);
  assert.equal(controller.snapshot.generation, 3);
});

test('keyboard reorder can move a project between groups and preserves group order', () => {
  const snapshot = catalog(1,
    [{id: 8, name: 'A', project_ids: [1, 2]}, {id: 3, name: 'B', project_ids: []}],
    [{id: 1, identifier: 'ONE', name: 'One'}, {id: 2, identifier: 'TWO', name: 'Two'}]);
  assert.deepEqual(moveProject(snapshot, 2, 8, -1), {groupId: 8, beforeProjectId: 1});
  assert.deepEqual(moveBy([1, 2], 2, -1), [2, 1]);
  assert.deepEqual(moveBy([1, 2], 1, -1), [1, 2]);
});

test('drag reorder places projects before their target without dropping other rows', () => {
  assert.deepEqual(moveBefore([1, 2, 3], 3, 1), [3, 1, 2]);
  assert.deepEqual(moveBefore([1, 2, 3], 1, 3), [2, 1, 3]);
});

test('project reorder sends the frozen all-catalog command and keeps group assignment intact', async () => {
  let command;
  const controller = new CatalogController({command: async value => { command = value; return {}; }});
  controller.snapshot = catalog(1,
    [{id: 8, name: 'A', project_ids: [1, 2]}],
    [{id: 1, identifier: 'ONE', name: 'One'}, {id: 2, identifier: 'TWO', name: 'Two'},
      {id: 3, identifier: 'THREE', name: 'Three'}]);
  const order = mergeProjectOrder(controller.snapshot, 8, [2, 1]);
  await controller.reorderProjects(order);
  assert.deepEqual(command, {type: 'reorder_projects', ids: [2, 1, 3]});
  assert.deepEqual(controller.snapshot.groups[0].project_ids, [2, 1]);
});

test('public catalog omits groups and private projects', () => {
  const controller = new CatalogController({scope: 'public', fetch: async () => catalog(1, [], [])});
  assert.deepEqual(controller.normalize(catalog(1,
    [{id: 1, name: 'Private', project_ids: [2]}],
    [{id: 2, identifier: 'PRIV', name: 'Private project'}])), catalog(1, [], []));
});

test('disclosure state survives rerender when optional storage writes fail', () => {
  const storage = {getItem: () => null, setItem() { throw new Error('storage unavailable'); }};
  const root = testRoot(storage);
  const app = attach(root, {current: () => catalog(1,
    [{id: 1, name: 'Group', project_ids: [2]}], [{id: 2, identifier: 'TWO', name: 'Two'}])});
  const disclosure = findNode(root, node => node.dataset.action === 'disclose');
  disclosure.focus();
  root.listeners.click({target: disclosure});
  const rerendered = findNode(root, node => node.dataset.action === 'disclose');
  assert.equal(rerendered.getAttribute('aria-expanded'), 'false');
  assert.equal(root.ownerDocument.activeElement, rerendered);
  assert.equal(findNode(root, node => node.className === 'tc-projects__project-list').hidden, true);
  app.destroy();
});

test('save rerenders optimistic order, pending state, and failure status', async () => {
  let fail;
  const root = testRoot();
  const app = attach(root, {
    current: () => catalog(1,
      [{id: 1, name: 'First', project_ids: []}, {id: 2, name: 'Second', project_ids: []}], []),
    command: () => new Promise((_, reject) => { fail = reject; }),
  });
  const moveDown = findNode(root, node => node.dataset.action === 'move-group' && node.dataset.delta === '1');
  moveDown.focus();
  root.listeners.click({target: moveDown});
  assert.equal(findNode(root, node => node.className === 'tc-projects').getAttribute('aria-busy'), 'true');
  assert.equal(root.ownerDocument.activeElement.dataset.action, 'move-group');
  assert.deepEqual(root.children[0].children.at(-1).children.map(node => node.dataset.groupId), ['2', '1', '']);
  fail(new Error('save failed'));
  await new Promise(resolve => setImmediate(resolve));
  assert.equal(findNode(root, node => node.className === 'tc-projects').getAttribute('aria-busy'), 'false');
  const error = findNode(root, node => node.className === 'tc-projects__error');
  assert.equal(error.textContent, 'save failed');
  assert.equal(error.hidden, false);
  assert.equal(root.ownerDocument.activeElement.dataset.action, 'move-group');
  app.destroy();
});

test('submit is prevented during a pending save', () => {
  let finish;
  const root = testRoot();
  const app = attach(root, {
    current: () => catalog(1,
      [{id: 1, name: 'First', project_ids: []}, {id: 2, name: 'Second', project_ids: []}], []),
    command: () => new Promise(resolve => { finish = resolve; }),
  });
  const move = findNode(root, node => node.dataset.action === 'move-group' && node.dataset.delta === '1');
  root.listeners.click({target: move});
  assert.equal(app.controller.pending, true);
  const form = findNode(root, node => node.className === 'tc-projects__create');
  assert.equal(submit(root, form), true);
  finish({});
  return new Promise(resolve => setImmediate(resolve)).then(() => app.destroy());
});

test('untouched rename inputs accept catalog updates while edited drafts stay local', () => {
  let publish;
  const root = testRoot();
  const app = attach(root, {
    current: () => catalog(1, [{id: 1, name: 'Original', project_ids: []}], []),
    subscribe: callback => { publish = callback; return () => {}; },
  });
  let rename = findNode(root, node => node.name === 'name');
  publish(catalog(2, [{id: 1, name: 'Remote update', project_ids: []}], []));
  rename = findNode(root, node => node.name === 'name');
  assert.equal(rename.value, 'Remote update');
  rename.value = 'Local draft';
  rename.setSelectionRange(2, 7, 'backward');
  publish(catalog(3, [{id: 1, name: 'Another remote update', project_ids: []}], []));
  rename = findNode(root, node => node.name === 'name');
  assert.equal(rename.value, 'Local draft');
  assert.deepEqual([rename.selectionStart, rename.selectionEnd, rename.selectionDirection], [2, 7, 'backward']);
  app.destroy();
});

test('failed create and rename submissions preserve drafts and selection', async () => {
  let fail;
  const root = testRoot();
  const app = attach(root, {
    current: () => catalog(1, [{id: 1, name: 'Original', project_ids: []}], []),
    command: () => new Promise((_, reject) => { fail = reject; }),
  });
  const create = findNode(root, node => node.name === 'group-name');
  create.value = 'New group';
  create.setSelectionRange(2, 5, 'forward');
  create.focus();
  assert.equal(submit(root, findNode(root, node => node.className === 'tc-projects__create')), true);
  let input = findNode(root, node => node.name === 'group-name');
  assert.equal(input.value, 'New group');
  assert.deepEqual([input.selectionStart, input.selectionEnd, input.selectionDirection], [2, 5, 'forward']);
  fail(new Error('create failed'));
  await new Promise(resolve => setImmediate(resolve));
  input = findNode(root, node => node.name === 'group-name');
  assert.equal(input.value, 'New group');
  assert.deepEqual([input.selectionStart, input.selectionEnd], [2, 5]);

  const rename = findNode(root, node => node.name === 'name');
  rename.value = 'Draft name';
  rename.setSelectionRange(1, 4);
  assert.equal(submit(root, rename.closest('[data-action="rename-group"]')), true);
  fail(new Error('rename failed'));
  await new Promise(resolve => setImmediate(resolve));
  input = findNode(root, node => node.name === 'name');
  assert.equal(input.value, 'Draft name');
  assert.deepEqual([input.selectionStart, input.selectionEnd], [1, 4]);
  app.destroy();
});

test('successful create clears the draft after submission', async () => {
  const root = testRoot();
  const app = attach(root, {
    current: () => catalog(1, [], []),
    command: async () => ({}),
  });
  const input = findNode(root, node => node.name === 'group-name');
  input.value = 'Saved group';
  assert.equal(submit(root, findNode(root, node => node.className === 'tc-projects__create')), true);
  await new Promise(resolve => setImmediate(resolve));
  assert.equal(findNode(root, node => node.name === 'group-name').value, '');
  app.destroy();
});

test('a throwing localStorage property getter does not prevent rendering', () => {
  const root = testRoot();
  Object.defineProperty(root.ownerDocument.defaultView, 'localStorage', {
    get() { throw new Error('storage blocked'); },
  });
  assert.doesNotThrow(() => attach(root, {current: () => catalog(1, [], [])}));
  assert.equal(root.children.length, 1);
});

test('public scope renders no private tree and rejects mutations before the adapter', async () => {
  let called = false;
  const root = testRoot();
  const app = attach(root, {scope: 'public', command: async () => { called = true; }});
  assert.equal(root.children.length, 0);
  await assert.rejects(app.controller.deleteGroup(1), /unavailable/);
  assert.equal(called, false);
  app.destroy();
});

test('disconnect makes a pending command unable to publish its old snapshot', async () => {
  let finish;
  let dispatched = 0;
  class TestEvent { constructor(type, init) { this.type = type; this.detail = init.detail; } }
  const eventTarget = {CustomEvent: TestEvent, dispatchEvent() { dispatched += 1; }};
  const controller = new CatalogController({
    current: () => catalog(1, [], [{id: 1, name: 'One'}, {id: 2, name: 'Two'}]),
    command: () => new Promise(resolve => { finish = resolve; }),
    eventTarget,
  });
  const save = controller.reorderProjects([2, 1]);
  dispatched = 0;
  controller.disconnect();
  finish({snapshot: catalog(2, [], [{id: 1, name: 'Old account'}, {id: 2, name: 'Two'}])});
  await save;
  assert.equal(controller.snapshot.generation, 1);
  assert.equal(dispatched, 0);
  assert.equal(controller.pending, false);
});

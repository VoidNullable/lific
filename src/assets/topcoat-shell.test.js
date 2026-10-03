const {test} = require('node:test');
const assert = require('node:assert/strict');
const fs = require('node:fs');
const vm = require('node:vm');

function element() {
  const listeners = new Map();
  const attrs = new Map();
  return {
    dataset: {}, hidden: false, focused: false,
    style: {setProperty(name, value) { this[name] = value; }},
    addEventListener(name, handler) { listeners.set(name, handler); },
    setAttribute(name, value) { attrs.set(name, String(value)); },
    getAttribute(name) { return attrs.get(name); },
    focus() { this.focused = true; },
    setPointerCapture(id) { this.pointer = id; },
    hasPointerCapture(id) { return this.pointer === id; },
    releasePointerCapture() { this.pointer = null; },
    emit(name, values = {}) {
      const event = {button: 0, pointerId: 1, isPrimary: true, currentTarget: this,
        preventDefault() { this.defaultPrevented = true; }, ...values};
      listeners.get(name)?.(event);
      return event;
    },
  };
}

function fixture(initial = {}, options = {}) {
  const storage = new Map(Object.entries(initial));
  const shell = element(), sidebar = element(), toggle = element(), handle = element(), probe = element();
  const nodes = {'[data-sidebar-toggle]': toggle, '[data-sidebar-resize]': handle,
    '[data-sidebar-probe]': probe, '.tc-shell__desktop': sidebar};
  shell.querySelector = selector => nodes[selector];
  const body = {style: {cursor: 'auto', userSelect: 'text'}};
  let fontSize = options.fontSize ?? 16;
  let resize;
  const document = {body, documentElement: element(), querySelector: () => shell};
  const window = {addEventListener() {}};
  const localStorage = {
    getItem(key) { if (options.noStorage) throw Error('Unavailable'); return storage.get(key) ?? null; },
    setItem(key, value) { if (options.noStorage) throw Error('Unavailable'); storage.set(key, value); },
    removeItem(key) { if (options.noStorage) throw Error('Unavailable'); storage.delete(key); },
  };
  const location = {hash: '', href: 'https://lific.test/LIF/issues'};
  const context = {document, window, localStorage, location, URL,
    getComputedStyle: () => ({fontSize: String(fontSize)}),
    ResizeObserver: class {constructor(callback) {resize = callback;} observe() {}},
  };
  vm.runInNewContext(fs.readFileSync(`${__dirname}/topcoat-shell.js`, 'utf8'), context);
  return {shell, sidebar, toggle, handle, body, storage,
    setFont(size) { fontSize = size; resize?.([{contentRect: {width: size}}]); },
    hideProbe() { resize?.([{contentRect: {width: 0}}]); },
  };
}

test('sidebar restores existing width and collapse preferences across navigation', () => {
  const f = fixture({'lific:sidebar:width': '300', 'lific:sidebar:collapsed': '1'});
  assert.equal(f.shell.style['--tc-sidebar-width'], '300px');
  assert.equal(f.shell.dataset.sidebarCollapsed, 'true');
  assert.equal(f.toggle.getAttribute('aria-expanded'), 'false');
  f.toggle.emit('click');
  assert.equal(f.toggle.getAttribute('aria-label'), 'Collapse sidebar');
  assert.equal(f.storage.get('lific:sidebar:collapsed'), '0');
  const next = fixture(Object.fromEntries(f.storage));
  assert.equal(next.shell.dataset.sidebarCollapsed, 'false');
  assert.equal(next.shell.style['--tc-sidebar-width'], '300px');
});

test('sidebar resize preserves saved pixels while temporary font constraints apply', () => {
  const f = fixture({'lific:sidebar:width': '200'}, {fontSize: 32});
  assert.equal(f.handle.getAttribute('aria-valuenow'), '360');
  assert.equal(f.handle.getAttribute('aria-valuemin'), '360');
  f.hideProbe();
  assert.equal(f.handle.getAttribute('aria-valuenow'), '360');
  f.setFont(16);
  assert.equal(f.handle.getAttribute('aria-valuenow'), '200');
  assert.equal(f.storage.get('lific:sidebar:width'), '200');
  f.handle.emit('keydown', {key: 'ArrowRight'});
  assert.equal(f.handle.getAttribute('aria-valuenow'), '210');
  assert.equal(f.storage.get('lific:sidebar:width'), '210');
  f.handle.emit('dblclick');
  assert.equal(f.storage.has('lific:sidebar:width'), false);
  assert.equal(f.handle.getAttribute('aria-valuenow'), '230');
  f.setFont(32);
  assert.equal(f.handle.getAttribute('aria-valuenow'), '400');
});

test('pointer resize clamps bounds, persists on finish and restores body styles', () => {
  const f = fixture();
  f.handle.emit('pointerdown', {clientX: 230});
  assert.equal(f.body.style.cursor, 'col-resize');
  f.handle.emit('pointermove', {clientX: 900});
  assert.equal(f.handle.getAttribute('aria-valuenow'), '400');
  assert.equal(f.storage.has('lific:sidebar:width'), false);
  f.handle.emit('pointercancel');
  assert.equal(f.storage.get('lific:sidebar:width'), '400');
  assert.equal(f.body.style.cursor, 'auto');
  assert.equal(f.body.style.userSelect, 'text');
  assert.equal(f.handle.pointer, null);
  f.handle.emit('pointerdown', {clientX: 400});
  f.handle.emit('pointermove', {clientX: -900});
  f.handle.emit('pointerup');
  assert.equal(f.storage.get('lific:sidebar:width'), '180');
});

test('unavailable and malformed storage still leaves usable navigation and resizing', () => {
  for (const options of [{noStorage: true}, {}]) {
    const f = fixture({'lific:sidebar:width': 'NaN', 'lific:sidebar:collapsed': 'invalid'}, options);
    assert.equal(f.handle.getAttribute('aria-valuenow'), '230');
    assert.equal(f.toggle.getAttribute('aria-expanded'), 'true');
    f.toggle.emit('click');
    assert.equal(f.shell.dataset.sidebarCollapsed, 'true');
    f.handle.emit('keydown', {key: 'ArrowLeft'});
    assert.equal(f.handle.getAttribute('aria-valuenow'), '220');
  }
});

test('secondary pointers cannot replace an active resize or leave body styles stuck', () => {
  const f = fixture();
  f.handle.emit('pointerdown', {clientX: 230, isPrimary: true});
  f.handle.emit('pointerdown', {clientX: 900, pointerId: 2, isPrimary: false});
  f.handle.emit('pointerup');
  assert.equal(f.body.style.cursor, 'auto');
  assert.equal(f.body.style.userSelect, 'text');
  assert.equal(f.handle.pointer, null);
});

test('a second primary pointer cannot replace the active resize owner', () => {
  const f = fixture();
  f.handle.emit('pointerdown', {clientX: 230, pointerId: 1, isPrimary: true});
  f.handle.emit('pointerdown', {clientX: 900, pointerId: 2, isPrimary: true});
  f.handle.emit('pointerup', {pointerId: 1});
  assert.equal(f.body.style.cursor, 'auto');
  assert.equal(f.body.style.userSelect, 'text');
});

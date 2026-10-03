const {test} = require('node:test');
const assert = require('node:assert/strict');
const fs = require('node:fs');
const vm = require('node:vm');

const context = {URL};
vm.runInNewContext(
  fs.readFileSync(`${__dirname}/page-chrome.js`, 'utf8'),
  context,
);
const chrome = context.LificTopcoatPageChrome;

test('subtab storage uses project-scoped keys and ignores invalid stored ids', () => {
  const values = new Map([['lific:subtab:issues:7', 'all']]);
  const storage = {
    getItem(key) { return values.get(key) ?? null; },
    setItem(key, value) { values.set(key, value); },
  };
  assert.equal(chrome.subtabKey('issues', '7'), 'lific:subtab:issues:7');
  assert.equal(chrome.loadSubtab('issues', '7', ['all', 'active'], storage), 'all');
  assert.equal(chrome.loadSubtab('issues', '7', ['active'], storage), null);
  assert.equal(chrome.saveSubtab('issues', '7', 'active', storage), true);
  assert.equal(values.get('lific:subtab:issues:7'), 'active');
  assert.equal(chrome.loadSubtab('issues', '8', ['active'], storage), null);
});

test('storage failures leave the supplied default selection usable', () => {
  const storage = {
    getItem() { throw Error('blocked'); },
    setItem() { throw Error('blocked'); },
  };
  assert.equal(chrome.loadSubtab('pages', '7', ['active'], storage), null);
  assert.equal(chrome.saveSubtab('pages', '7', 'active', storage), false);
});

test('a blocked localStorage property also falls back without throwing', () => {
  const blocked = {};
  Object.defineProperty(blocked, 'localStorage', {
    get() { throw Error('storage access blocked'); },
  });
  const isolated = {};
  vm.runInNewContext(
    fs.readFileSync(`${__dirname}/page-chrome.js`, 'utf8'),
    Object.assign(isolated, {globalThis: blocked, URL}),
  );
  const api = blocked.LificTopcoatPageChrome;
  assert.equal(api.loadSubtab('plans', '7', ['active']), null);
  assert.equal(api.saveSubtab('plans', '7', 'active'), false);
});

test('public breadcrumb targets stay inside the selected project', () => {
  assert.equal(chrome.publicHrefIsScoped('/public/LIF/issues', 'LIF'), true);
  assert.equal(chrome.publicHrefIsScoped('#/public/LIF/issues?status=open', 'LIF'), true);
  assert.equal(chrome.publicHrefIsScoped('/LIF/issues', 'LIF'), false);
  assert.equal(chrome.publicHrefIsScoped('/public/LIF-extra/issues', 'LIF'), false);
});

test('Issues and Board route changes stay in one fade family', () => {
  assert.equal(chrome.routeFamily('/LIF/issues'), chrome.routeFamily('/LIF/board'));
  assert.equal(chrome.routeFamily('/public/LIF/issues'), chrome.routeFamily('/public/LIF/board'));
  assert.equal(chrome.shouldFade('/LIF/issues', '/LIF/board', false), false);
  assert.equal(chrome.shouldFade('/LIF/issues', '/LIF/pages', false), true);
  assert.equal(chrome.shouldFade('/LIF/issues', '/LIF/pages', true), false);
});

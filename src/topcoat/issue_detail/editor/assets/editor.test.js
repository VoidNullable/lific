const {test} = require('node:test');
const assert = require('node:assert/strict');
const editor = require('./editor.js');
const fs = require('node:fs');
const vm = require('node:vm');

const deferred = () => {let resolve, reject; const promise = new Promise((yes, no) => {resolve = yes; reject = no;}); return {promise, resolve, reject};};

test('serializes flushes and sends the newest draft after the active save completes', async () => {
  const first = deferred(), calls = [];
  const queue = editor.createSaveQueue({save: value => {calls.push(value); return calls.length === 1 ? first.promise : Promise.resolve({status: 'applied'});}});
  queue.edit('first');
  const flushing = queue.flush();
  queue.edit('latest');
  const secondFlush = queue.flush();
  await Promise.resolve();
  assert.deepEqual(calls, ['first']);
  first.resolve({status: 'applied'});
  await Promise.all([flushing, secondFlush]);
  assert.deepEqual(calls, ['first', 'latest']);
  assert.equal(queue.state().dirty, false);
});

test('debounce coalesces rapid edits into one save of the latest markdown', async () => {
  const calls = [];
  const queue = editor.createSaveQueue({debounceMs: 10, save: async value => {calls.push(value); return {status: 'applied'};}});
  queue.edit('a'); queue.edit('ab'); queue.edit('abc');
  await new Promise(resolve => setTimeout(resolve, 30));
  assert.deepEqual(calls, ['abc']);
  assert.equal(queue.state().dirty, false);
  queue.dispose();
});

test('conflict keeps the draft and expected sequence, and a later explicit flush retries it', async () => {
  const calls = [];
  const queue = editor.createSaveQueue({expectedSeq: 4, savedDescription: 'saved', save: async (text, seq) => {
    calls.push([text, seq]);
    return calls.length === 1 ? {status: 'conflict', currentDescription: 'server', expectedSeq: 8}
      : {status: 'applied', description: text, expectedSeq: 9};
  }});
  queue.edit('mine');
  await queue.flush();
  assert.deepEqual(queue.state(), {text: 'mine', savedDescription: 'server', dirty: true, expectedSeq: 8, conflict: true, error: ''});
  await queue.flush();
  assert.deepEqual(calls, [['mine', 4], ['mine', 8]]);
  assert.equal(queue.state().dirty, false);
});

test('a failed request preserves text and exposes an explicit retry state', async () => {
  let attempts = 0;
  const queue = editor.createSaveQueue({savedDescription: 'saved', save: async () => {
    attempts++;
    if (attempts === 1) throw new Error('Offline');
    return {status: 'applied', description: 'draft', expected_seq: 6};
  }});
  queue.edit('draft');
  await queue.flush();
  assert.equal(queue.state().text, 'draft');
  assert.equal(queue.state().dirty, true);
  assert.equal(queue.state().error, 'Offline');
  await queue.flush();
  assert.equal(queue.state().dirty, false);
  assert.equal(queue.state().expectedSeq, 6);
});

test('editing back to the old baseline during a save queues it after acknowledgement', async () => {
  const first = deferred(), calls = [];
  const queue = editor.createSaveQueue({text: 'saved', savedDescription: 'saved', expectedSeq: 3, debounceMs: 10,
    save: (text, seq) => {calls.push([text, seq]); return calls.length === 1 ? first.promise : Promise.resolve({status: 'applied', description: text, expected_seq: seq + 1});}});
  queue.edit('in flight');
  const flushing = queue.flush();
  queue.edit('saved');
  first.resolve({status: 'applied', description: 'in flight', expected_seq: 4});
  await flushing;
  await new Promise(resolve => setTimeout(resolve, 30));
  assert.deepEqual(calls, [['in flight', 3], ['saved', 4]]);
  assert.equal(queue.state().dirty, false);
  queue.dispose();
});

test('blocked saves retain the draft and resume debounce after attachment completion', async () => {
  const calls = [];
  const queue = editor.createSaveQueue({debounceMs: 10, save: async text => {calls.push(text); return {status: 'applied', description: text};}});
  queue.edit('with attachment');
  queue.setBlocked(true);
  await queue.flush();
  await new Promise(resolve => setTimeout(resolve, 20));
  assert.deepEqual(calls, []);
  queue.setBlocked(false);
  await new Promise(resolve => setTimeout(resolve, 30));
  assert.deepEqual(calls, ['with attachment']);
  assert.equal(queue.state().dirty, false);
  queue.dispose();
});

test('route conflict update preserves draft and blocks automatic saves', async () => {
  const calls = [];
  const queue = editor.createSaveQueue({text: 'draft', savedDescription: 'old', expectedSeq: 2, debounceMs: 10,
    save: async (...args) => {calls.push(args); return {status: 'applied'};}});
  queue.setConflict({current_description: 'server', expected_seq: 5});
  await new Promise(resolve => setTimeout(resolve, 20));
  assert.deepEqual(queue.state(), {text: 'draft', savedDescription: 'server', dirty: true, expectedSeq: 5, conflict: true, error: ''});
  assert.deepEqual(calls, []);
  queue.dispose();
});

test('matching the server description resolves a conflict without another save', async () => {
  const calls = [];
  const queue = editor.createSaveQueue({text: 'draft', savedDescription: 'old', expectedSeq: 2, debounceMs: -1,
    save: async (...args) => {calls.push(args); return {status: 'applied'};}});
  queue.setConflict({current_description: 'server', expected_seq: 5});
  queue.edit('still different');
  assert.equal(queue.state().conflict, true);
  queue.edit('server');
  assert.deepEqual(queue.state(), {text: 'server', savedDescription: 'server', dirty: false, expectedSeq: 5, conflict: false, error: ''});
  await queue.flush();
  assert.deepEqual(calls, []);
  queue.dispose();
});

test('a canonical update matching the draft resolves a conflict', () => {
  const queue = editor.createSaveQueue({text: 'draft', savedDescription: 'old', expectedSeq: 2, debounceMs: -1, save: async () => {}});
  queue.setConflict({current_description: 'server', expected_seq: 5});
  queue.setCanonical({savedDescription: 'draft', expectedSeq: 6});
  assert.equal(queue.state().dirty, false);
  assert.equal(queue.state().conflict, false);
  queue.dispose();
});

test('empty Markdown headings render and advance to the following paragraph', () => {
  const node = tag => ({tag, childNodes: [], append(...children) {this.childNodes.push(...children);}, replaceChildren() {this.childNodes = [];}});
  const preview = node('article');
  const context = vm.createContext({module: {exports: {}}, document: {createElement: node, createTextNode: text => ({text})}, preview});
  vm.runInContext(fs.readFileSync(`${__dirname}/editor.js`, 'utf8'), context);
  vm.runInContext('module.exports.renderMarkdown(preview, "# \\n## \\n###\\t\\n###### \\nafter")', context, {timeout: 1000});
  assert.deepEqual(preview.childNodes.map(child => child.tag), ['h1', 'h2', 'h3', 'h6', 'p']);
  assert.equal(preview.childNodes.slice(0, 4).every(child => child.childNodes.length === 0), true);
  assert.equal(preview.childNodes[4].childNodes[0].text, 'after');
});

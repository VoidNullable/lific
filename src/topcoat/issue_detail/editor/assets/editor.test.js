const {test} = require('node:test');
const assert = require('node:assert/strict');
const editor = require('./editor.js');

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

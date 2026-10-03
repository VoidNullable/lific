const {test} = require('node:test');
const assert = require('node:assert/strict');
const {STATUSES, PRIORITIES, labelNames} = require('./fields.js');

test('field choices preserve the issue API vocabulary and order', () => {
  assert.deepEqual(STATUSES, ['backlog','todo','active','done','cancelled']);
  assert.deepEqual(PRIORITIES, ['urgent','high','medium','low','none']);
});

test('labels trim whitespace, drop blanks and preserve first occurrence', () => {
  assert.deepEqual(labelNames('bug, needs review, bug, ,urgent'), ['bug','needs review','urgent']);
  assert.deepEqual(labelNames([' bug ','', 'docs', 'bug']), ['bug','docs']);
  assert.deepEqual(labelNames(null), []);
});

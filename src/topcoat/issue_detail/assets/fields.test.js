const {test} = require('node:test');
const assert = require('node:assert/strict');
const {STATUSES, PRIORITIES, labelNames} = require('./fields.js');

test('field choices preserve the issue API vocabulary and order', () => {
  assert.deepEqual(STATUSES, ['backlog','todo','active','done','cancelled']);
  assert.deepEqual(PRIORITIES, ['urgent','high','medium','low','none']);
});

test('labels preserve exact names instead of interpreting commas as delimiters', () => {
  assert.deepEqual(labelNames(['API, clients','urgent','API, clients']), ['API, clients','urgent']);
  assert.deepEqual(labelNames([' bug ','', 'docs', 'bug']), [' bug ','docs','bug']);
  assert.deepEqual(labelNames(null), []);
});

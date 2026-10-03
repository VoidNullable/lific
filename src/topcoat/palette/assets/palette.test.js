const {test} = require('node:test');
const assert = require('node:assert/strict');
const {parseRefQuery, searchDocuments, catalogResults, scopedResults, recentResults, snippetSegments} = require('./palette.js');

const projects = [{id: 7, identifier: 'LIF', name: 'Lific'}, {id: 8, identifier: 'OTHER', name: 'Other'}];

test('reference queries preserve issue/page intent and implicit project', () => {
  for (const query of ['34', '#34']) assert.deepEqual(parseRefQuery(query), {kind: 'issue', project: null, n: 34});
  for (const query of ['LIF34', 'lif 34', 'LIF-34']) assert.deepEqual(parseRefQuery(query), {kind: 'issue', project: 'LIF', n: 34});
  assert.deepEqual(parseRefQuery('doc 3'), {kind: 'page', project: null, n: 3});
  assert.deepEqual(parseRefQuery('lif doc 3'), {kind: 'page', project: 'LIF', n: 3});
  assert.equal(parseRefQuery('fix broken search'), null);
});

test('local document search matches all terms across fields, tolerates typos and keeps page slots', () => {
  const docs = Array.from({length: 12}, (_, id) => ({kind: 'issue', id, identifier: `LIF-${id}`, title: 'Search bug', labels: ['frontend'], preview: ''}));
  docs.push({kind: 'page', id: 20, identifier: 'LIF-DOC-1', title: 'Search design', labels: ['frontend'], preview: ''});
  const hits = searchDocuments('serch frontend', docs);
  assert.equal(hits.filter(hit => hit.doc.kind === 'issue').length, 8);
  assert.equal(hits.filter(hit => hit.doc.kind === 'page').length, 1);
  assert.deepEqual(searchDocuments('search unrelated', docs), []);
});

test('catalog search reaches projects modules plans folders and gives exact projects priority', () => {
  const catalog = {projects, modules: [{id: 1, project_id: 7, name: 'Lific UI'}], folders: [{id: 2, project_id: 7, name: 'Lific docs'}], plans: [{id: 3, project_id: 7, title: 'Lific migration', identifier: 'LIF-PLAN-3'}]};
  const hits = catalogResults('lific', catalog);
  assert.deepEqual(new Set(hits.map(hit => hit.kind)), new Set(['project', 'module', 'folder', 'plan']));
  assert.equal(hits[0].route, '/LIF/overview');
  assert.equal(hits.find(hit => hit.kind === 'plan').route, '/LIF/plans/3');
});

test('result scope rejects foreign origins, unknown projects and inaccessible routes', () => {
  const hits = ['/LIF/issues/LIF-1', '/OTHER/pages/2', '/UNKNOWN/issues/X-1', '//evil.test/LIF/issues/LIF-1', '/public/LIF/issues/LIF-1', '/settings', '/LIF/issues/new'].map(route => ({kind: 'issue', title: 'x', route}));
  assert.deepEqual(scopedResults(hits, projects).map(hit => hit.route), ['/LIF/issues/LIF-1', '/OTHER/pages/2']);
});

test('recents exclude current route and projects no longer authorized', () => {
  const entries = [{type: 'issue', routeId: 'LIF-1', project: 'LIF', title: 'One'}, {type: 'page', routeId: '2', project: 'OTHER', title: 'Two'}, {type: 'plan', routeId: '3', project: 'GONE', title: 'Gone'}];
  assert.deepEqual(recentResults(entries, projects, '/LIF/issues/LIF-1').map(hit => hit.route), ['/OTHER/pages/2']);
});

test('recents tolerate malformed browser storage rows without exposing invalid routes', () => {
  assert.deepEqual(recentResults([null, 7, {}, {type: 'issue', project: 'LIF', routeId: '//evil.test', title: 'Injected'}], projects, '/'), []);
});

test('FTS snippets highlight paired markers and preserve unmatched markers as literal text', () => {
  assert.deepEqual(snippetSegments('<b> **match** tail **'), [
    {text: '<b> ', highlighted: false}, {text: 'match', highlighted: true}, {text: ' tail **', highlighted: false},
  ]);
});

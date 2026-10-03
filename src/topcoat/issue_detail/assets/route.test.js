const {test} = require('node:test');
const assert = require('node:assert/strict');
const {scalarPatch, editable, routeMatches} = require('./route.js');

test('scalar form fields map to the existing issue update shape', () => {
  assert.deepEqual(scalarPatch({field:'title',value:'  Keep title  '}), {title:'Keep title'});
  assert.deepEqual(scalarPatch({field:'status',value:'active'}), {status:'active'});
  assert.deepEqual(scalarPatch({field:'priority',value:'high'}), {priority:'high'});
  assert.deepEqual(scalarPatch({field:'module_id',value:null}), {module_id:null});
  assert.deepEqual(scalarPatch({field:'labels',value:['bug']}), {labels:['bug']});
  assert.equal(scalarPatch({field:'start_date',value:'2026-10-03'}), null);
  assert.equal(scalarPatch({field:'other',value:'x'}), null);
});

test('only maintainers, leads and admins can edit scalar fields', () => {
  assert.equal(editable({role:'maintainer',enforced:true}), true);
  assert.equal(editable({role:'lead',enforced:true}), true);
  assert.equal(editable({role:'viewer',enforced:true,is_admin:true}), true);
  assert.equal(editable({role:'viewer',enforced:true,is_admin:false}), false);
  assert.equal(editable({role:null,enforced:false}), true);
  assert.equal(editable(null), false);
});

test('route generations reject results from an earlier activation of the same issue', () => {
  assert.equal(routeMatches({issue_id:4,generation:2},{issue_id:4,generation:2}), true);
  assert.equal(routeMatches({issue_id:4,generation:1},{issue_id:4,generation:2}), false);
  assert.equal(routeMatches({issue_id:4,generation:2},{issue_id:5,generation:2}), false);
  assert.equal(routeMatches(null,{issue_id:4,generation:2}), false);
});

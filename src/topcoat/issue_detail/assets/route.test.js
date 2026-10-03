const {test} = require('node:test');
const assert = require('node:assert/strict');
const {scalarPatch, editable, routeMatches, recordRecent} = require('./route.js');

test('scalar form fields map to the existing issue update shape', () => {
  assert.deepEqual(scalarPatch({field:'title',value:'  Keep title  '}), {title:'Keep title'});
  assert.deepEqual(scalarPatch({field:'status',value:'active'}), {status:'active'});
  assert.deepEqual(scalarPatch({field:'priority',value:'high'}), {priority:'high'});
  assert.deepEqual(scalarPatch({field:'module_id',value:null}), {module_id:null});
  assert.deepEqual(scalarPatch({field:'target_date',value:'2026-10-03'}), {target_date:'2026-10-03'});
  assert.deepEqual(scalarPatch({field:'target_date',value:''}), {target_date:null});
  assert.deepEqual(scalarPatch({field:'labels',value:['bug']}), {labels:['bug']});
  assert.equal(scalarPatch({field:'start_date',value:'2026-10-03'}), null);
  assert.equal(scalarPatch({field:'other',value:'x'}), null);
});

test('issue visits use the existing bounded and deduplicated dashboard recent format', () => {
  const values = new Map([['lific_recents',JSON.stringify([{type:'issue',routeId:'ENG-7',title:'Old'}])]]);
  global.localStorage = {getItem:key=>values.get(key) ?? null,setItem:(key,value)=>values.set(key,value)};
  recordRecent({identifier:'ENG-7',title:'Updated title'},'ENG');
  const [recent] = JSON.parse(values.get('lific_recents'));
  assert.deepEqual({type:recent.type,routeId:recent.routeId,identifier:recent.identifier,title:recent.title,project:recent.project},
    {type:'issue',routeId:'ENG-7',identifier:'ENG-7',title:'Updated title',project:'ENG'});
  assert.equal(typeof recent.ts,'number');
  delete global.localStorage;
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

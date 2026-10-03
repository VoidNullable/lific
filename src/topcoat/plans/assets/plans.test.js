const {test}=require('node:test');
const assert=require('node:assert/strict');
const {flattenSteps,movePatch,provenance,progress}=require('./plans.js');
const {identifierShape,findIssues}=require('./picker.js');

const tree=[{id:1,title:'Root',position:0,children:[{id:2,title:'Child',position:0,children:[{id:3,title:'Grandchild',children:[]}]}]},
  {id:4,title:'Another root',position:1,children:[]}];

test('step moves preserve API parent and position semantics and exclude descendants as parents',()=>{
  assert.deepEqual(flattenSteps(tree).map(row=>[row.step.id,row.depth]),[[1,0],[2,1],[3,2],[4,0]]);
  assert.deepEqual(movePatch(null,1),{move_to_root:true,move_position:1});
  assert.deepEqual(movePatch(4,0),{move_parent_step_id:4,move_position:0});
  assert.equal(flattenSteps(tree[0].children).some(row=>row.step.id===3),true);
});

test('completion and reopened provenance render server results without recalculating the tree',()=>{
  assert.deepEqual(progress({done_count:7,step_count:10,steps:tree}),{done:7,total:10,fraction:.7});
  assert.match(provenance({done:true,issue_identifier:'ENG-7',issue_status:'done'}),/via ENG-7/);
  assert.match(provenance({done:false,issue_identifier:'ENG-7',issue_status:'active',reopened_via_issue_at:'today'}),/reopened/);
});

test('issue picker resolves numeric IDs, merges project FTS hits, and restricts module assignment scope',async()=>{
  assert.equal(identifierShape('7','ENG'),'ENG-7');
  assert.equal(identifierShape('eng 007','ENG'),'ENG-7');
  assert.equal(identifierShape('OTHER-9','ENG'),'OTHER-9');
  const calls=[];
  const request=async path=>{
    calls.push(path);
    if(path.startsWith('/issues/resolve/'))return {id:9,project_id:4,identifier:'OTHER-9',title:'Elsewhere'};
    return [{result_type:'issue',id:7,project_id:3,identifier:'ENG-7',title:'Engine'},
      {result_type:'issue',id:8,project_id:4,identifier:'OTHER-8',title:'Foreign FTS result'},
      {result_type:'page',id:10,project_id:3,identifier:'ENG-DOC-10',title:'Page'}];
  };
  const project={id:3,identifier:'ENG'};
  assert.deepEqual((await findIssues(request,'OTHER-9',project)).map(hit=>hit.identifier),['OTHER-9','ENG-7']);
  assert.deepEqual((await findIssues(request,'OTHER-9',project,{projectOnly:true})).map(hit=>hit.identifier),['ENG-7']);
  assert.ok(calls.some(path=>path.includes('project_id=3')));
});

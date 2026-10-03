const {test}=require('node:test');
const assert=require('node:assert/strict');
const model=require('./model.js');
const issue=(id,status='todo')=>({id,identifier:`ENG-${id}`,title:`Issue ${id}`,status});

test('activity filters keep system distinct from everyone and scope dates, actor, query, and project together',()=>{
  const rows=[{id:1,project_id:3,actor_user_id:null,ts:'2026-10-01 10:00:00',entity_label:'ENG-1',new_value:'Build engine'},
    {id:2,project_id:3,actor_user_id:7,ts:'2026-10-02 10:00:00',entity_label:'ENG-2',new_value:'Build engine'},
    {id:3,project_id:4,actor_user_id:7,ts:'2026-10-02 10:00:00',new_value:'Build engine'}];
  assert.deepEqual(model.filterActivity(rows,{projectId:3,actor:'system',query:'ENGINE',start:'2026-10-01',end:'2026-10-02'}).map(row=>row.id),[1]);
  assert.deepEqual(model.filterActivity(rows,{projectId:3,actor:'7',query:'engine',start:'2026-10-02',end:'2026-10-02'}).map(row=>row.id),[2]);
  assert.equal(model.filterActivity(rows,{projectId:3,actor:'all'}).length,2);
});

test('activity entity links preserve issue, page, module, and comment destinations',()=>{
  assert.equal(model.activityHref('ENG',{entity_type:'issue',entity_label:'ENG-7'}),'/ENG/issues/ENG-7');
  assert.equal(model.activityHref('ENG',{entity_type:'comment',issue_id:7,page_id:null,entity_label:'ENG-7'}),'/ENG/issues/ENG-7');
  assert.equal(model.activityHref('ENG',{entity_type:'comment',issue_id:null,page_id:8}),'/ENG/pages/8');
  assert.equal(model.activityHref('ENG',{entity_type:'module',entity_id:9}),'/ENG/modules/9');
  assert.equal(model.activityHref('ENG',{entity_type:'folder',entity_id:9}),null);
});

test('insight series aligns weeks by date while keeping a missing observation separate from zero',()=>{
  assert.deepEqual(model.insightSeries({created_per_week:[],closed_per_week:[]}),[]);
  assert.deepEqual(model.insightSeries({created_per_week:[{week_start:'2026-09-28',count:0},{week_start:'2026-10-05',count:3}],closed_per_week:[{week_start:'2026-10-05',count:1}]}),[
    {week:'2026-09-28',created:0,closed:null},{week:'2026-10-05',created:3,closed:1}]);
});

test('graph partition computes linkage after closed visibility and drops foreign endpoints',()=>{
  const issues=[issue(1),issue(2,'done'),issue(3)],relations=[{source_id:1,target_id:2,relation_type:'blocks'},{source_id:3,target_id:99,relation_type:'blocks'}];
  assert.deepEqual(model.graphPartition(issues,relations,false).unlinked.map(row=>row.id),[1,3]);
  assert.deepEqual(model.graphPartition(issues,relations,true).linked.map(row=>row.id),[1,2]);
  assert.equal(model.graphPartition(issues,relations,true).relations.length,1);
});

test('graph layout preserves left to right blockers, disconnected clusters, and cycles without losing nodes',()=>{
  const opts={nodeWidth:200,nodeHeight:58,gapX:90,gapY:18,componentGap:48};
  const edges=[{source:1,target:2},{source:2,target:3},{source:3,target:1}];
  const layout=model.layoutGraph([1,2,3,4],edges,opts);
  assert.equal(layout.positions.size,4);assert.ok(layout.positions.get(1).x<layout.positions.get(2).x);
  for(const point of layout.positions.values()){assert.ok(Number.isFinite(point.x));assert.ok(Number.isFinite(point.y));}
  const clustered=model.layoutGraph([1,2],[],opts,[{source:1,target:2}]);
  assert.equal(clustered.positions.get(1).x,clustered.positions.get(2).x);
  assert.notEqual(clustered.positions.get(1).y,clustered.positions.get(2).y);
});

test('activity multiline diff folds unchanged context without discarding changed values',()=>{
  const before=Array.from({length:20},(_,index)=>`line ${index}`).join('\n');
  const after=before.replace('line 10','changed line');const rows=model.foldContext(model.diffLines(before,after));
  assert.ok(rows.some(row=>row.kind==='fold'));assert.ok(rows.some(row=>row.kind==='added'&&row.text==='changed line'));
});

const {test}=require('node:test');
const assert=require('node:assert/strict');
const {moduleProgress,visibleModules,issueState,metadataEditable,editable}=require('./modules.js');

test('module counts match the issue read model and count cancelled issues in the denominator',()=>{
  const rows=[{id:1,module_id:2,status:'done'},{id:2,module_id:2,status:'cancelled'},
    {id:3,module_id:3,status:'done'},{id:4,module_id:null,status:'active'}];
  assert.deepEqual(moduleProgress(rows,2),{done:1,total:2,fraction:.5});
});

test('module tabs retain lifecycle grouping and unknown statuses are visible in All',()=>{
  const rows=[{id:1,status:'active'},{id:2,status:'paused'},{id:3,status:'planned'},
    {id:4,status:'backlog'},{id:5,status:'done'},{id:6,status:'cancelled'},{id:7,status:'future'}];
  assert.deepEqual(visibleModules(rows,'active').map(row=>row.id),[1,2,3]);
  assert.deepEqual(visibleModules(rows,'archive').map(row=>row.id),[5,6]);
  assert.equal(visibleModules(rows,'all').length,7);
});

test('blocked and workable labels use authoritative filter membership',()=>{
  assert.equal(issueState(7,new Map([[7,{blocked_by:['ENG-8']}]]),new Set()),'Blocked by ENG-8');
  assert.equal(issueState(7,new Map([[7,{blocked_by:[]}]]),new Set()),'Blocked');
  assert.equal(issueState(8,new Map(),new Set([8])),'Workable');
  assert.equal(issueState(9,new Map(),new Set()),'');
});

test('module metadata and issue assignment follow separate enforced and legacy gates',()=>{
  for(const enforced of [true,false]){
    for(const role of ['lead','maintainer','viewer',null]){
      const access={role,enforced,is_admin:false};
      assert.equal(metadataEditable(access),role==='lead'||enforced&&role==='maintainer');
      assert.equal(editable(access),!enforced||role==='lead'||role==='maintainer');
    }
    assert.equal(metadataEditable({role:null,enforced,is_admin:true}),true);
  }
  assert.equal(metadataEditable({role:null,enforced:false,is_admin:false},{lead_user_id:1},{id:1}),true);
  assert.equal(metadataEditable(null),false);
});

const {test} = require('node:test');
const assert = require('node:assert/strict');
const {parseConfig, queryConfig, configQuery, visibleIssues, groups, boardLanes, Controller, createApi} = require('./issue-list.js');
const issue = (id, patch={}) => ({id,project_id:7,sequence:id,identifier:`ENG-${id}`,title:`Issue ${id}`,description:'',status:'todo',priority:'medium',module_id:null,labels:[],created_at:`2026-10-0${id}`,updated_at:`2026-10-0${id}`,sort_order:id,...patch});
const snapshot = issues => ({status:'ready',issues});
function setup(overrides={}) {
 const changes=[];
 const sync={peekProject:()=>snapshot([]),ensureProject:async()=>snapshot([]),refreshProject:async()=>{},setActiveProject(){},subscribe:()=>()=>{}};
 const api={projects:async()=>[{id:7,identifier:'ENG'}],modules:async()=>[{id:3,name:'Engine'}],labels:async()=>[{name:'bug'}],role:async()=>({enforced:true,role:'maintainer',is_admin:false}),views:async()=>[],...overrides};
 const store=new Map(),sessionStorage={getItem:key=>store.get(key)??null,setItem:(key,value)=>store.set(key,value)};
 const controller=new Controller({api,sync,sessionStorage,projectIdentifier:'ENG',onChange:state=>changes.push(state),...overrides});
 return {controller,changes,sync,api};
}
test('all persisted and saved view fields round trip through a deep link',()=>{
 const config=parseConfig(JSON.stringify({layout:'board',filterStatus:'@unresolved',filterPriority:'high',filterLabel:'needs review',filterModule:'Engine',searchQuery:'hello world',sortField:'updated',sortDir:'desc',groupBy:'module',density:'comfortable',laneBy:'priority',hiddenStatuses:['done']}));
 assert.deepEqual(queryConfig(configQuery(config),parseConfig('{}')),config);
});
test('foreign saved view fields fall back to existing view defaults',()=>{
 assert.equal(parseConfig('{'),null);
 assert.deepEqual(parseConfig('{"sortField":"bogus","laneBy":"bogus"}'),parseConfig('{}'));
});
test('unresolved label and module filters compose with priority sorting',()=>{
 const rows=[issue(1,{priority:'high',module_id:3,labels:['bug']}),issue(2,{priority:'urgent',module_id:3,labels:['bug']}),issue(3,{status:'done',module_id:3,labels:['bug']})];
 const config={...parseConfig('{}'),filterStatus:'@unresolved',filterLabel:'bug',filterModule:'Engine'};
 assert.deepEqual(visibleIssues(rows,config,[{id:3,name:'Engine'}]).map(x=>x.id),[2,1]);
});
test('fuzzy search matches subsequences and ranks title above preview',()=>{
 const rows=[issue(1,{title:'Unrelated',description:'engine repair'}),issue(2,{title:'Engine repair'}),issue(3,{title:'Elsewhere'})];
 assert.deepEqual(visibleIssues(rows,{...parseConfig('{}'),searchQuery:'engn'},[]).map(x=>x.id),[2,1]);
});
test('list grouping omits empty groups while board lanes preserve empty drop targets',()=>{
 const config={...parseConfig('{}'),groupBy:'module',laneBy:'module'};
 assert.deepEqual(groups([issue(1,{module_id:3})],config,[{id:3,name:'Engine'},{id:4,name:'Design'}]).map(x=>x.label),['Engine']);
 assert.deepEqual(boardLanes([issue(1)],config,[{id:3,name:'Engine'},{id:4,name:'Design'}]).map(x=>x.label),['Engine','Design','No module']);
});
test('cold loading publishes rows and live replica changes preserve selected surviving rows',async()=>{
 const {controller,sync}=setup();
 let rows=[issue(1),issue(2)]; sync.ensureProject=async()=>snapshot(rows);sync.peekProject=()=>snapshot(rows);
 await controller.load(); controller.select(1); controller.select(2);
 rows=[issue(1,{title:'Changed'})];controller.syncChanged();
 assert.equal(controller.state.rows[0].title,'Changed');assert.deepEqual([...controller.state.selected],[1]);
});
test('bulk updates retain failed selection and report every partial failure',async()=>{
 const {controller,sync}=setup({update:async(id,patch)=>{if(id===2)throw new Error('Permission denied');return issue(id,patch);}});
 sync.ensureProject=async()=>snapshot([issue(1),issue(2)]);
 await controller.load(); controller.select(1);controller.select(2);await controller.bulk({status:'active'});
 assert.equal(controller.state.rows.find(x=>x.id===1).status,'active');
 assert.deepEqual([...controller.state.selected],[2]);assert.match(controller.state.message,/1 updated/);assert.match(controller.state.error,/ENG-2: Permission denied/);
});
test('enforced viewer cannot select or mutate rows and public export is unavailable',async()=>{
 const calls=[];const {controller,sync}=setup({role:async()=>({enforced:true,role:'viewer',is_admin:false}),update:async()=>calls.push('write')});sync.ensureProject=async()=>snapshot([issue(1)]);
 await controller.load();controller.select(1);await controller.bulk({status:'active'});assert.equal(controller.state.selected.size,0);assert.deepEqual(calls,[]);
 const publicController=setup({publicScope:true}).controller;await publicController.load();assert.equal(publicController.state.canExport,false);
});
test('workspace selection applies project permissions per row',async()=>{
 const {controller,sync}=setup({projectIdentifier:null,projects:async()=>[{id:7,identifier:'ENG'},{id:8,identifier:'WEB'}],role:async id=>({enforced:true,role:id===7?'maintainer':'viewer',is_admin:false})});
 sync.ensureProject=async id=>snapshot([issue(id,{project_id:id})]);await controller.load();controller.select(7);controller.select(8);
 assert.deepEqual([...controller.state.selected],[7]);
});
test('old load completion cannot publish after disposal',async()=>{
 let resolve;const {controller}=setup({projects:()=>new Promise(done=>resolve=done)});
 const pending=controller.load();controller.dispose();resolve([{id:7,identifier:'ENG'}]);await pending;assert.equal(controller.state.rows.length,0);
});
test('bulk label union skips existing label and keeps other labels',async()=>{
 const writes=[];const {controller,sync}=setup({update:async(id,patch)=>{writes.push([id,patch]);return issue(id,patch);}});
 sync.ensureProject=async()=>snapshot([issue(1,{labels:['bug']}),issue(2,{labels:['design']})]);await controller.load();controller.select(1);controller.select(2);await controller.addLabel('bug');
 assert.deepEqual(writes,[[2,{labels:['design','bug']}] ]);
});
test('saved view APIs use existing ownership endpoints and canonical config strings',async()=>{
 const calls=[];const api=createApi({request:async(path,options)=>{calls.push([path,options]);return {ok:true,data:{id:2}};}});
 const config=parseConfig('{}');await api.saveView(7,'Triage',config,true);await api.updateView(7,2,{name:'New'});await api.deleteView(7,2);
 assert.equal(calls[0][0],'/projects/7/views');assert.equal(JSON.parse(calls[0][1].body).config,JSON.stringify(config));assert.equal(calls[1][1].method,'PATCH');assert.equal(calls[2][1].method,'DELETE');
});
test('board move writes status and lane assignment plus midpoint order',async()=>{
 const writes=[];const {controller,sync}=setup({update:async(id,patch)=>{writes.push(patch);return issue(id,patch);}});sync.ensureProject=async()=>snapshot([issue(1),issue(2,{sort_order:10}),issue(3,{sort_order:20})]);await controller.load();
 controller.change({...controller.state.config,laneBy:'module'});await controller.move(1,'active','3',2,3);
 assert.deepEqual(writes,[{status:'active',module_id:3,sort_order:15}]);
});
test('failed cold bootstrap reports an error without pretending there are no issues',async()=>{
 const {controller,sync}=setup();sync.ensureProject=async()=>{throw new Error('Offline');};await controller.load();assert.match(controller.state.error,/Offline/);assert.equal(controller.state.loading,false);
});
test('board without a status filter includes resolved columns in the state',()=>{
 const c=parseConfig('{}');assert.deepEqual(visibleIssues([issue(1,{status:'done'})],c,[]).map(i=>i.id),[1]);
});
test('existing raw layout lane and subtab persistence is readable',async()=>{
 const values=new Map([['lific:board:lanes:ENG','priority'],['lific:subtab:issues:7','closed']]);
 const {controller,sync}=setup({storage:{getItem:key=>values.get(key)??null,setItem:(key,value)=>values.set(key,value)}});sync.ensureProject=async()=>snapshot([issue(1,{status:'done'})]);await controller.load();assert.equal(controller.state.config.laneBy,'priority');assert.equal(controller.state.subTab,'closed');controller.change({...controller.state.config,layout:'board'});assert.equal(values.get('lific:list:layout:ENG'),'board');
});
test('explicit empty hidden-column state in a deep link overrides persisted hidden columns',()=>{
 const c=parseConfig('{}'),stored={...c,hiddenStatuses:['done']};assert.deepEqual(queryConfig(configQuery(c),stored).hiddenStatuses,[]);
});
test('deferred delete can undo without calling the server',async()=>{
 const calls=[];let timer;const {controller,sync}=setup({remove:async id=>calls.push(id),schedule:handler=>{timer=handler;return 1;},cancel:()=>{timer=null;}});sync.ensureProject=async()=>snapshot([issue(1)]);await controller.load();controller.select(1);await controller.scheduleDelete();assert.equal(controller.state.rows.length,0);controller.undoDelete();assert.equal(controller.state.rows.length,1);assert.deepEqual(calls,[]);assert.equal(timer,null);
});
test('deferred delete commits after route disposal instead of disappearing silently',async()=>{
 const calls=[];const {controller,sync}=setup({remove:async id=>calls.push(id),schedule:()=>1,cancel:()=>{}});sync.ensureProject=async()=>snapshot([issue(1)]);await controller.load();controller.select(1);await controller.scheduleDelete();const term=controller.generation;controller.dispose();await controller.finishDelete(term);assert.deepEqual(calls,[1]);
});
test('deferred delete never sends writes under a changed account',async()=>{
 const calls=[];let account='a';const {controller,sync}=setup({remove:async id=>calls.push(id),schedule:()=>1,identity:()=>account});sync.ensureProject=async()=>snapshot([issue(1)]);await controller.load();controller.select(1);await controller.scheduleDelete();account='b';await controller.finishDelete(controller.generation);assert.deepEqual(calls,[]);
});
test('canonical selected exports combine issue documents and reject failures and public scope',async()=>{
 const {exportSelected}=require('./issue-list.js'),calls=[];
 const env={identity:()=>1,headers:()=>({}),fetch:async path=>{calls.push(path);return new Response('Document');}};
 assert.equal(await (await exportSelected([issue(1),issue(2)],env)).text(),'Document\n\n---\n\nDocument');assert.deepEqual(calls,['/api/export/issues/ENG-1','/api/export/issues/ENG-2']);
 await assert.rejects(exportSelected([issue(1)],{...env,publicScope:true}),/public view/);
 await assert.rejects(exportSelected([issue(1)],{...env,fetch:async()=>new Response('Denied',{status:403})}),/ENG-1.*403/);
});
test('unavailable session storage does not repeatedly overwrite manual state with a default view',async()=>{
 const {controller,sync}=setup({sessionStorage:{getItem(){throw Error('blocked');},setItem(){throw Error('blocked');}},views:async()=>[{id:9,is_default:true,name:'Default',config:JSON.stringify({...parseConfig('{}'),filterStatus:'done'})}]});sync.ensureProject=async()=>snapshot([issue(1)]);await controller.load();assert.equal(controller.state.config.filterStatus,'');
});
test('a failed reload never leaves old project data mislabeled as a successful empty response',async()=>{
 let fail=false;const {controller,sync}=setup({projects:async()=>{if(fail)throw Error('Offline');return [{id:7,identifier:'ENG'}];}});sync.ensureProject=async()=>snapshot([issue(1)]);await controller.load();fail=true;await controller.load();assert.match(controller.state.error,/Offline/);assert.equal(controller.state.rows[0].id,1);
});
test('export stops at the existing 16 MiB limit and cancels its body reader',async()=>{
 const {exportSelected}=require('./issue-list.js');let cancelled=false;
 const body=new ReadableStream({start(stream){stream.enqueue(new Uint8Array(16*1024*1024+1));},cancel(){cancelled=true;}});
 await assert.rejects(exportSelected([issue(1)],{identity:()=>1,headers:()=>({}),fetch:async()=>({ok:true,body})}),/16 MiB/);assert.equal(cancelled,true);
});
test('an account change while reading export bytes cancels the document and prevents further exports',async()=>{
 const {exportSelected}=require('./issue-list.js');let account=1,cancelled=false,calls=0;
 const body=new ReadableStream({pull(stream){account=2;stream.enqueue(new Uint8Array([1]));},cancel(){cancelled=true;}},{highWaterMark:0});
 await assert.rejects(exportSelected([issue(1),issue(2)],{identity:()=>account,headers:()=>({}),fetch:async()=>{calls++;return {ok:true,body};}}),/Account changed/);
 assert.equal(cancelled,true);assert.equal(calls,1);
});
test('hidden rows cannot be selected through stale row handlers',async()=>{
 const {controller,sync}=setup();sync.ensureProject=async()=>snapshot([issue(1),issue(2,{status:'done'})]);await controller.load();controller.change({...controller.state.config,filterStatus:'todo'});controller.select(2);assert.equal(controller.state.selected.size,0);
});
test('undo restores each successful issue previous value rather than applying one shared value',async()=>{
 const writes=[];const {controller,sync}=setup({update:async(id,patch)=>{writes.push([id,patch]);return issue(id,patch);}});sync.ensureProject=async()=>snapshot([issue(1,{priority:'high'}),issue(2,{priority:'low'})]);await controller.load();controller.select(1);controller.select(2);await controller.bulk({priority:'urgent'});await controller.undoWrite();assert.deepEqual(writes.slice(2),[[1,{priority:'high'}],[2,{priority:'low'}]]);
});
test('retry during a deferred delete keeps deleted rows hidden and releases deletion bookkeeping',async()=>{
 const calls=[];const {controller,sync}=setup({remove:async id=>calls.push(id),schedule:()=>1,cancel(){}});sync.ensureProject=async()=>snapshot([issue(1),issue(2)]);await controller.load();controller.select(1);await controller.scheduleDelete();const term=controller.generation;await controller.load();assert.deepEqual(controller.state.rows.map(i=>i.id),[2]);await controller.finishDelete(term);assert.deepEqual(calls,[1]);assert.equal(controller.pendingDelete,null);assert.equal(controller.inFlight,0);assert.equal(controller.state.busy,false);
});
test('retry while a delete request is running cannot strand the operation counters',async()=>{
 let done;const {controller,sync}=setup({remove:()=>new Promise(resolve=>done=resolve),schedule:()=>1,cancel(){}});sync.ensureProject=async()=>snapshot([issue(1)]);await controller.load();controller.select(1);await controller.scheduleDelete();const pending=controller.finishDelete(controller.generation);while(!done)await new Promise(resolve=>setImmediate(resolve));await controller.load();done();await pending;assert.equal(controller.pendingDelete,null);assert.equal(controller.inFlight,0);assert.equal(controller.state.busy,false);
});
test('a stale saved-view fetch failure cannot overwrite a newer load',async()=>{
 let rejectOld,loaded=false;const {controller,sync}=setup({views:()=>loaded?Promise.resolve([]):new Promise((_,reject)=>{loaded=true;rejectOld=reject;})});sync.ensureProject=async()=>snapshot([issue(1)]);const old=controller.load();while(!rejectOld)await new Promise(resolve=>setImmediate(resolve));await controller.load();rejectOld(new Error('Old error'));await old;assert.equal(controller.state.error,'');
});
test('failed saved-view deletion keeps the active view and its session selection',async()=>{
 const values=new Map(),view={id:9,name:'Triage',config:JSON.stringify(parseConfig('{}'))};const {controller,sync}=setup({sessionStorage:{getItem:key=>values.get(key)??null,setItem:(key,value)=>values.set(key,value)},views:async()=>[view],deleteView:async()=>{throw new Error('Permission denied');}});sync.ensureProject=async()=>snapshot([issue(1)]);await controller.load();controller.applyView(9);await controller.deleteView();assert.equal(controller.state.activeView,9);assert.equal(values.get('lific:views:active:ENG'),'9');assert.match(controller.state.error,/Permission denied/);
});
test('a persisted deferred delete resumes in a fresh document under the same account',async()=>{
 const {resumeDeletions}=require('./issue-list.js'),values=new Map(),calls=[];let queued;
 const sessionStorage={getItem:key=>values.get(key)??null,setItem:(key,value)=>values.set(key,value)};
 const {controller,sync}=setup({sessionStorage,identity:()=> 'private:secret-session',remove:async id=>calls.push(id),schedule:()=>1,cancel(){}});sync.ensureProject=async()=>snapshot([issue(1)]);await controller.load();controller.select(1);await controller.scheduleDelete();
 assert.equal([...values.values()].some(value=>value.includes('secret-session')),false);
 await resumeDeletions({sessionStorage,identity:()=> 'private:secret-session',api:{remove:async id=>calls.push(id)},schedule:handler=>{queued=handler;return 1;}});await queued();assert.deepEqual(calls,[1]);
});
test('another account cannot resume a stored deferred deletion',async()=>{
 const {resumeDeletions}=require('./issue-list.js'),values=new Map(),calls=[];const sessionStorage={getItem:key=>values.get(key)??null,setItem:(key,value)=>values.set(key,value)};
 const {controller,sync}=setup({sessionStorage,identity:()=> 'account:a',remove:async id=>calls.push(id),schedule:()=>1,cancel(){}});sync.ensureProject=async()=>snapshot([issue(1)]);await controller.load();controller.select(1);await controller.scheduleDelete();
 await resumeDeletions({sessionStorage,identity:()=> 'account:b',api:{remove:async id=>calls.push(id)},schedule:()=>{throw new Error('Must not schedule another account deletion');}});assert.deepEqual(calls,[]);
});
test('a successful saved-view deletion clears its active selection even when refreshing views fails',async()=>{
 let deleted=false;const view={id:9,name:'Triage',config:JSON.stringify(parseConfig('{}'))};const {controller,sync}=setup({views:async()=>{if(deleted)throw new Error('Refresh unavailable');return [view];},deleteView:async()=>{deleted=true;return {deleted:true};}});sync.ensureProject=async()=>snapshot([issue(1)]);await controller.load();controller.applyView(9);await controller.deleteView();assert.equal(controller.state.activeView,null);assert.match(controller.state.error,/Refresh unavailable/);
});
test('unavailable deletion storage completes writes before announcing success',async()=>{
 const calls=[];const {controller,sync}=setup({sessionStorage:{getItem:()=>null,setItem(){throw Error('Storage blocked');}},remove:async id=>calls.push(id)});sync.ensureProject=async()=>snapshot([issue(1)]);await controller.load();controller.select(1);await controller.scheduleDelete();assert.deepEqual(calls,[1]);assert.equal(controller.pendingDelete,null);assert.equal(controller.state.busy,false);assert.doesNotMatch(controller.state.message,/Undo is available/);
});
test('partial delete failure restores failed rows and reports their identifiers after a retry',async()=>{
 const {controller,sync}=setup({remove:async id=>{if(id===2)throw Error('Permission denied');},schedule:()=>1,cancel(){}});sync.ensureProject=async()=>snapshot([issue(1),issue(2)]);await controller.load();controller.selectAll();await controller.scheduleDelete();await controller.load();await controller.finishDelete();assert.deepEqual(controller.state.rows.map(row=>row.id),[2]);assert.deepEqual([...controller.state.selected],[2]);assert.match(controller.state.error,/ENG-2: Permission denied/);assert.equal(controller.pendingDelete,null);assert.equal(controller.inFlight,0);assert.equal(controller.state.busy,false);
});
test('resumed Undo cannot cancel or remove a deletion record after its request starts',async()=>{
 const {resumeDeletions}=require('./issue-list.js'),values=new Map();let queued,undo,complete;const sessionStorage={getItem:key=>values.get(key)??null,setItem:(key,value)=>values.set(key,value)};
 const {controller,sync}=setup({sessionStorage,identity:()=> 'owner',schedule:()=>1,cancel(){}});sync.ensureProject=async()=>snapshot([issue(1)]);await controller.load();controller.select(1);await controller.scheduleDelete();
 await resumeDeletions({sessionStorage,identity:()=> 'owner',api:{remove:()=>new Promise(resolve=>complete=resolve)},schedule:handler=>{queued=handler;return 1;},cancel(){},onPending:(_,cancel)=>undo=cancel});
 const deleting=queued();while(!complete)await new Promise(resolve=>setImmediate(resolve));const cancelled=undo(),remaining=JSON.parse(sessionStorage.getItem('lific:issue-list:deferred-deletions')).length;complete();await deleting;
 assert.equal(cancelled,false);assert.equal(remaining,1);
});
test('a resumed deletion stays hidden across live replica updates until Undo removes its record',async()=>{
 const values=new Map(),sessionStorage={getItem:key=>values.get(key)??null,setItem:(key,value)=>values.set(key,value)};
 const original=setup({sessionStorage,identity:()=> 'owner',schedule:()=>1,cancel(){}});original.sync.ensureProject=async()=>snapshot([issue(1)]);await original.controller.load();original.controller.select(1);await original.controller.scheduleDelete();original.controller.dispose();
 const fresh=setup({sessionStorage,identity:()=> 'owner'});fresh.sync.ensureProject=async()=>snapshot([issue(1),issue(2)]);fresh.sync.peekProject=()=>snapshot([issue(1,{title:'Live update'}),issue(2)]);await fresh.controller.load();assert.deepEqual(fresh.controller.state.rows.map(row=>row.id),[2]);fresh.controller.syncChanged();assert.deepEqual(fresh.controller.state.rows.map(row=>row.id),[2]);
 sessionStorage.setItem('lific:issue-list:deferred-deletions','[]');fresh.controller.syncChanged();assert.deepEqual(fresh.controller.state.rows.map(row=>row.id),[1,2]);
});
test('failed recovered deletes restore mounted rows after the final replica refresh removes their mask',async()=>{
 const {resumeDeletions}=require('./issue-list.js'),values=new Map();let queued;const sessionStorage={getItem:key=>values.get(key)??null,setItem:(key,value)=>values.set(key,value)};
 const original=setup({sessionStorage,identity:()=> 'owner',schedule:()=>1,cancel(){}});original.sync.ensureProject=async()=>snapshot([issue(1)]);await original.controller.load();original.controller.select(1);await original.controller.scheduleDelete();original.controller.dispose();
 const mounted=setup({sessionStorage,identity:()=> 'owner'});mounted.sync.ensureProject=async()=>snapshot([issue(1)]);mounted.sync.peekProject=()=>snapshot([issue(1)]);mounted.sync.refreshProject=async()=>mounted.controller.syncChanged();await mounted.controller.load();assert.equal(mounted.controller.state.rows.length,0);
 await resumeDeletions({sessionStorage,identity:()=> 'owner',api:{remove:async()=>{throw Error('Permission denied');}},sync:mounted.sync,schedule:handler=>{queued=handler;return 1;}});await queued();assert.deepEqual(mounted.controller.state.rows.map(row=>row.id),[1]);mounted.controller.dispose();
});
test('a recovered completion never publishes failed issue identifiers after the account changes during replica refresh',async()=>{
 const {resumeDeletions}=require('./issue-list.js'),values=new Map(),completed=[];let queued,finishRefresh,account='owner';const sessionStorage={getItem:key=>values.get(key)??null,setItem:(key,value)=>values.set(key,value)};
 const original=setup({sessionStorage,identity:()=>account,schedule:()=>1,cancel(){}});original.sync.ensureProject=async()=>snapshot([issue(1)]);await original.controller.load();original.controller.select(1);await original.controller.scheduleDelete();original.controller.dispose();
 await resumeDeletions({sessionStorage,identity:()=>account,api:{remove:async()=>{throw Error('Secret issue failed');}},sync:{refreshProject:()=>new Promise(resolve=>finishRefresh=resolve)},schedule:handler=>{queued=handler;return 1;},onComplete:results=>completed.push(results)});
 const pending=queued();while(!finishRefresh)await new Promise(resolve=>setImmediate(resolve));account='new-owner';finishRefresh();await pending;assert.deepEqual(completed,[]);
});

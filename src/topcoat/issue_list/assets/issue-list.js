(() => {
 'use strict';
 const STATUSES=['backlog','todo','active','done','cancelled'];
 const PRIORITIES=['urgent','high','medium','low','none'];
 const DEFAULT={version:1,layout:'list',filterStatus:'',filterPriority:'',filterLabel:'',filterModule:'',searchQuery:'',sortField:'priority',sortDir:'asc',groupBy:'status',density:'compact',laneBy:'none',hiddenStatuses:[]};
 const QUERY={filterStatus:'status',filterPriority:'priority',filterLabel:'label',filterModule:'module',searchQuery:'q',sortField:'sort',sortDir:'order',groupBy:'group',density:'density',laneBy:'lanes',layout:'layout'};
 const member=(value,values,fallback)=>values.includes(value)?value:fallback;
 function parseConfig(raw) {
  try {
   const p=typeof raw==='string'?JSON.parse(raw):raw;
   if(!p||typeof p!=='object'||Array.isArray(p))return null;
   return {...DEFAULT,...Object.fromEntries(['filterStatus','filterPriority','filterLabel','filterModule','searchQuery'].map(k=>[k,typeof p[k]==='string'?p[k]:''])),
    layout:member(p.layout,['list','board'],'list'),sortField:member(p.sortField,['priority','age','number','updated'],'priority'),sortDir:member(p.sortDir,['asc','desc'],'asc'),
    groupBy:member(p.groupBy,['status','priority','module','none'],'status'),density:member(p.density,['compact','comfortable'],'compact'),laneBy:member(p.laneBy,['none','priority','module'],'none'),
    hiddenStatuses:Array.isArray(p.hiddenStatuses)?p.hiddenStatuses.filter(x=>typeof x==='string'):[]};
  }catch{return null;}
 }
 function queryConfig(search,base) {
  const params=new URLSearchParams(search),patch={...base};
  for(const [key,name]of Object.entries(QUERY))if(params.has(name))patch[key]=params.get(name);
  if(params.has('hidden'))patch.hiddenStatuses=params.getAll('hidden').filter(Boolean);
  return parseConfig(patch);
 }
 function configQuery(config,search='') {
  const params=new URLSearchParams(search);
  for(const [key,name]of Object.entries(QUERY))params.set(name,config[key]);
  params.delete('hidden');for(const value of config.hiddenStatuses.length?config.hiddenStatuses:[''])params.append('hidden',value);
  return params.toString();
 }
 // Same scorer and weights as the existing issue list, including its 50-row cap.
 const BOUNDARY=/[\s\-_/.,()[\]{}<>:;!?"'`]/;
 function fuzzy(query,text) {
  const q=query.toLowerCase(),t=(text||'').toLowerCase(),direct=t.indexOf(q);
  if(direct!==-1)return direct===0?.95:BOUNDARY.test(t[direct-1])?.9:.8;
  let qi=0,first=-1,last=-1,run=0,longest=0,words=0;
  for(let ti=0;qi<q.length&&ti<t.length;ti++)if(q[qi]===t[ti]) {
   if(first===-1)first=ti;
   if(ti===last+1)run++;else{run=1;if(BOUNDARY.test(ti>0?t[ti-1]:' '))words++;}
   longest=Math.max(longest,run);last=ti;qi++;
  }
  return qi<q.length?0:Math.min(.7,q.length/Math.max(1,last-first+1)*.4+longest/q.length*.4+words/q.length*.2);
 }
 function visibleIssues(rows,config,modules,subTab='all') {
  const module=modules.find(m=>m.name===config.filterModule)?.id??null,q=config.searchQuery.trim(),scores=new Map();
  let filtered=rows.filter(i=>(!config.filterStatus||(config.filterStatus==='@unresolved'?!['done','cancelled'].includes(i.status):i.status===config.filterStatus))
   &&(!config.filterPriority||i.priority===config.filterPriority)&&(!config.filterLabel||i.labels.includes(config.filterLabel))&&(!config.filterModule||i.module_id===module));
  if(q)filtered=filtered.filter(i=>{const score=Math.max(fuzzy(q,i.title),fuzzy(q,i.identifier)*.9,fuzzy(q,(i.description||i.preview||'').slice(0,4000))*.6);scores.set(i.id,score);return score>=.25;}).sort((a,b)=>scores.get(b.id)-scores.get(a.id)).slice(0,50);
  filtered.sort((a,b)=>{
   if(q)return scores.get(b.id)-scores.get(a.id)||a.identifier.localeCompare(b.identifier);
   const comparison=config.sortField==='priority'?(PRIORITIES.indexOf(a.priority)-PRIORITIES.indexOf(b.priority)||b.created_at.localeCompare(a.created_at))
    :config.sortField==='age'?a.created_at.localeCompare(b.created_at):config.sortField==='updated'?a.updated_at.localeCompare(b.updated_at):a.sequence-b.sequence;
   return config.sortDir==='asc'?comparison:-comparison;
  });
  if(config.layout==='list'&&subTab==='recent')return filtered.sort((a,b)=>b.updated_at.localeCompare(a.updated_at)).slice(0,20);
  return filtered.filter(i=>subTab==='open'?!['done','cancelled'].includes(i.status):subTab==='closed'?['done','cancelled'].includes(i.status):true);
 }
 function buckets(rows,by,modules,includeEmpty) {
  const choices=by==='module'?[...modules.map(m=>({key:String(m.id),label:m.name})),{key:'none',label:'No module'}]
   :(by==='priority'?PRIORITIES:STATUSES).map(key=>({key,label:key}));
  return choices.map(bucket=>({...bucket,issues:rows.filter(i=>by==='module'?(i.module_id===null?'none':String(i.module_id))===bucket.key:i[by]===bucket.key)})).filter(b=>includeEmpty||b.issues.length);
 }
 function groups(rows,config,modules) {
  return config.searchQuery.trim()||config.groupBy==='none'||(config.groupBy==='status'&&config.filterStatus&&config.filterStatus!=='@unresolved')
   ?[{key:'all',label:'',issues:rows}]:buckets(rows,config.groupBy,modules,false);
 }
 function boardLanes(rows,config,modules){return config.laneBy==='none'?[{key:'',label:'',issues:rows}]:buckets(rows,config.laneBy,modules,true);}
 function adapt(row,projectId){return {...row,project_id:projectId,sequence:row.sequence??Number(/-(\d+)$/.exec(row.identifier)?.[1]??0),description:row.description??row.preview??''};}
 const editable=role=>role&&(role.is_admin||!role.enforced||role.role==='lead'||role.role==='maintainer');
 function storageRead(storage,key,fallback){try{const raw=storage?.getItem(key);if(raw===null||raw===undefined)return fallback;try{return JSON.parse(raw);}catch{return typeof fallback==='string'?raw:fallback;}}catch{return fallback;}}
 function checkDefault(storage,key){try{if(!storage||storage.getItem(key))return false;storage.setItem(key,'1');return true;}catch{return false;}}
 function storageWrite(storage,key,value){try{storage?.setItem(key,typeof value==='string'?value:JSON.stringify(value));}catch{/* Preferences remain usable in memory when storage is unavailable. */}}
 const DELETIONS_KEY='lific:issue-list:deferred-deletions';
 const runningDeletions=new Map();
 const deletionListeners=new Map();
 async function deletionOwner(identity) {
  if(!globalThis.crypto?.subtle)return null;
  const digest=await globalThis.crypto.subtle.digest('SHA-256',new TextEncoder().encode(String(identity)));
  return [...new Uint8Array(digest)].map(byte=>byte.toString(16).padStart(2,'0')).join('');
 }
 function deletionRecords(env) {
  const records=storageRead(env.sessionStorage,DELETIONS_KEY,[]);
  return Array.isArray(records)?records.filter(record=>typeof record?.id==='string'&&typeof record.owner==='string'&&Number.isFinite(record.deadline)&&Array.isArray(record.rows)&&record.rows.every(row=>Number.isSafeInteger(row.id)&&row.id>0)):[];
 }
 function storeDeletion(env,record) {
  try {if(!env.sessionStorage||record.owner===null)return false;env.sessionStorage.setItem(DELETIONS_KEY,JSON.stringify([...deletionRecords(env).filter(r=>r.id!==record.id),record]));return true;}catch{return false;}
 }
 async function queueDeletion(rows,env,delay=5000) {
  const identity=env.identity?.(),owner=await deletionOwner(identity);
  if(identity!==env.identity?.())return {queued:false,reason:'identity_changed'};
  if(owner===null||!Array.isArray(rows)||!rows.length)return {queued:false,reason:'storage_unavailable'};
  const record={id:globalThis.crypto?.randomUUID?.()??`delete-${Date.now()}-${Math.random()}`,owner,deadline:Date.now()+delay,rows};
  return storeDeletion(env,record)?{queued:true,record}:{queued:false,reason:'storage_unavailable'};
 }
 function discardDeletion(env,id) {try{env.sessionStorage?.setItem(DELETIONS_KEY,JSON.stringify(deletionRecords(env).filter(record=>record.id!==id)));for(const [listener,storage]of deletionListeners)if(storage===env.sessionStorage)listener();}catch{/* A replayed DELETE is idempotent if removal of its record fails. */}}
 async function performDeletion(record,env) {
  if(runningDeletions.has(record.id))return runningDeletions.get(record.id);
  const completion=(async()=>{
   const identity=Object.hasOwn(env,'expectedIdentity')?env.expectedIdentity:env.identity?.();
   if(await deletionOwner(identity)!==record.owner||identity!==env.identity?.()){discardDeletion(env,record.id);return {cancelled:true,results:[]};}
   const results=await Promise.all(record.rows.map(async row=>{
    try {if(identity!==env.identity?.())throw new Error('Account changed. Deletion cancelled.');await env.api.remove(row.id);return {row};}
    catch(error){return error.status===404?{row}:{row,error};}
   }));
   if(identity!==env.identity?.()){discardDeletion(env,record.id);return {cancelled:true,results:[]};}
   await Promise.allSettled([...new Set(record.rows.map(row=>row.project_id))].map(id=>env.sync?.refreshProject(id)));
   discardDeletion(env,record.id);
   if(identity!==env.identity?.())return {cancelled:true,results:[]};
   return {cancelled:false,results};
  })();
  runningDeletions.set(record.id,completion);
  try{return await completion;}finally{runningDeletions.delete(record.id);}
 }
 async function resumeDeletions(env) {
  const identity=env.identity?.(),owner=await deletionOwner(identity);
  if(identity!==env.identity?.())return [];
  const records=deletionRecords(env);
  for(const record of records){
   if(record.owner!==owner){discardDeletion(env,record.id);continue;}
   let phase='pending';
   const timer=(env.schedule||setTimeout)(async()=>{if(phase!=='pending')return;phase='deleting';env.onDeleting?.(record);const outcome=await performDeletion(record,env);phase='complete';if(!outcome.cancelled&&identity===env.identity?.())env.onComplete?.(outcome.results,record);},Math.max(0,record.deadline-Date.now()));
   env.onPending?.(record,()=>{if(phase!=='pending'||runningDeletions.has(record.id))return false;phase='cancelled';(env.cancel||clearTimeout)(timer);discardDeletion(env,record.id);void Promise.allSettled([...new Set(record.rows.map(row=>row.project_id))].map(id=>env.sync?.refreshProject(id)));return true;});
  }
  return records.filter(record=>record.owner===owner);
 }
 function createApi(session) {
  const read=async(path,options)=>{const result=await session.request(path,options);if(!result.ok){const error=new Error(result.error||`HTTP ${result.status}`);error.status=result.status;throw error;}return result.data;};
  return {
   projects:()=>read('/projects'),modules:id=>read(`/modules?project_id=${id}`),labels:id=>read(`/labels?project_id=${id}`),role:id=>read(`/projects/${id}/my-role`),
   index:id=>read(`/projects/${id}/index`),views:id=>read(`/projects/${id}/views`),
   saveView:(id,name,config,isDefault)=>read(`/projects/${id}/views`,{method:'POST',body:JSON.stringify({name,config:JSON.stringify(config),is_default:isDefault})}),
   updateView:(id,viewId,patch)=>read(`/projects/${id}/views/${viewId}`,{method:'PATCH',body:JSON.stringify(patch)}),deleteView:(id,viewId)=>read(`/projects/${id}/views/${viewId}`,{method:'DELETE'}),
   update:(id,patch)=>read(`/issues/${id}`,{method:'PUT',body:JSON.stringify(patch)}),remove:id=>read(`/issues/${id}`,{method:'DELETE',keepalive:true}),
   issue:(id,options)=>read(`/issues/${id}`,options),create:input=>read('/issues',{method:'POST',body:JSON.stringify(input)})
  };
 }
 class Controller {
  constructor(env) {
   this.env=env;this.generation=0;this.disposed=false;this.inFlight=0;this.local=new Map();this.undo=null;this.pendingDelete=null;this.deletionOwner=null;
   this.state={loading:true,error:'',message:'',rows:[],projects:[],modules:[],labels:[],roles:new Map(),views:[],selected:new Set(),lastSelected:null,config:parseConfig({...DEFAULT,layout:env.layout||'list'}),subTab:'all',collapsedGroups:new Set(),collapsedLanes:new Set(),collapsedColumns:new Set(),busy:false,canExport:!env.publicScope,activeView:null};
   this.unsubscribe=env.sync?.subscribe(()=>this.syncChanged());
   this.deletionChanged=()=>this.syncChanged();deletionListeners.set(this.deletionChanged,env.sessionStorage);
  }
  publish(){if(!this.disposed)this.env.onChange?.(this.state);}
  valid(term){return !this.disposed&&term===this.generation;}
  key(){return this.env.projectIdentifier??'workspace';}
  deferredIds(){return new Set([...(this.pendingDelete?.ids??[]),...deletionRecords(this.env).filter(record=>record.owner===this.deletionOwner).flatMap(record=>record.rows.map(row=>row.id))]);}
  visible(){return visibleIssues(this.state.rows,this.state.config,this.state.modules,this.state.subTab);}
  canEdit(id){const row=this.state.rows.find(i=>i.id===id);return !this.env.publicScope&&!!row&&editable(this.state.roles.get(row.project_id));}
  async load() {
   const term=++this.generation;this.inFlight=0;this.state.busy=this.pendingDelete?.deleting===true;this.state.loading=true;this.state.error='';this.publish();
   try {
    const projects=await this.env.api.projects();if(!this.valid(term))return;
    const relevant=this.env.projectIdentifier?projects.filter(p=>p.identifier.toLowerCase()===this.env.projectIdentifier.toLowerCase()):projects;
    if(this.env.projectIdentifier&&!relevant.length)throw new Error('Project is unavailable.');
    const chunks=await Promise.all(relevant.map(async p=>{
     const [model,modules,labels,role]=await Promise.all([this.env.publicScope?this.env.api.index(p.id):this.env.sync.ensureProject(p.id),this.env.api.modules(p.id),this.env.api.labels(p.id),this.env.api.role(p.id)]);
     if(!model||model.error||model.status==='cold')throw new Error(model?.error||'Could not load issues.');
     return {project:p,rows:model.issues.map(i=>adapt(i,p.id)),modules,labels,role};
    }));
    if(!this.valid(term))return;
    this.state.projects=relevant;this.state.modules=chunks.flatMap(c=>c.modules);this.state.labels=[...new Map(chunks.flatMap(c=>c.labels).map(l=>[l.name,l])).values()];this.state.roles=new Map(chunks.map(c=>[c.project.id,c.role]));
    const owner=await deletionOwner(this.env.identity?.());if(!this.valid(term))return;this.deletionOwner=owner;const deferred=this.deferredIds();
    this.state.rows=chunks.flatMap(c=>c.rows).filter(row=>!deferred.has(row.id));this.local.clear();
    const key=this.key(),stored=storageRead(this.env.storage,`lific:list:state:${key}`,{});
    const extras={layout:this.env.layout||'list',laneBy:storageRead(this.env.storage,`lific:board:lanes:${key}`,'none'),hiddenStatuses:storageRead(this.env.storage,`lific:board:hidden-statuses:${key}`,[])};
    this.state.config=queryConfig(this.env.search||'',parseConfig({...stored,...extras}));
    this.state.collapsedGroups=new Set(storageRead(this.env.storage,`lific:list:collapsed:${key}`,[]));this.state.collapsedLanes=new Set(storageRead(this.env.storage,`lific:board:collapsed-lanes:${key}`,[]));this.state.collapsedColumns=new Set(storageRead(this.env.storage,`lific:board:collapsed-columns:${key}`,[]));
    if(relevant.length===1) {
     const id=relevant[0].id;this.env.sync?.setActiveProject(id);
     this.state.subTab=storageRead(this.env.storage,`lific:subtab:issues:${id}`,'all');
     if(!this.env.publicScope){
      try {
       const views=await this.env.api.views(id);if(!this.valid(term))return;this.state.views=views;
       this.state.activeView=storageRead(this.env.sessionStorage,`lific:views:active:${key}`,null);
       const applyDefault=checkDefault(this.env.sessionStorage,`lific:views:session-checked:${key}`);
       const defaultView=views.find(v=>v.is_default);
       if(applyDefault&&defaultView&&!new URLSearchParams(this.env.search||'').size)this.applyView(defaultView.id);
      }catch(error){if(this.valid(term))this.state.error=`Saved views: ${error.message}`;}
     }
    }else this.env.sync?.setActiveProject(null);
    this.prune();
   }catch(error){if(this.valid(term)){this.state.error=error.message;if([401,403,404].includes(error.status)){this.state.rows=[];this.state.selected.clear();}}}
   finally{if(this.valid(term)){this.state.loading=false;this.publish();}}
  }
  syncChanged() {
   if(this.state.loading||this.inFlight||this.pendingDelete?.deleting||this.env.publicScope||this.disposed)return;
   const rows=this.state.projects.flatMap(p=>{const model=this.env.sync.peekProject(p.id);if(!model){return [];}return model.issues.map(i=>adapt(i,p.id));});
   const deferred=this.deferredIds();
   this.state.rows=rows.filter(i=>!deferred.has(i.id)).map(i=>{const local=this.local.get(i.id);return local&&local.seq>i.seq?local:i;});
   this.prune();this.publish();
  }
  prune(){const ids=new Set(this.visible().map(i=>i.id));this.state.selected=new Set([...this.state.selected].filter(id=>ids.has(id)&&this.canEdit(id)));}
  change(config){this.state.config=parseConfig(config);this.persist();this.prune();this.publish();this.env.onQuery?.(configQuery(this.state.config,this.env.search||''));}
  persist(){const key=this.key(),c=this.state.config;storageWrite(this.env.storage,`lific:list:state:${key}`,c);storageWrite(this.env.storage,`lific:list:layout:${key}`,c.layout);storageWrite(this.env.storage,`lific:board:lanes:${key}`,c.laneBy);storageWrite(this.env.storage,`lific:board:hidden-statuses:${key}`,c.hiddenStatuses);}
  select(id,range=false) {
   if(this.state.busy||!this.canEdit(id))return;
   const visible=this.visible(),idx=visible.findIndex(i=>i.id===id),anchor=visible.findIndex(i=>i.id===this.state.lastSelected);
   if(range&&anchor>=0&&idx>=0){for(const row of visible.slice(Math.min(idx,anchor),Math.max(idx,anchor)+1))if(this.canEdit(row.id))this.state.selected.add(row.id);}
   else if(this.state.selected.has(id))this.state.selected.delete(id);else for(const row of visible.filter(i=>i.id===id))this.state.selected.add(row.id);
   this.state.lastSelected=id;this.publish();
  }
  selectAll(){if(!this.state.busy){this.state.selected=new Set(this.visible().filter(i=>this.canEdit(i.id)).map(i=>i.id));this.publish();}}
  clear(){this.state.selected.clear();this.state.lastSelected=null;this.publish();}
  toggle(kind,key){const set=this.state[kind];if(set.has(key))set.delete(key);else set.add(key);const storageKey={collapsedGroups:'lific:list:collapsed:',collapsedLanes:'lific:board:collapsed-lanes:',collapsedColumns:'lific:board:collapsed-columns:'}[kind];storageWrite(this.env.storage,`${storageKey}${this.key()}`,[...set]);this.publish();}
  applyView(id){const saved=this.state.views.find(v=>v.id===id),config=saved?parseConfig(saved.config):null;if(config){this.state.activeView=id;storageWrite(this.env.sessionStorage,`lific:views:active:${this.key()}`,id);this.change(config);}else if(saved){this.state.error='This saved view contains invalid settings.';this.publish();}}
  async saveView(name,isDefault=false){return this.viewMutation(()=>this.env.api.saveView(this.state.projects[0].id,name,this.state.config,isDefault));}
  async updateView(patch){return this.viewMutation(()=>this.env.api.updateView(this.state.projects[0].id,this.state.activeView,patch));}
  async deleteView(){const id=this.state.activeView,term=this.generation,deleted=await this.viewMutation(()=>this.env.api.deleteView(this.state.projects[0].id,id));if(deleted&&this.valid(term)&&this.state.activeView===id){this.state.activeView=null;storageWrite(this.env.sessionStorage,`lific:views:active:${this.key()}`,null);this.publish();}}
  async viewMutation(operation){if(this.env.publicScope||this.state.projects.length!==1||this.state.busy)return;const term=this.generation;let succeeded=false;this.state.busy=true;this.publish();try{const saved=await operation();succeeded=this.valid(term);const views=await this.env.api.views(this.state.projects[0].id);if(this.valid(term)){this.state.views=views;if(saved.id)this.state.activeView=saved.id;this.state.message='Saved views updated.';return true;}}catch(error){if(this.valid(term))this.state.error=error.message;return succeeded;}finally{if(this.valid(term)){this.state.busy=false;this.publish();}}}
  async write(targets,patchFor,verb='updated',recordUndo=true) {
   if(this.state.busy||!targets.length||targets.some(i=>!this.canEdit(i.id)))return;
   const term=this.generation;this.state.busy=true;this.state.error='';this.inFlight++;this.publish();
   const results=await Promise.all(targets.map(async row=>{const patch=patchFor(row);try{return {row,patch,issue:await this.env.api.update(row.id,patch)};}catch(error){return {row,error};}}));
   if(!this.valid(term))return;
   const succeeded=results.filter(r=>!r.error),failed=results.filter(r=>r.error);
   for(const result of succeeded){const fresh={...result.row,...result.issue};this.local.set(fresh.id,fresh);this.state.rows=this.state.rows.map(i=>i.id===fresh.id?fresh:i);}
   this.state.selected=new Set(failed.map(r=>r.row.id));this.state.message=`${succeeded.length} ${verb}${failed.length?`; ${failed.length} failed`:''}.`;
   this.state.error=failed.map(r=>`${r.row.identifier}: ${r.error.message}`).join('\n');
   if(recordUndo&&succeeded.length)this.undo=succeeded.map(r=>({id:r.row.id,patch:Object.fromEntries(Object.keys(r.patch).map(key=>[key,r.row[key]]))}));
   this.inFlight--;this.state.busy=false;this.publish();
   await Promise.allSettled([...new Set(targets.map(i=>i.project_id))].map(id=>this.env.sync?.refreshProject(id)));
  }
  bulk(patch){return this.write(this.state.rows.filter(i=>this.state.selected.has(i.id)),()=>patch);}
  addLabel(name){return this.write(this.state.rows.filter(i=>this.state.selected.has(i.id)&&!i.labels.includes(name)),row=>({labels:[...row.labels,name]}));}
  async undoWrite(){const undo=this.undo;if(!undo)return;this.undo=null;const patches=new Map(undo.map(u=>[u.id,u.patch]));await this.write(this.state.rows.filter(i=>patches.has(i.id)),row=>patches.get(row.id),'restored',false);}
  move(id,status,lane,beforeId,afterId) {
   const row=this.state.rows.find(i=>i.id===id);if(!row)return;
   const before=this.state.rows.find(i=>i.id===beforeId)?.sort_order,after=this.state.rows.find(i=>i.id===afterId)?.sort_order;
   const sort_order=before===undefined?(after===undefined?0:after-1):after===undefined?before+1:(before+after)/2;
   const patch={status,sort_order};if(this.state.config.laneBy==='module')patch.module_id=lane==='none'?null:Number(lane);if(this.state.config.laneBy==='priority')patch.priority=lane;
   return this.write([row],()=>patch);
  }
  async scheduleDelete() {
   if(this.state.busy||this.pendingDelete)return;
   const rows=this.state.rows.filter(i=>this.state.selected.has(i.id)&&this.canEdit(i.id));if(!rows.length)return;
   const identity=this.env.identity?.(),term=this.generation;this.state.busy=true;this.publish();
   let prepared;const preparation=new Promise(resolve=>{prepared=resolve;});this.deletePreparation=preparation;
   try{
   const record={id:globalThis.crypto?.randomUUID?.()??`delete-${Date.now()}-${Math.random()}`,owner:await deletionOwner(identity),deadline:Date.now()+5000,rows};
   if(!this.valid(term)||identity!==this.env.identity?.())return;
   const ids=new Set(rows.map(i=>i.id));this.pendingDelete={ids,rows,identity,record,deleting:false};
   const durable=storeDeletion(this.env,record);
   this.state.rows=this.state.rows.filter(i=>!ids.has(i.id));this.state.selected.clear();this.undo=null;this.state.error='';this.state.busy=false;
   if(durable){this.state.message=`${rows.length} deleted. Undo is available for 5 seconds.`;this.publish();this.deleteTimer=(this.env.schedule||setTimeout)(()=>this.finishDelete(),5000);}
   else await this.finishDelete();
   }finally{if(this.deletePreparation===preparation)this.deletePreparation=null;prepared();}
  }
  undoDelete(){const pending=this.pendingDelete;if(!pending||pending.deleting)return;(this.env.cancel||clearTimeout)(this.deleteTimer);discardDeletion(this.env,pending.record.id);this.state.rows=[...new Map([...this.state.rows,...pending.rows].map(row=>[row.id,row])).values()];this.pendingDelete=null;this.state.message='Deletion undone.';this.publish();}
  cancelDelete(){const pending=this.pendingDelete;if(!pending)return;(this.env.cancel||clearTimeout)(this.deleteTimer);discardDeletion(this.env,pending.record.id);this.pendingDelete=null;}
  async finishDelete() {
   const pending=this.pendingDelete;if(!pending)return;
   if(pending.completion)return pending.completion;
   (this.env.cancel||clearTimeout)(this.deleteTimer);pending.deleting=true;this.state.busy=true;this.publish();
   pending.completion=(async()=>{
    const outcome=await performDeletion(pending.record,{...this.env,expectedIdentity:pending.identity});
    if(this.pendingDelete!==pending)return;
    this.pendingDelete=null;this.state.busy=false;
    if(!outcome.cancelled&&pending.identity===this.env.identity?.()){
     const failed=outcome.results.filter(result=>result.error);this.state.rows=[...new Map([...this.state.rows,...failed.map(result=>result.row)].map(row=>[row.id,row])).values()];this.state.selected=new Set(failed.map(result=>result.row.id));this.state.error=failed.map(result=>`${result.row.identifier}: ${result.error.message}`).join('\n');this.state.message=`${outcome.results.length-failed.length} deleted; ${failed.length} failed.`;
    }
    this.publish();
   })();
   return pending.completion;
  }
  async create(title,status='backlog') {
   const project=this.state.projects[0];if(this.state.projects.length!==1||!editable(this.state.roles.get(project.id))||this.env.publicScope||this.state.busy)return;
   const term=this.generation;this.state.busy=true;this.publish();try{const row=await this.env.api.create({project_id:project.id,title,status});if(this.valid(term)){this.state.rows.push(row);this.state.message=`Created ${row.identifier}.`;void this.env.sync?.refreshProject(project.id);}}catch(error){if(this.valid(term))this.state.error=error.message;}finally{if(this.valid(term)){this.state.busy=false;this.publish();}}
  }
  dispose(){this.disposed=true;this.generation++;this.unsubscribe?.();deletionListeners.delete(this.deletionChanged);}
 }
 async function exportSelected(rows,env) {
  if(env.publicScope)throw new Error('Export is unavailable in the public view.');
  const identity=env.identity(),parts=[],separator=new TextEncoder().encode('\n\n---\n\n');let size=0;
  for(const row of rows){
   if(env.identity()!==identity)throw new Error('Account changed. Export cancelled.');
   const response=await env.fetch(`/api/export/issues/${encodeURIComponent(row.identifier)}`,{headers:env.headers(),signal:env.signal});
   if(env.identity()!==identity){await response.body?.cancel();throw new Error('Account changed. Export cancelled.');}
   if(!response.ok)throw new Error(`Could not export ${row.identifier} (HTTP ${response.status}).`);
   if(!response.body)throw new Error(`No export returned for ${row.identifier}.`);
   if(parts.length){parts.push(separator);size+=separator.byteLength;}
   const reader=response.body.getReader();
   try{for(;;){const {done,value}=await reader.read();if(done)break;size+=value.byteLength;if(size>16*1024*1024){await reader.cancel();throw new Error('Selected exports exceed 16 MiB. Select fewer issues and try again.');}if(env.identity()!==identity){await reader.cancel();throw new Error('Account changed. Export cancelled.');}parts.push(value);}}
   finally{reader.releaseLock();}
  }
  return new Blob(parts,{type:'text/markdown;charset=utf-8'});
 }
 function attach(root,env={}) {
  const doc=root.ownerDocument,win=doc.defaultView,session=env.session||win.lificSession;
  const api=env.api||createApi(session),sync=env.sync||win.lificSync;
  let publicScope=session.state.publicProject!==null;
  const storage=()=>{try{return win.localStorage;}catch{return undefined;}};
  const sessionStorage=()=>{try{return win.sessionStorage;}catch{return undefined;}};
  const route=()=>win.location.hash.startsWith('#/')?new URL(win.location.hash.slice(1),win.location.origin):new URL(win.location.href);
  const node=(tag,text,attrs={})=>{const element=doc.createElement(tag);if(text!==undefined)element.textContent=text;for(const [key,value]of Object.entries(attrs))element.setAttribute(key,value);return element;};
  const button=(text,action,attrs={})=>node('button',text,{type:'button','data-action':action,...attrs});
  const field=(label,name,values,value)=>{const wrapper=node('label',label),select=node('select',undefined,{'data-config':name,'aria-label':label});for(const [key,text]of values)select.append(node('option',text,{value:key}));select.value=value;wrapper.append(select);return wrapper;};
  const form=root.querySelector('[data-issues-controls]'),content=root.querySelector('[data-issues-content]'),feedback=root.querySelector('[data-issues-feedback]'),bulk=root.querySelector('[data-issues-bulk]'),peek=root.querySelector('[data-issues-peek]');
  let controller,peekGeneration=0,peekController=null,focusIndex=-1,creationDraft='',activeIdentity;
  function href(row){const project=controller.state.projects.find(p=>p.id===row.project_id);return `${publicScope?'/public':''}/${encodeURIComponent(project?.identifier||row.identifier.replace(/-\d+$/,''))}/issues/${encodeURIComponent(row.identifier)}`;}
  function navigate(href){win.dispatchEvent(new win.CustomEvent('lific:navigate',{detail:{href,history:'push'}}));}
  function retainFocus(render){const active=doc.activeElement,key=active?.dataset?.focusKey,selection=active?.selectionStart===null?null:[active?.selectionStart,active?.selectionEnd,active?.selectionDirection];render();if(key){const next=[...root.querySelectorAll('[data-focus-key]')].find(n=>n.dataset.focusKey===key);if(next){next.focus({preventScroll:true});if(selection&&typeof selection[0]==='number'&&next.setSelectionRange)next.setSelectionRange(...selection);}else root.querySelector('[data-search]')?.focus({preventScroll:true});}}
  function render(state) {
   retainFocus(()=>{
    root.setAttribute('aria-busy',String(state.loading));root.dataset.layout=state.config.layout;root.dataset.density=state.config.density;
    feedback.replaceChildren();feedback.append(node('p',state.loading?'Loading issues…':`${controller.visible().length} issues`,{'role':'status','aria-live':'polite'}));
    if(state.error)feedback.append(node('p',state.error,{role:'alert',class:'tc-issues__error'}));
    if(state.message)feedback.append(node('p',state.message,{role:'status','aria-live':'polite'}));
    if(controller.undo||controller.pendingDelete)feedback.append(button('Undo','undo',{'data-focus-key':'undo'}));
    if(state.error)feedback.append(button('Retry','retry'));
    const c=state.config;
    form.replaceChildren();const search=node('input',undefined,{type:'search','data-search':'','aria-label':'Search issues',placeholder:'Search issues','data-focus-key':'search'});search.value=c.searchQuery;form.append(search);
    for(const [label,key,values]of [
     ['Status','filterStatus',[['','All statuses'],['@unresolved','Unresolved'],...STATUSES.map(x=>[x,x])]],['Priority','filterPriority',[['','All priorities'],...PRIORITIES.map(x=>[x,x])]],
     ['Label','filterLabel',[['','All labels'],...state.labels.map(x=>[x.name,x.name])]],['Module','filterModule',[['','All modules'],['@none','No module'],...state.modules.map(x=>[x.name,x.name])]],
     ['Sort','sortField',['priority','age','number','updated'].map(x=>[x,x])],['Order','sortDir',[['asc','Ascending'],['desc','Descending']]],
     ['Group','groupBy',['status','priority','module','none'].map(x=>[x,x])],['Density','density',['compact','comfortable'].map(x=>[x,x])],['Layout','layout',[['list','List'],['board','Board']]],
     ...(c.layout==='board'?[['Swimlanes','laneBy',['none','module','priority'].map(x=>[x,x])]]:[])
    ]){const f=field(label,key,values,c[key]);f.querySelector('select').dataset.focusKey=key;form.append(f);}
    form.append(button('Clear filters','clear-filters',{'data-focus-key':'clear-filters'}));
    if(c.layout==='list'&&!doc.querySelector('[data-subtabs][data-view="issues"]')){const tabs=node('nav',undefined,{'aria-label':'Issue views'});for(const tab of ['all','open','closed','recent'])tabs.append(button(tab,'subtab',{'data-tab':tab,'aria-pressed':String(state.subTab===tab),'data-focus-key':`subtab:${tab}`}));form.append(tabs);}
    if(c.layout==='board'){const columns=node('fieldset');columns.append(node('legend','Visible columns'));for(const status of STATUSES){const label=node('label',status),checkbox=node('input',undefined,{type:'checkbox','data-hidden-status':status,'data-focus-key':`column:${status}`});checkbox.checked=!c.hiddenStatuses.includes(status);label.append(checkbox);columns.append(label);}form.append(columns);}
    if(!publicScope&&state.projects.length===1){const label=node('label','Saved view'),select=node('select',undefined,{'data-saved-view':'','aria-label':'Saved view','data-focus-key':'saved-view'});select.append(node('option','Custom',{value:''}));for(const view of state.views)select.append(node('option',`${view.name}${view.is_default?' (default)':''}`,{value:String(view.id)}));select.value=String(state.activeView??'');label.append(select);form.append(label,button('Save view','save-view'),...(state.activeView?[button('Update view','update-view'),button('Rename view','rename-view'),button('Make default','default-view'),button('Delete view','delete-view')]:[]));}
    const writableProject=state.projects.length===1&&editable(state.roles.get(state.projects[0].id))&&!publicScope;
    if(writableProject){const create=node('form',undefined,{'data-create-issue':''}),input=node('input',undefined,{name:'title',required:'required',placeholder:'New issue title','aria-label':'New issue title','data-focus-key':'new-title'});input.value=creationDraft;create.append(input,node('button','Create issue',{type:'submit'}));form.append(create);}
    bulk.replaceChildren();bulk.hidden=state.selected.size===0;
    if(state.selected.size){bulk.append(node('strong',`${state.selected.size} selected`),button('Clear selection','clear-selection'));
     for(const [label,key,values]of [['Set status','status',STATUSES],['Set priority','priority',PRIORITIES],['Set module','module_id',[['null','No module'],...state.modules.map(m=>[String(m.id),m.name])]],['Add label','label',state.labels.map(l=>l.name)]]){
      const f=field(label,key,[['',label],...values.map(v=>Array.isArray(v)?v:[v,v])],'');f.querySelector('select').removeAttribute('data-config');f.querySelector('select').dataset.bulk=key;f.querySelector('select').disabled=state.busy;bulk.append(f);
     }
     bulk.append(button('Delete selected','delete-selected'),button('Export selected','export'));for(const b of bulk.querySelectorAll('button'))b.disabled=state.busy;
    }
    content.replaceChildren();if(state.loading)return;
    const rows=controller.visible();if(!rows.length){content.append(node('p',state.error?'Issues could not be loaded.':'No issues match this view.',{class:'tc-issues__empty'}));return;}
    if(rows.some(i=>controller.canEdit(i.id)))content.append(button('Select all visible','select-all',{'data-focus-key':'select-all'}));
    function rowNode(row,board=false){const wrapper=node('article',undefined,{class:'tc-issues__row','data-issue-id':String(row.id),'data-focus-key':`row:${row.id}`,tabindex:'0'});wrapper.dataset.editable=String(controller.canEdit(row.id));
     if(controller.canEdit(row.id)){const checkbox=node('input',undefined,{type:'checkbox','data-select':String(row.id),'aria-label':`Select ${row.identifier}`,'data-focus-key':`select:${row.id}`});checkbox.checked=state.selected.has(row.id);checkbox.disabled=state.busy;wrapper.append(checkbox);}
     const link=node('a',`${row.identifier} ${row.title}`,{href:href(row),'data-detail':'','data-focus-key':`detail:${row.id}`});wrapper.append(link,node('span',`${row.priority} · ${row.status}`,{class:'tc-issues__metadata'}));
     const blocked=row.blocked_by?.length||row.blocker_count||0,waits=(row.waits||[]).filter(w=>w.state!=='cleared');if(blocked)wrapper.append(node('span',`Blocked${row.blocked_by?.length?` by ${row.blocked_by.join(', ')}`:` (${blocked})`}`,{class:'tc-issues__blocked'}));
     for(const wait of waits)wrapper.append(node('span',`${wait.state}: ${wait.kind==='user'?wait.display_name||wait.username:wait.earliest||''}${wait.note?` · ${wait.note}`:''}`,{class:'tc-issues__wait'}));
     if(c.density==='comfortable'&&row.description)wrapper.append(node('p',row.description.split('\n').find(line=>line.trim()&&!line.startsWith('#'))?.replace(/[*_`>[\]]/g,'').slice(0,160)||''));
     wrapper.append(button(`Peek ${row.identifier}`,'peek',{'data-id':String(row.id),'data-focus-key':`peek:${row.id}`}));
     if(controller.canEdit(row.id)){const select=field(`Status ${row.identifier}`,'status',STATUSES.map(s=>[s,s]),row.status).querySelector('select');select.removeAttribute('data-config');select.dataset.rowStatus=String(row.id);select.dataset.focusKey=`status:${row.id}`;select.disabled=state.busy;wrapper.append(select);}
     if(board&&controller.canEdit(row.id)){wrapper.draggable=!state.busy;wrapper.setAttribute('aria-label',`${row.identifier}, ${row.title}, ${row.status}. Drag to move or use the status control.`);}
     return wrapper;
    }
    if(c.layout==='list'){for(const group of groups(rows,c,state.modules)){const section=node('section',undefined,{class:'tc-issues__group'}),key=`${c.groupBy}:${group.key}`,collapsed=state.collapsedGroups.has(key);if(group.label)section.append(button(`${group.label} (${group.issues.length})`,'collapse-group',{'data-key':key,'aria-expanded':String(!collapsed),'data-focus-key':`group:${key}`}));if(!collapsed)for(const row of group.issues)section.append(rowNode(row));content.append(section);}}
    else for(const lane of boardLanes(rows,c,state.modules)){const section=node('section',undefined,{class:'tc-issues__lane'}),collapsed=state.collapsedLanes.has(lane.key);if(lane.label)section.append(button(`${lane.label} (${lane.issues.length})`,'collapse-lane',{'data-key':lane.key,'aria-expanded':String(!collapsed),'data-focus-key':`lane:${lane.key}`}));if(!collapsed){const columns=node('div',undefined,{class:'tc-issues__columns'});for(const status of STATUSES.filter(s=>!c.hiddenStatuses.includes(s)&&(!c.filterStatus||(c.filterStatus==='@unresolved'?!['done','cancelled'].includes(s):c.filterStatus===s)))){const column=node('section',undefined,{class:'tc-issues__column','data-drop-status':status,'data-drop-lane':lane.key,'aria-label':`${lane.label} ${status}`});column.append(button(`${status} (${lane.issues.filter(i=>i.status===status).length})`,'collapse-column',{'data-key':status,'aria-expanded':String(!state.collapsedColumns.has(status)),'data-focus-key':`column:${lane.key}:${status}`}));if(!state.collapsedColumns.has(status))for(const row of lane.issues.filter(i=>i.status===status).sort((a,b)=>a.sort_order-b.sort_order))column.append(rowNode(row,true));columns.append(column);}section.append(columns);}content.append(section);}
   });
  }
  controller=new Controller({...env,api,sync,publicScope,projectIdentifier:root.dataset.projectIdentifier||null,layout:root.dataset.layout||'list',search:route().search,storage:storage(),sessionStorage:sessionStorage(),identity:()=>`${session.state.publicProject??''}:${storage()?.getItem('lific_token')??''}`,onChange:render,onQuery:query=>{const current=route(),targetLayout=controller.state.config.layout;if(root.dataset.projectIdentifier)current.pathname=current.pathname.replace(/\/(issues|board)$/,`/${targetLayout==='board'?'board':'issues'}`);current.search=query;win.history.replaceState(win.history.state,'',current.pathname+current.search);}});
  async function openPeek(id) {
   const term=++peekGeneration;peekController?.abort();peekController=new AbortController();const returnFocus=doc.activeElement;peek.replaceChildren(node('p','Loading issue…',{role:'status'}),button('Close peek','close-peek'));peek.showModal();
   peek.addEventListener('close',()=>{peekGeneration++;peekController?.abort();returnFocus?.isConnected&&returnFocus.focus();},{once:true});
   try{const issue=await api.issue(id,{signal:peekController.signal});if(term!==peekGeneration)return;peek.replaceChildren(node('h2',`${issue.identifier} ${issue.title}`),node('p',`${issue.status} · ${issue.priority}`),node('pre',issue.description,{class:'tc-issues__peek-description'}),node('a','Open issue',{href:href(issue),'data-detail':''}),button('Close peek','close-peek'));}catch(error){if(term===peekGeneration)peek.replaceChildren(node('p',error.message,{role:'alert'}),button('Close peek','close-peek'));}
  }
  async function exportRows(){if(controller.state.busy||!controller.state.canExport)return;const term=controller.generation;controller.state.busy=true;controller.publish();try{const blob=await exportSelected(controller.state.rows.filter(i=>controller.state.selected.has(i.id)),{publicScope,identity:controller.env.identity,fetch:win.fetch.bind(win),headers:()=>{const token=storage()?.getItem('lific_token');return token?{Authorization:`Bearer ${token}`}:{}}});if(!controller.valid(term))return;const link=node('a',undefined,{href:win.URL.createObjectURL(blob),download:`${controller.key()}-selected-issues.md`});doc.body.append(link);link.click();link.remove();win.setTimeout(()=>win.URL.revokeObjectURL(link.href),1000);}catch(error){if(controller.valid(term))controller.state.error=error.message;}finally{if(controller.valid(term)){controller.state.busy=false;controller.publish();}}}
  function click(event){const b=event.target.closest('[data-action]');if(!b||!root.contains(b))return;const action=b.dataset.action;
   if(action==='select-all')controller.selectAll();else if(action==='clear-selection')controller.clear();else if(action==='clear-filters')controller.change({...controller.state.config,filterStatus:'',filterPriority:'',filterLabel:'',filterModule:'',searchQuery:''});
   else if(action==='subtab'){controller.state.subTab=b.dataset.tab;if(controller.state.projects.length===1)storageWrite(storage(),`lific:subtab:issues:${controller.state.projects[0].id}`,b.dataset.tab);controller.prune();controller.publish();}
   else if(action.startsWith('collapse-'))controller.toggle({'collapse-group':'collapsedGroups','collapse-lane':'collapsedLanes','collapse-column':'collapsedColumns'}[action],b.dataset.key);
   else if(action==='undo'){if(controller.pendingDelete)controller.undoDelete();else void controller.undoWrite();}
   else if(action==='peek')void openPeek(Number(b.dataset.id));else if(action==='close-peek')peek.close();else if(action==='retry')void controller.load();
   else if(action==='delete-selected'){if(win.confirm(`Delete ${controller.state.selected.size} selected issues?`))controller.scheduleDelete();}
   else if(action==='export')void exportRows();
   else if(action==='save-view'){const name=win.prompt('Name this view');if(name?.trim())void controller.saveView(name.trim());}
   else if(action==='update-view')void controller.updateView({config:JSON.stringify(controller.state.config)});
   else if(action==='rename-view'){const name=win.prompt('Rename this view',controller.state.views.find(v=>v.id===controller.state.activeView)?.name);if(name?.trim())void controller.updateView({name:name.trim()});}
   else if(action==='default-view')void controller.updateView({is_default:true});else if(action==='delete-view'&&win.confirm('Delete this saved view?'))void controller.deleteView();
  }
  function change(event){const el=event.target;if(el.matches('[data-config]'))controller.change({...controller.state.config,[el.dataset.config]:el.value});
   else if(el.matches('[data-select]'))controller.select(Number(el.dataset.select),event.shiftKey);
   else if(el.matches('[data-row-status]'))void controller.write(controller.state.rows.filter(i=>i.id===Number(el.dataset.rowStatus)),()=>({status:el.value}));
   else if(el.matches('[data-saved-view]')){if(el.value)controller.applyView(Number(el.value));else{controller.state.activeView=null;storageWrite(sessionStorage(),`lific:views:active:${controller.key()}`,null);controller.publish();}}
   else if(el.matches('[data-hidden-status]')){const hidden=new Set(controller.state.config.hiddenStatuses);if(el.checked)hidden.delete(el.dataset.hiddenStatus);else hidden.add(el.dataset.hiddenStatus);controller.change({...controller.state.config,hiddenStatuses:[...hidden]});}
   else if(el.matches('[data-bulk]')&&el.value){const value=el.value,key=el.dataset.bulk;if(key==='label')void controller.addLabel(value);else void controller.bulk({[key]:key==='module_id'?(value==='null'?null:Number(value)):value});}
  }
  function input(event){if(event.target.matches('[data-create-issue] input'))creationDraft=event.target.value;if(event.target.matches('[data-search]'))controller.change({...controller.state.config,searchQuery:event.target.value});}
  function submit(event){if(event.target.matches('[data-create-issue]')){event.preventDefault();const title=new win.FormData(event.target).get('title');if(title?.trim())void controller.create(title.trim());}}
  function keydown(event){if(event.target.matches('input,select,textarea')||peek.open)return;const rows=[...content.querySelectorAll('[data-issue-id]')];const focused=event.target.closest('[data-issue-id]');if(focused)focusIndex=rows.indexOf(focused);if(['j','k','ArrowDown','ArrowUp'].includes(event.key)){event.preventDefault();focusIndex=Math.max(0,Math.min(rows.length-1,focusIndex+(['j','ArrowDown'].includes(event.key)?1:-1)));rows[focusIndex]?.focus();if(event.shiftKey&&rows[focusIndex])controller.select(Number(rows[focusIndex].dataset.issueId),true);}
   else if((event.ctrlKey||event.metaKey)&&event.key==='a'){event.preventDefault();controller.selectAll();}else if(event.key==='x'){const row=event.target.closest('[data-issue-id]');if(row){event.preventDefault();controller.select(Number(row.dataset.issueId),event.shiftKey);}}
   else if(event.key==='Escape'){controller.clear();}else if(event.key==='Enter'){const row=event.target.closest('[data-issue-id]');if(row&&event.target===row){event.preventDefault();navigate(row.querySelector('[data-detail]').getAttribute('href'));}}
   else if(event.code==='Space'){const row=event.target.closest('[data-issue-id]');if(row){event.preventDefault();void openPeek(Number(row.dataset.issueId));}}
  }
  function dragstart(event){const row=event.target.closest('[data-issue-id]');if(row&&controller.canEdit(Number(row.dataset.issueId))){event.dataTransfer.setData('application/x-lific-issue',row.dataset.issueId);event.dataTransfer.effectAllowed='move';}}
  function dragover(event){if(event.target.closest('[data-drop-status]')&&event.dataTransfer.types.includes('application/x-lific-issue'))event.preventDefault();}
  function drop(event){const column=event.target.closest('[data-drop-status]');if(!column)return;event.preventDefault();const id=Number(event.dataTransfer.getData('application/x-lific-issue'));const target=event.target.closest('[data-issue-id]'),cards=[...column.querySelectorAll('[data-issue-id]')].filter(n=>Number(n.dataset.issueId)!==id),index=target?cards.indexOf(target):cards.length;void controller.move(id,column.dataset.dropStatus,column.dataset.dropLane,index>0?Number(cards[index-1]?.dataset.issueId):undefined,index>=0&&index<cards.length?Number(cards[index].dataset.issueId):undefined);}
  function subtabChanged(event){if(event.detail?.view==='issues'&&controller.state.projects.some(p=>String(p.id)===String(event.detail.projectId))){controller.state.subTab=event.detail.id;controller.prune();controller.publish();}}
  doc.addEventListener('lific:subtab-change',subtabChanged);
  const listeners={click,change,input,submit,keydown,dragstart,dragover,drop};for(const [name,handler]of Object.entries(listeners))root.addEventListener(name,handler);
  activeIdentity=controller.env.identity();
  const accountChanged=()=>{const nextIdentity=controller.env.identity();if(nextIdentity===activeIdentity){if(session.state.projectId&&controller.state.roles.has(session.state.projectId)){controller.state.roles.set(session.state.projectId,session.state.role);controller.prune();controller.publish();}return;}activeIdentity=nextIdentity;creationDraft='';peekGeneration++;peekController?.abort();if(peek.open)peek.close();peek.replaceChildren();publicScope=session.state.publicProject!==null;controller.env.publicScope=publicScope;controller.state.canExport=!publicScope;controller.undo=null;controller.state.message='';controller.state.error='';controller.state.activeView=null;controller.local.clear();controller.cancelDelete();controller.generation++;controller.state.rows=[];controller.state.selected.clear();controller.state.roles.clear();controller.state.views=[];controller.publish();void controller.load();};
  for(const name of ['lific:account-change','lific:session-change','lific:scope-change'])win.addEventListener(name,accountChanged);
  let leaving=false;
  async function beforeNavigation(event) {
   if(leaving||(!controller.deletePreparation&&!controller.pendingDelete))return;
   const custom=event.type==='lific:navigate';
   const link=custom?null:event.target.closest('a[href]');
   if(!custom&&(!link||event.button!==0||event.ctrlKey||event.metaKey||event.altKey||event.shiftKey||link.hasAttribute('download')||(link.target&&link.target!=='_self')))return;
   const destination=custom?event.detail?.href:link.href;if(!destination)return;
   event.preventDefault();event.stopImmediatePropagation();
   await controller.deletePreparation;await controller.finishDelete();
   leaving=true;
   if(custom)win.dispatchEvent(new win.CustomEvent('lific:navigate',{detail:event.detail}));else win.location.assign(destination);
  }
  win.addEventListener('click',beforeNavigation,true);win.addEventListener('lific:navigate',beforeNavigation,true);
  const realtime=event=>{if(controller.state.projects.length!==1&&event.detail?.project_id&&controller.state.projects.some(p=>p.id===event.detail.project_id))void sync.refreshProject(event.detail.project_id);};win.addEventListener('lific:realtime',realtime);
  void controller.load();
  return {controller,dispose(){win.removeEventListener('click',beforeNavigation,true);win.removeEventListener('lific:navigate',beforeNavigation,true);doc.removeEventListener('lific:subtab-change',subtabChanged);peekGeneration++;peekController?.abort();if(peek.open)peek.close();controller.dispose();for(const [name,handler]of Object.entries(listeners))root.removeEventListener(name,handler);for(const name of ['lific:account-change','lific:session-change','lific:scope-change'])win.removeEventListener(name,accountChanged);win.removeEventListener('lific:realtime',realtime);}};
 }
 const exports={parseConfig,queryConfig,configQuery,visibleIssues,groups,boardLanes,Controller,createApi,exportSelected,resumeDeletions,queueDeletion,attach};
 if(typeof module!=='undefined')module.exports=exports;
 if(typeof window!=='undefined') {
  window.LificTopcoatIssueList=exports;
  async function start(){
   const session=window.lificSession,notices=new Map(),cancellations=new Map();
   const storage=(()=>{try{return window.sessionStorage;}catch{return undefined;}})();
   const identity=()=>{try{return `${session.state.publicProject??''}:${window.localStorage.getItem('lific_token')??''}`;}catch{return `${session.state.publicProject??''}:`;}};
   let replayIdentity=identity();
   const replayAccountChanged=()=>{if(identity()!==replayIdentity){replayIdentity=identity();for(const cancel of cancellations.values())cancel();cancellations.clear();for(const box of notices.values())box.remove();notices.clear();}};
   for(const event of ['lific:account-change','lific:session-change','lific:scope-change'])window.addEventListener(event,replayAccountChanged);
   function notice(record,text,error=false){let box=notices.get(record.id);if(!box){box=document.createElement('aside');box.className='tc-issues__deletion-feedback';document.body.append(box);notices.set(record.id,box);}box.replaceChildren();box.setAttribute('role',error?'alert':'status');const message=document.createElement('p');message.textContent=text;const dismiss=document.createElement('button');dismiss.type='button';dismiss.textContent='Dismiss';dismiss.addEventListener('click',()=>box.remove());box.append(message,dismiss);return box;}
   await resumeDeletions({sessionStorage:storage,identity,api:createApi(session),sync:window.lificSync,
    onPending(record,cancel){cancellations.set(record.id,cancel);const box=notice(record,`${record.rows.length} issues scheduled for deletion.`),undo=document.createElement('button');undo.type='button';undo.textContent='Undo deletion';undo.addEventListener('click',()=>{if(cancel())notice(record,'Deletion undone.');});box.append(undo);},
    onDeleting(record){notice(record,`Deleting ${record.rows.length} issues…`);},
    onComplete(results,record){cancellations.delete(record.id);const failed=results.filter(result=>result.error);notice(record,`${results.length-failed.length} deleted; ${failed.length} failed.${failed.length?`\n${failed.map(result=>`${result.row.identifier}: ${result.error.message}`).join('\n')}`:''}`,failed.length>0);}
   });
   for(const root of document.querySelectorAll('[data-topcoat-issue-list]')){root._lificIssueList?.dispose();root._lificIssueList=attach(root);}
  }
  if(document.readyState==='loading')document.addEventListener('DOMContentLoaded',start,{once:true});else start();
 }
})();

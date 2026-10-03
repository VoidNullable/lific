const {test}=require('node:test');
const assert=require('node:assert/strict');
const fs=require('node:fs');
const path=require('node:path');
test('headless issue list and board preserve interactive filters, selection, focus, peek and permission gates',{skip:!process.env.PLAYWRIGHT_EXECUTABLE_PATH},async t=>{
 const {chromium}=await import(path.resolve(__dirname,'../../../..','e2e/node_modules/playwright/index.mjs'));
 const browser=await chromium.launch({headless:true,executablePath:process.env.PLAYWRIGHT_EXECUTABLE_PATH});
 const page=await browser.newPage({viewport:{width:1200,height:900}});page.setDefaultTimeout(5000);const failures=[];page.on('pageerror',error=>failures.push(error.message));
 try{
  const html=`<!doctype html><html lang="en"><head><meta name="viewport" content="width=device-width"><style>:root{--tc-surface:white;--tc-bg:#eee;--tc-text:#222;--tc-border:#bbb;--tc-muted:#555;--tc-accent:#346;--tc-accent-text:white;--tc-focus:#346;--tc-font:system-ui;--tc-radius:4px}${fs.readFileSync(`${__dirname}/issue-list.css`,'utf8')}</style></head><body><section class="tc-issues" data-topcoat-issue-list data-project-identifier="ENG" data-layout="list"><h1>ENG issues</h1><div data-issues-controls></div><div data-issues-feedback></div><div data-issues-content></div><aside data-issues-bulk hidden></aside><dialog data-issues-peek aria-label="Issue preview"></dialog></section></body></html>`;
  await page.route('http://localhost/**',route=>route.fulfill({contentType:'text/html',body:html}));await page.goto('http://localhost/ENG/issues');
  await page.evaluate(()=>{
   window.makeIssue=(id,patch={})=>({id,project_id:7,identifier:`ENG-${id}`,sequence:id,title:`Issue ${id}`,preview:'Preview',labels:[],module_id:null,priority:'medium',status:'todo',created_at:`2026-10-0${id}`,updated_at:`2026-10-0${id}`,sort_order:id,seq:1,...patch});
   window.rows=[makeIssue(1),makeIssue(2),makeIssue(3,{status:'done'})];window.writes=[];window.listeners=[];
   window.lificSession={state:{publicProject:null,projectId:7,role:{enforced:true,role:'maintainer',is_admin:false}},request:async(path,options={})=>{
    if(path==='/projects')return {ok:true,data:[{id:7,identifier:'ENG'}]};
    if(path.startsWith('/modules'))return {ok:true,data:[{id:3,name:'Engine'}]};if(path.startsWith('/labels'))return {ok:true,data:[{name:'bug'}]};
    if(path.endsWith('my-role'))return {ok:true,data:lificSession.state.role};if(path.endsWith('/index'))return {ok:true,data:{issues:rows,pages:[],cursor:1}};if(path.endsWith('/views'))return {ok:true,data:[]};
    if(path.startsWith('/issues/')&&options.method==='PUT'){const id=Number(path.split('/')[2]);writes.push([id,JSON.parse(options.body)]);if(id===2)return {ok:false,status:403,error:'Permission denied'};return {ok:true,data:makeIssue(id,{...JSON.parse(options.body),seq:2})};}
    if(path.startsWith('/issues/'))return {ok:true,data:makeIssue(Number(path.split('/')[2]),{description:'Body <script>not HTML</script>'})};return {ok:false,status:404,error:'Missing mock'};
   }};
   window.lificSync={subscribe:fn=>{listeners.push(fn);return()=>{};},ensureProject:async()=>({status:'ready',issues:rows}),peekProject:()=>({status:'ready',issues:rows}),refreshProject:async()=>{},setActiveProject(){}};
  });await page.addScriptTag({content:fs.readFileSync(`${__dirname}/issue-list.js`,'utf8')});await page.waitForFunction(()=>document.querySelectorAll('[data-issue-id]').length===3);
  await t.test('live rows retain selection and focused checkbox',async()=>{
   await page.getByRole('checkbox',{name:'Select ENG-1',exact:true}).check();await page.getByRole('checkbox',{name:'Select ENG-1',exact:true}).focus();
   await page.evaluate(()=>{rows[0]=makeIssue(1,{title:'Changed <markup>',seq:2});listeners.forEach(fn=>fn());});
   assert.equal(await page.getByRole('checkbox',{name:'Select ENG-1',exact:true}).isChecked(),true);assert.equal(await page.evaluate(()=>document.activeElement.dataset.focusKey),'select:1');assert.match(await page.locator('[data-issue-id="1"]').innerText(),/Changed <markup>/);
  });
  await t.test('bulk partial failure selects failed rows and publishes failure text',async()=>{
   await page.getByRole('checkbox',{name:'Select ENG-2',exact:true}).check();await page.getByRole('combobox',{name:'Set status',exact:true}).selectOption('active');
   await page.waitForFunction(()=>document.querySelector('[data-issues-feedback]').textContent.includes('Permission denied'));assert.equal(await page.getByRole('checkbox',{name:'Select ENG-2',exact:true}).isChecked(),true);assert.equal(await page.getByRole('checkbox',{name:'Select ENG-1',exact:true}).isChecked(),false);
   assert.match(await page.locator('[data-issues-feedback]').innerText(),/1 updated; 1 failed/);
  });
  await t.test('search typing retains focus and selection through refreshed markup',async()=>{
   const search=page.getByRole('searchbox',{name:'Search issues'});await search.fill('Issue');await search.press('End');await search.press(' ');assert.equal(await search.inputValue(),'Issue ');assert.equal(await search.evaluate(el=>el===document.activeElement),true);
   await search.fill('');await page.waitForFunction(()=>document.querySelectorAll('[data-issue-id]').length===3);
  });
  await t.test('peek is a modal with escaped text and returns focus on Escape',async()=>{
   const trigger=page.getByRole('button',{name:'Peek ENG-1',exact:true});await trigger.click();await page.getByRole('dialog',{name:'Issue preview'}).waitFor();await page.waitForFunction(()=>document.querySelector('[data-issues-peek] pre'));assert.match(await page.locator('[data-issues-peek]').innerText(),/Body <script>not HTML<\/script>/);assert.equal(await page.locator('[data-issues-peek] script').count(),0);await page.keyboard.press('Escape');assert.equal(await trigger.evaluate(el=>el===document.activeElement),true);
  });
  await t.test('board displays all five status columns and URL round trips layout',async()=>{
   await page.getByRole('combobox',{name:'Layout',exact:true}).selectOption('board');assert.equal(await page.locator('[data-drop-status]').count(),5);assert.match(page.url(),/\/ENG\/board\?/);assert.equal(await page.locator('[data-drop-status="done"] [data-issue-id]').count(),1);
   await page.getByRole('combobox',{name:'Swimlanes',exact:true}).selectOption('module');assert.equal(await page.locator('[data-drop-status]').count(),10);
  });
  await t.test('account switches remove old Undo actions and feedback before reloading',async()=>{
   assert.equal(await page.locator('[data-action="undo"]').count(),1);
   await page.evaluate(()=>{localStorage.setItem('lific_token','new-account');dispatchEvent(new CustomEvent('lific:account-change'));});
   await page.waitForFunction(()=>document.querySelector('[data-topcoat-issue-list]').getAttribute('aria-busy')==='false');
   assert.equal(await page.locator('[data-action="undo"]').count(),0);assert.doesNotMatch(await page.locator('[data-issues-feedback]').innerText(),/Permission denied|1 updated/);
  });
  await t.test('role revocation removes selection and write controls immediately',async()=>{
   await page.evaluate(()=>{lificSession.state.role={role:'viewer',enforced:true,is_admin:false};dispatchEvent(new CustomEvent('lific:account-change'));});assert.equal(await page.locator('[data-select]').count(),0);assert.equal(await page.locator('[data-issues-bulk]').isVisible(),false);assert.equal(await page.getByRole('button',{name:'Create issue',exact:true}).count(),0);
  });
  await t.test('public scope does not expose saved views exports or private mutations',async()=>{
   await page.evaluate(()=>{lificSession.state.publicProject='ENG';dispatchEvent(new CustomEvent('lific:scope-change'));});await page.waitForFunction(()=>document.querySelector('[data-topcoat-issue-list]').getAttribute('aria-busy')==='false');assert.equal(await page.locator('[data-saved-view]').count(),0);assert.equal(await page.locator('[data-select]').count(),0);assert.equal(await page.getByRole('button',{name:'Export selected'}).count(),0);
  });
  assert.deepEqual(failures,[]);
 }finally{await browser.close();}
});

test('headless document navigation completes pending deletion and reload restores its remaining Undo window',{skip:!process.env.PLAYWRIGHT_EXECUTABLE_PATH},async t=>{
 const {chromium}=await import(path.resolve(__dirname,'../../../..','e2e/node_modules/playwright/index.mjs'));
 const browser=await chromium.launch({headless:true,executablePath:process.env.PLAYWRIGHT_EXECUTABLE_PATH}),page=await browser.newPage();page.setDefaultTimeout(5000);
 const source=fs.readFileSync(`${__dirname}/issue-list.js`,'utf8');let rows=[{id:1,project_id:7,identifier:'ENG-1',sequence:1,title:'Issue 1',preview:'',labels:[],module_id:null,priority:'medium',status:'todo',created_at:'2026-10-01',updated_at:'2026-10-01',sort_order:1,seq:1}],deletes=[],holdDelete=false,denyDelete=false,deleteStarted,releaseDelete;
 const html=`<!doctype html><html><head><script defer src="/issue-list.js"></script></head><body><a id="destination" href="/ENG/issues/ENG-2">Open another issue</a><section data-topcoat-issue-list data-project-identifier="ENG" data-layout="list"><div data-issues-controls></div><div data-issues-feedback></div><div data-issues-content></div><aside data-issues-bulk hidden></aside><dialog data-issues-peek aria-label="Issue preview"></dialog></section></body></html>`;
 try{
  await page.addInitScript(()=>{
   window.lificSession={state:{publicProject:null,projectId:7,role:{enforced:true,role:'maintainer',is_admin:false}},request:async(path,options={})=>{
    if(path==='/projects')return {ok:true,data:[{id:7,identifier:'ENG'}]};if(path.startsWith('/modules')||path.startsWith('/labels')||path.endsWith('/views'))return {ok:true,data:[]};if(path.endsWith('my-role'))return {ok:true,data:lificSession.state.role};
    const response=await fetch('/api'+path,options);return {ok:response.ok,status:response.status,data:await response.json()};
   }};
   window.fixtureReplicaRows=[];window.fixtureSyncListeners=[];const model=async()=>{fixtureReplicaRows=await(await fetch('/fixture-rows')).json();return {status:'ready',issues:fixtureReplicaRows};};window.lificSync={subscribe:listener=>{fixtureSyncListeners.push(listener);return()=>{};},ensureProject:model,peekProject:()=>({status:'ready',issues:fixtureReplicaRows}),refreshProject:async()=>{if(window.holdFixtureRefresh)await new Promise(resolve=>window.finishFixtureRefresh=resolve);await model();fixtureSyncListeners.forEach(listener=>listener());},setActiveProject(){}};
  });
  await page.route('http://localhost/**',async route=>{
   const pathName=new URL(route.request().url()).pathname;
   if(pathName==='/issue-list.js')return route.fulfill({contentType:'text/javascript',body:source});
   if(pathName==='/fixture-rows')return route.fulfill({contentType:'application/json',body:JSON.stringify(rows)});
   if(pathName.startsWith('/api/issues/')&&route.request().method()==='DELETE'){const id=Number(pathName.split('/').pop());deletes.push(id);if(denyDelete)return route.fulfill({status:403,contentType:'application/json',body:'{"error":"Permission denied"}'});rows=rows.filter(row=>row.id!==id);if(holdDelete){deleteStarted();await new Promise(resolve=>releaseDelete=resolve);}return route.fulfill({contentType:'application/json',body:'{"deleted":true}'});}
   return route.fulfill({contentType:'text/html',body:html});
  });
  await t.test('a full link navigation waits for deletion completion',async()=>{
   await page.goto('http://localhost/ENG/issues');await page.waitForFunction(()=>document.querySelector('[data-select]'));
   await page.evaluate(async()=>{const c=document.querySelector('[data-topcoat-issue-list]')._lificIssueList.controller;c.select(1);await c.scheduleDelete();});
   await page.locator('#destination').click();await page.waitForURL('**/ENG/issues/ENG-2');assert.deepEqual(deletes,[1]);
   assert.equal(await page.evaluate(()=>JSON.parse(sessionStorage.getItem('lific:issue-list:deferred-deletions')).length),0);
  });
  await t.test('a reload preserves the record and exposes Undo before the original deadline',async()=>{
   rows=[{id:2,project_id:7,identifier:'ENG-2',sequence:2,title:'Issue 2',preview:'',labels:[],module_id:null,priority:'medium',status:'todo',created_at:'2026-10-01',updated_at:'2026-10-01',sort_order:1,seq:1}];await page.goto('http://localhost/ENG/issues');await page.waitForFunction(()=>document.querySelector('[data-select]'));
   await page.evaluate(async()=>{const c=document.querySelector('[data-topcoat-issue-list]')._lificIssueList.controller;c.select(2);await c.scheduleDelete();});
   await page.reload();await page.getByRole('button',{name:'Undo deletion',exact:true}).waitFor();await page.waitForFunction(()=>document.querySelector('[data-topcoat-issue-list]').getAttribute('aria-busy')==='false');try{assert.equal(await page.locator('[data-issue-id]').count(),0);await page.evaluate(()=>fixtureSyncListeners.forEach(listener=>listener()));assert.equal(await page.locator('[data-issue-id]').count(),0);}finally{await page.getByRole('button',{name:'Undo deletion',exact:true}).click();}assert.deepEqual(deletes,[1]);assert.equal(await page.evaluate(()=>JSON.parse(sessionStorage.getItem('lific:issue-list:deferred-deletions')).length),0);assert.match(await page.locator('.tc-issues__deletion-feedback').innerText(),/Deletion undone/);
  });
  await t.test('account switches clear recovered Undo notices and cancel their stored operation',async()=>{
   await page.goto('http://localhost/ENG/issues');await page.waitForFunction(()=>document.querySelector('[data-select]'));
   await page.evaluate(async()=>{const c=document.querySelector('[data-topcoat-issue-list]')._lificIssueList.controller;c.select(2);await c.scheduleDelete();});await page.reload();await page.getByRole('button',{name:'Undo deletion',exact:true}).waitFor();
   await page.evaluate(()=>{localStorage.setItem('lific_token','different-owner');dispatchEvent(new CustomEvent('lific:account-change'));});assert.equal(await page.locator('.tc-issues__deletion-feedback').count(),0);assert.equal(await page.evaluate(()=>JSON.parse(sessionStorage.getItem('lific:issue-list:deferred-deletions')).length),0);assert.deepEqual(deletes,[1]);
  });
  await t.test('shell navigation intents also wait for pending deletion before a full load',async()=>{
   rows=[{id:3,project_id:7,identifier:'ENG-3',sequence:3,title:'Issue 3',preview:'',labels:[],module_id:null,priority:'medium',status:'todo',created_at:'2026-10-01',updated_at:'2026-10-01',sort_order:1,seq:1}];await page.goto('http://localhost/ENG/issues');await page.waitForFunction(()=>document.querySelector('[data-select]'));
   await page.evaluate(async()=>{window.addEventListener('lific:navigate',event=>location.assign(event.detail.href));const c=document.querySelector('[data-topcoat-issue-list]')._lificIssueList.controller;c.select(3);await c.scheduleDelete();dispatchEvent(new CustomEvent('lific:navigate',{detail:{href:'/ENG/issues/ENG-4',history:'push'}}));});await page.waitForURL('**/ENG/issues/ENG-4');assert.deepEqual(deletes,[1,3]);
  });
  await t.test('resumed Undo disappears when DELETE starts and a stale button cannot announce success',async()=>{
   rows=[{id:4,project_id:7,identifier:'ENG-4',sequence:4,title:'Issue 4',preview:'',labels:[],module_id:null,priority:'medium',status:'todo',created_at:'2026-10-01',updated_at:'2026-10-01',sort_order:1,seq:1}];await page.goto('http://localhost/ENG/issues');await page.waitForFunction(()=>document.querySelector('[data-select]'));
   await page.evaluate(async()=>{const c=document.querySelector('[data-topcoat-issue-list]')._lificIssueList.controller;c.select(4);await c.scheduleDelete();const key='lific:issue-list:deferred-deletions',records=JSON.parse(sessionStorage.getItem(key));records[0].deadline=Date.now()+1500;sessionStorage.setItem(key,JSON.stringify(records));});
   holdDelete=true;const started=new Promise(resolve=>deleteStarted=resolve);await page.reload();await page.getByRole('button',{name:'Undo deletion',exact:true}).waitFor();await page.evaluate(()=>window.staleUndo=document.querySelector('.tc-issues__deletion-feedback button:last-child'));
   await started;
   try{assert.equal(await page.getByRole('button',{name:'Undo deletion',exact:true}).count(),0);await page.evaluate(()=>staleUndo.click());assert.doesNotMatch(await page.locator('.tc-issues__deletion-feedback').innerText(),/Deletion undone/);assert.equal(await page.evaluate(()=>JSON.parse(sessionStorage.getItem('lific:issue-list:deferred-deletions')).length),1);}finally{holdDelete=false;releaseDelete();}
   await page.waitForFunction(()=>document.querySelector('.tc-issues__deletion-feedback').textContent.includes('1 deleted; 0 failed'));
  });
  await t.test('failed recovered deletions restore mounted rows when their record is removed',async()=>{
   denyDelete=true;rows=[{id:5,project_id:7,identifier:'ENG-5',sequence:5,title:'Issue 5',preview:'',labels:[],module_id:null,priority:'medium',status:'todo',created_at:'2026-10-01',updated_at:'2026-10-01',sort_order:1,seq:1}];await page.goto('http://localhost/ENG/issues');await page.waitForFunction(()=>document.querySelector('[data-select]'));
   await page.evaluate(async()=>{const c=document.querySelector('[data-topcoat-issue-list]')._lificIssueList.controller;c.select(5);await c.scheduleDelete();const key='lific:issue-list:deferred-deletions',records=JSON.parse(sessionStorage.getItem(key));records[0].deadline=Date.now()+1500;sessionStorage.setItem(key,JSON.stringify(records));});await page.reload();await page.getByRole('button',{name:'Undo deletion',exact:true}).waitFor();assert.equal(await page.locator('[data-issue-id]').count(),0);
   await page.waitForFunction(()=>document.querySelector('.tc-issues__deletion-feedback').textContent.includes('0 deleted; 1 failed'));await page.locator('[data-issue-id="5"]').waitFor();assert.match(await page.locator('.tc-issues__deletion-feedback').innerText(),/ENG-5/);assert.equal(await page.evaluate(()=>JSON.parse(sessionStorage.getItem('lific:issue-list:deferred-deletions')).length),0);
  });
  await t.test('account changes during replay refresh cannot republish the previous account failure',async()=>{
   rows=[{id:6,project_id:7,identifier:'ENG-6',sequence:6,title:'Issue 6',preview:'',labels:[],module_id:null,priority:'medium',status:'todo',created_at:'2026-10-01',updated_at:'2026-10-01',sort_order:1,seq:1}];await page.goto('http://localhost/ENG/issues');await page.waitForFunction(()=>document.querySelector('[data-select]'));
   await page.evaluate(async()=>{const c=document.querySelector('[data-topcoat-issue-list]')._lificIssueList.controller;c.select(6);await c.scheduleDelete();const key='lific:issue-list:deferred-deletions',records=JSON.parse(sessionStorage.getItem(key));records[0].deadline=Date.now()+1500;sessionStorage.setItem(key,JSON.stringify(records));});await page.reload();await page.getByRole('button',{name:'Undo deletion',exact:true}).waitFor();await page.evaluate(()=>window.holdFixtureRefresh=true);await page.waitForFunction(()=>typeof window.finishFixtureRefresh==='function');
   await page.evaluate(()=>{localStorage.setItem('lific_token','third-owner');dispatchEvent(new CustomEvent('lific:account-change'));finishFixtureRefresh();});await page.waitForFunction(()=>JSON.parse(sessionStorage.getItem('lific:issue-list:deferred-deletions')).length===0);assert.equal(await page.locator('.tc-issues__deletion-feedback').count(),0);
  });
 }finally{await browser.close();}
});

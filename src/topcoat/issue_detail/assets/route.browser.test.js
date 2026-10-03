const {test} = require('node:test');
const assert = require('node:assert/strict');
const fs = require('node:fs');
const path = require('node:path');

test('headless issue detail resolves, edits scalar fields and saves markdown through one sequence',
  {skip: !process.env.PLAYWRIGHT_EXECUTABLE_PATH}, async t => {
    const {chromium} = await import(path.resolve(__dirname, '../../../../e2e/node_modules/playwright/index.mjs'));
    const browser = await chromium.launch({headless:true,executablePath:process.env.PLAYWRIGHT_EXECUTABLE_PATH});
    try {
      const page = await browser.newPage(); page.setDefaultTimeout(4000);
      await page.setContent(`<section data-topcoat-issue-detail data-project-identifier="ENG" data-issue-identifier="ENG-7" data-issue-scope="private" aria-busy="true">
        <div data-detail-loading><p role="status"></p></div><div data-detail-error hidden></div>
        <div data-detail-content hidden><header><a data-detail-back></a><p data-detail-identifier></p><h1 data-detail-title></h1></header>
          <section data-issue-fields aria-label="Issue fields"><label>Title<input data-field="title"></label>
            <label>Status<select data-field="status"><option value="backlog">Backlog</option><option value="active">Active</option></select></label>
            <label>Priority<select data-field="priority"><option value="none">None</option><option value="high">High</option></select></label>
            <label>Module<select data-field="module_id"><option value="">None</option></select></label>
            <label>Labels<input data-field="labels"></label><span data-field-metadata hidden><span data-field-created></span><span data-field-updated></span></span>
            <p data-fields-status role="status"></p></section>
          <section data-topcoat-issue-editor hidden><button data-editor-edit>Edit</button><button data-editor-preview-toggle>Preview</button><button data-editor-save>Save</button>
            <p data-editor-status role="status"></p><p data-editor-error hidden></p><section data-editor-conflict hidden><p data-editor-conflict-message></p><pre data-editor-server-value></pre></section>
            <textarea data-editor-input aria-label="Issue description"></textarea><article data-editor-preview hidden></article></section>
        </div></section>`);
      await page.evaluate(() => {
        window.issue = {id:7,project_id:3,seq:4,sequence:7,identifier:'ENG-7',title:'First title',description:'Saved body',status:'backlog',priority:'none',module_id:null,labels:[],created_at:'2026-10-01',updated_at:'2026-10-02'};
        window.calls=[];window.roleDenied=false;window.holdResolve=false; window.lificSession={state:{user:{id:1},publicProject:null,loading:false},request:async(path,options={})=>{
          calls.push({path,options});
          if(path==='/issues/resolve/ENG-7')return window.holdResolve
            ? await new Promise(resolve=>{window.releaseResolve=()=>resolve({ok:true,data:issue});}) : {ok:true,data:issue};
          if(path==='/projects/3/my-role')return window.roleDenied
            ? {ok:false,status:403,error:'Membership was revoked'} : {ok:true,data:{role:'maintainer',enforced:true,is_admin:false}};
          if(path==='/modules?project_id=3')return {ok:true,data:[{id:2,name:'Engine'}]};
          if(path==='/labels?project_id=3')return {ok:true,data:[{name:'bug'}]};
          if(path==='/issues/7'&&options.method==='PUT'){
            if(window.holdConflict)return await new Promise(resolve=>{window.releaseConflict=()=>resolve({ok:false,status:409,error:'Issue changed elsewhere',current:{...issue,seq:issue.seq+1,description:'External body'}});});
            const patch=JSON.parse(options.body);issue={...issue,...patch,seq:issue.seq+1,updated_at:'2026-10-03'};return {ok:true,data:issue};
          }
          return {ok:false,error:`Unexpected ${path}`};
        }};
      });
      for (const file of ['fields.js','../editor/assets/editor.js','route.js']) {
        await page.addScriptTag({content:fs.readFileSync(path.join(__dirname,file),'utf8')});
      }
      await page.waitForFunction(() => !document.querySelector('[data-detail-content]').hidden || !document.querySelector('[data-detail-error]').hidden);
      assert.equal(await page.locator('[data-detail-error]').textContent(),'');
      await page.locator('[data-detail-content]').waitFor({state:'visible'});
      await t.test('route resolves the requested issue and hydrates controls with server metadata', async () => {
        assert.equal(await page.locator('[data-detail-identifier]').textContent(),'ENG-7');
        assert.equal(await page.locator('[data-detail-title]').textContent(),'First title');
        assert.equal(await page.locator('[data-field="module_id"] option').count(),2);
        assert.equal(await page.locator('[data-editor-input]').inputValue(),'Saved body');
        assert.equal(await page.locator('[data-field="title"]').isDisabled(),false);
      });
      await t.test('scalar and description writes serialize and advance expected_seq', async () => {
        const title = page.locator('[data-field="title"]'); await title.fill('Second title'); await title.press('Enter');
        await page.waitForFunction(() => issue.title==='Second title');
        await page.locator('[data-editor-edit]').click(); await page.locator('[data-editor-input]').fill('Changed **body**'); await page.locator('[data-editor-save]').click();
        await page.waitForFunction(() => issue.description==='Changed **body**');
        const writes = await page.evaluate(() => calls.filter(call=>call.options.method==='PUT').map(call=>JSON.parse(call.options.body)));
        assert.equal(writes.length,2); assert.equal(writes[0].expected_seq,4); assert.equal(writes[1].expected_seq,5);
        assert.equal(writes[0].title,'Second title'); assert.equal(writes[1].description,'Changed **body**');
      });
      await t.test('realtime refresh applies a clean server description without remounting the editor', async () => {
        await page.evaluate(()=>{issue={...issue,seq:issue.seq+1,description:'Remote update'};dispatchEvent(new CustomEvent('lific:realtime',{detail:{type:'issue.updated',project_id:3,issue_id:7,seq:issue.seq}}));});
        await page.waitForFunction(()=>document.querySelector('[data-editor-input]').value==='Remote update');
        assert.equal(await page.locator('[data-detail-title]').textContent(),'Second title');
      });
      await t.test('refresh preserves a scalar draft started while the issue read is in flight', async () => {
        await page.evaluate(()=>{window.holdResolve=true;dispatchEvent(new CustomEvent('lific:realtime',{detail:{type:'issue.updated',project_id:3,issue_id:7}}));});
        await page.waitForFunction(()=>typeof window.releaseResolve==='function');
        const title=page.locator('[data-field="title"]');await title.focus();await title.fill('Unsaved title draft');
        await page.evaluate(()=>window.releaseResolve());
        await page.waitForTimeout(80);
        assert.equal(await title.inputValue(),'Unsaved title draft');
        await page.evaluate(()=>{window.holdResolve=false;});
      });
      await t.test('permission revocation disables controls and settles pending description saves', async () => {
        await page.evaluate(()=>{window.roleDenied=true;dispatchEvent(new CustomEvent('lific:account-change'));});
        await page.waitForFunction(()=>document.querySelector('[data-field="title"]').disabled);
        const denied=await page.evaluate(async()=>{
          const controller=window.lificIssueDetail;let failure=null;
          addEventListener('lific:issue-detail-error',event=>{if(event.detail.edit_revision===78)failure=event.detail;},{once:false});
          const result=await controller.accept({route:controller.route,action:{type:'save_description',description:'Draft',edit_revision:78}});
          return {status:result.status,error:failure?.error,disabled:document.querySelector('[data-editor-input]').disabled};
        });
        assert.equal(denied.status,'denied');assert.match(denied.error,/permission/);assert.equal(denied.disabled,true);
        await page.evaluate(()=>{window.roleDenied=false;dispatchEvent(new CustomEvent('lific:account-change'));});
        await page.waitForFunction(()=>!document.querySelector('[data-field="title"]').disabled);
      });
      await t.test('queued description draft is surfaced as a conflict after another field conflicts', async () => {
        await page.evaluate(async()=>{
          const controller=window.lificIssueDetail;
          window.holdConflict=true;window.routeConflict=null;
          window.routeConflicts=[];
          addEventListener('lific:issue-detail-conflict',event=>{window.routeConflicts.push(event.detail);if(event.detail.edit_revision===77)window.routeConflict=event.detail;},{once:false});
          const first=controller.accept({route:controller.route,action:{type:'set_scalar',field:'title',value:'Local title'}});
          while(typeof window.releaseConflict!=='function')await new Promise(resolve=>setTimeout(resolve,0));
          const second=controller.accept({route:controller.route,action:{type:'save_description',description:'Local draft',edit_revision:77}});
          window.releaseConflict();
          window.routeStatuses=await Promise.all([first,second]);
        });
        const evidence=await page.evaluate(()=>({writes:calls.filter(call=>call.options.method==='PUT').length,
          conflict:window.routeConflict?.edit_revision,current:window.routeConflict?.current_description,
          issue:window.lificIssueDetail.issue.description,conflicts:window.routeConflicts,statuses:window.routeStatuses}));
        assert.equal(evidence.conflict,77,JSON.stringify(evidence));
        assert.equal(evidence.writes,3);
        assert.equal(evidence.current,'External body');
        assert.equal(evidence.issue,'External body');
      });
    } finally {await browser.close();}
  });

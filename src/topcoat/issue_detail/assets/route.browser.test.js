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
            <label>Due date<input data-field="target_date" type="date"></label>
            <fieldset data-label-field><legend>Labels</legend><div data-label-options></div>
              <label>New label<input data-new-label-name></label><input data-new-label-color type="color" value="#6b7280"><button type="button" data-create-label>Create label</button></fieldset>
            <span data-field-metadata hidden><span data-field-created></span><span data-field-updated></span></span>
            <p data-fields-status role="status"></p></section>
          <section data-topcoat-issue-editor hidden><button data-editor-edit>Edit</button><button data-editor-preview-toggle>Preview</button><button data-editor-save>Save</button>
            <p data-editor-status role="status"></p><p data-editor-error hidden></p><section data-editor-conflict hidden><p data-editor-conflict-message></p><pre data-editor-server-value></pre></section>
            <textarea data-editor-input aria-label="Issue description"></textarea><article data-editor-preview hidden></article></section>
        </div></section>`);
      await page.evaluate(() => {
        window.issue = {id:7,project_id:3,seq:4,sequence:7,identifier:'ENG-7',title:'First title',description:'Saved body',status:'backlog',priority:'none',module_id:null,labels:[],created_at:'2026-10-01',updated_at:'2026-10-02'};
        window.calls=[];window.roleDenied=false;window.holdResolve=false;window.holdDescription=false;window.holdLabel=false;window.lificSession={state:{user:{id:1},publicProject:null,loading:false},request:async(path,options={})=>{
          calls.push({path,options});
          if(path==='/issues/resolve/ENG-7')return window.holdResolve
            ? await new Promise(resolve=>{window.releaseResolve=()=>resolve({ok:true,data:issue});}) : {ok:true,data:issue};
          if(path==='/projects/3/my-role')return window.roleDenied
            ? {ok:false,status:403,error:'Membership was revoked'} : {ok:true,data:{role:'maintainer',enforced:true,is_admin:false}};
          if(path==='/modules?project_id=3')return {ok:true,data:[{id:2,name:'Engine'}]};
          if(path==='/labels?project_id=3')return {ok:true,data:[{name:'bug'},{name:'API, clients'}]};
          if(path==='/labels'&&options.method==='POST')return window.holdLabel
            ? await new Promise(resolve=>{window.releaseLabel=()=>resolve({ok:true,data:{name:JSON.parse(options.body).name,color:JSON.parse(options.body).color}});})
            : {ok:true,data:{name:JSON.parse(options.body).name,color:JSON.parse(options.body).color}};
          if(path==='/issues/7'&&options.method==='PUT'){
            if(window.holdConflict)return await new Promise(resolve=>{window.releaseConflict=()=>resolve({ok:false,status:409,error:'Issue changed elsewhere',current:{...issue,seq:issue.seq+1,description:'External body'}});});
            if(window.holdDescription&&Object.hasOwn(JSON.parse(options.body),'description'))return await new Promise(resolve=>{window.releaseDescription=()=>{const patch=JSON.parse(options.body);issue={...issue,...patch,seq:issue.seq+1};resolve({ok:true,data:issue});};});
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
        assert.equal(await page.locator('[data-label-options] input').count(),2);
        assert.equal(await page.locator('[data-field="target_date"]').inputValue(),'');
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
      await t.test('date edits and exact-name labels preserve labels containing commas', async () => {
        const dueDate=page.locator('[data-field="target_date"]'); await dueDate.fill('2026-10-15');
        await page.waitForFunction(()=>issue.target_date==='2026-10-15');
        await page.locator('[data-label-options] input[type="checkbox"][value="API, clients"]').check();
        await page.waitForFunction(()=>issue.labels.includes('API, clients'));
        await page.locator('[data-new-label-name]').fill('triage, urgent');
        await page.locator('[data-create-label]').click();
        await page.waitForFunction(()=>issue.labels.includes('triage, urgent'));
        assert.deepEqual(await page.evaluate(()=>issue.labels),['API, clients','triage, urgent']);
        assert.equal(await page.evaluate(()=>calls.filter(call=>call.path==='/labels'&&call.options.method==='POST').length),1);
      });
      await t.test('an older local description acknowledgement keeps a newer draft autosaving', async () => {
        await page.evaluate(()=>{window.holdDescription=true;window.editorConflicts=[];addEventListener('lific:issue-detail-conflict',event=>editorConflicts.push(event.detail));
          window.lificIssueDetail.editor.queue.edit('First draft');});
        await page.waitForFunction(()=>typeof window.releaseDescription==='function');
        await page.evaluate(()=>window.lificIssueDetail.editor.queue.edit('Newer draft'));
        await page.evaluate(()=>{window.holdDescription=false;window.releaseDescription();});
        await page.waitForFunction(()=>issue.description==='Newer draft');
        assert.equal(await page.evaluate(()=>window.editorConflicts.length),0);
        assert.equal(await page.evaluate(()=>window.lificIssueDetail.editor.queue.state().dirty),false);
        await page.evaluate(()=>{window.holdDescription=true;window.releaseDescription=null;window.descriptionWritesBefore=calls.filter(call=>
          call.options.method==='PUT'&&Object.hasOwn(JSON.parse(call.options.body),'description')).length;
          window.lificIssueDetail.editor.queue.edit('In flight draft');});
        await page.waitForFunction(()=>typeof window.releaseDescription==='function');
        await page.evaluate(()=>window.lificIssueDetail.editor.queue.edit('Newer draft'));
        assert.equal(await page.evaluate(()=>window.lificIssueDetail.editor.queue.state().dirty),false);
        await page.evaluate(()=>{window.holdDescription=false;window.releaseDescription();});
        await page.waitForFunction(()=>calls.filter(call=>call.options.method==='PUT'&&
          Object.hasOwn(JSON.parse(call.options.body),'description')).length===window.descriptionWritesBefore+2);
        await page.waitForFunction(()=>issue.description==='Newer draft'&&
          !window.lificIssueDetail.editor.queue.state().dirty);
        assert.equal(await page.evaluate(()=>window.editorConflicts.length),0);
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
        await page.evaluate(()=>{
          const editor=window.lificIssueDetail.editor;
          editor.queue.edit('Local draft');
          document.querySelector('[data-editor-input]').value='Local draft';
        });
        await page.evaluate(async()=>{
          const controller=window.lificIssueDetail;
          window.conflictStartWrites=calls.filter(call=>call.options.method==='PUT').length;
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
        assert.equal(evidence.writes,await page.evaluate(()=>window.conflictStartWrites+1));
        assert.equal(evidence.current,'External body');
        assert.equal(evidence.issue,'External body');
        assert.equal(await page.locator('[data-editor-input]').inputValue(),'Local draft');
        assert.equal(await page.locator('[data-editor-conflict]').isVisible(),true);
      });
      await t.test('a label response from the previous account cannot mutate the replacement issue route', async () => {
        const oldGeneration = await page.evaluate(() => {
          window.holdLabel = true;
          document.querySelector('[data-new-label-name]').value = 'old account label';
          document.querySelector('[data-create-label]').click();
          return window.lificIssueDetail.route.generation;
        });
        await page.waitForFunction(() => typeof window.releaseLabel === 'function');
        await page.evaluate(() => {
          window.lificSession.state.user = {id:2};
          dispatchEvent(new CustomEvent('lific:account-change'));
        });
        await page.waitForFunction(generation => window.lificIssueDetail.route.generation > generation, oldGeneration);
        await page.evaluate(() => {window.holdLabel=false;window.releaseLabel();});
        await page.waitForTimeout(0);
        assert.equal(await page.evaluate(() => window.lificIssueDetail.labels.some(label => label.name === 'old account label')),false);
      });
    } finally {await browser.close();}
  });

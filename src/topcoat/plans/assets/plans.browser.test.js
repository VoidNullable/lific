const {test}=require('node:test');
const assert=require('node:assert/strict');
const fs=require('node:fs');
const path=require('node:path');

test('headless plans preserve nested edits, linked completion, server progress, and scoped requests',
  {skip:!process.env.PLAYWRIGHT_EXECUTABLE_PATH},async t=>{
    const {chromium}=await import(path.resolve(__dirname,'../../../../e2e/node_modules/playwright/index.mjs'));
    const browser=await chromium.launch({headless:true,executablePath:process.env.PLAYWRIGHT_EXECUTABLE_PATH});
    const page=await browser.newPage();page.setDefaultTimeout(5000);const errors=[];page.on('pageerror',error=>errors.push(error.message));
    async function mount({mode='detail',viewer=false}={}){
      await page.route('http://planning.test/**',route=>route.fulfill({contentType:'text/html',body:`<!doctype html><section data-topcoat-plans="${mode}" data-project-identifier="ENG" data-plan-id="10"><p data-plans-status role="status"></p><div data-plans-error role="alert" hidden></div><div data-plans-content></div></section>`}));
      await page.goto(`http://planning.test/ENG/plans${mode==='detail'?'/10':''}`);
      await page.evaluate(({viewer})=>{
        window.calls=[];window.destinations=[];window.failure=null;window.role=viewer?'viewer':'maintainer';window.holdWrite=false;
        window.issue={id:7,project_id:3,identifier:'ENG-7',title:'Engine',status:'active'};
        const step=(id,title,parent=null,children=[])=>({id,plan_id:10,parent_step_id:parent,position:0,title,description:id===2?'**Child markdown**':'',done:false,issue_id:id===2?7:null,
          issue_identifier:id===2?'ENG-7':undefined,issue_status:id===2?'active':undefined,children});
        window.plan={id:10,project_id:3,identifier:'ENG-PLAN-1',title:'Deployment',status:'active',issue_id:null,steps:[step(1,'Root',null,[step(2,'Child',1,[step(3,'Grandchild',2)])]),{...step(4,'Second root'),position:1}],
          step_count:10,done_count:7,created_at:'yesterday',updated_at:'today'};
        window.flatten=steps=>steps.flatMap(row=>[row,...flatten(row.children)]);
        window.clone=value=>JSON.parse(JSON.stringify(value));window.nextStep=5;
        window.lificSync={setActiveProject(){},subscribe(){return ()=>{};}};
        window.lificSession={state:{user:{id:1},publicProject:null,loading:false},request:async(url,options={})=>{
          calls.push({url,...options});if(failure===url){failure=null;return {ok:false,status:503,error:'Temporary read failure'};}
          const success=data=>({ok:true,data:clone(data)}),body=options.body?JSON.parse(options.body):{};
          if(url==='/projects')return success([{id:3,identifier:'ENG',name:'Engineering'}]);
          if(url==='/projects/3/my-role')return success({role,enforced:true,is_admin:false});
          if(url.startsWith('/plans?')){const params=new URLSearchParams(url.split('?')[1]);const before=Number(params.get('before_id')||202);return success(Array.from({length:201},(_,index)=>({...plan,id:201-index,status:'active',updated_at:String(201-index)})).filter(row=>row.id<before).slice(0,200));}
          if(url==='/plans'&&options.method==='POST')return success({...plan,id:20,title:body.title});
          if(url==='/plans/10/activity?limit=100')return success({items:[{actor_display_name:'Alex',action:'create',new_value:'Deployment',ts:'today'}],has_more:false});
          if(url.startsWith('/search?'))return success([{result_type:'issue',...issue},{result_type:'issue',id:99,project_id:4,identifier:'OTHER-99',title:'Foreign result'}]);
          if(url==='/issues/resolve/ENG-7')return success(issue);
          if(url.startsWith('/issues/resolve/'))return {ok:false,status:404,error:'Issue not found'};
          if(url==='/plans/10'&&!options.method){const snapshot=clone(plan);return window.holdRead?new Promise(resolve=>{window.releaseRead=()=>resolve(success(snapshot));}):success(snapshot);}
          if(url==='/plans/10'&&options.method==='PUT'){
            Object.assign(plan,body);if(Object.hasOwn(body,'issue_id'))plan.anchor_identifier=body.issue_id===null?undefined:issue.identifier;
            return success(plan);
          }
          if(url==='/plans/10'&&options.method==='DELETE')return success({deleted:true});
          if(url==='/plans/10/steps'&&options.method==='POST'){
            const parent=body.parent_step_id==null?null:flatten(plan.steps).find(row=>row.id===body.parent_step_id),siblings=parent?parent.children:plan.steps;
            siblings.push({...step(nextStep++,body.title,parent?.id??null),position:siblings.length});
            plan.step_count=flatten(plan.steps).length;return success(plan);
          }
          const match=url.match(/^\/plans\/10\/steps\/(\d+)$/);
          if(match){
            const id=Number(match[1]),target=flatten(plan.steps).find(row=>row.id===id);
            const apply=()=>{
              let effect;
              const remove=rows=>{const index=rows.findIndex(row=>row.id===id);if(index>=0){rows.splice(index,1);return;}for(const row of rows)remove(row.children);};
              if(options.method==='DELETE'){remove(plan.steps);plan.step_count=flatten(plan.steps).length;return success(plan);}
              Object.assign(target,Object.fromEntries(Object.entries(body).filter(([key])=>['title','description','done','issue_id'].includes(key))));
              if(Object.hasOwn(body,'issue_id')){target.issue_identifier=body.issue_id===null?undefined:issue.identifier;target.issue_status=body.issue_id===null?undefined:issue.status;}
              if(body.done===true&&target.issue_id){issue.status='done';target.issue_status='done';effect={issue_status_changed:true,issue_identifier:issue.identifier,issue_new_status:'done'};}
              if(body.move_to_root||Object.hasOwn(body,'move_parent_step_id')||Object.hasOwn(body,'move_position')){
                remove(plan.steps);target.parent_step_id=body.move_to_root?null:body.move_parent_step_id??target.parent_step_id;
                const parent=target.parent_step_id===null?null:flatten(plan.steps).find(row=>row.id===target.parent_step_id),siblings=parent?parent.children:plan.steps;
                siblings.splice(body.move_position??siblings.length,0,target);siblings.forEach((row,index)=>{row.position=index;});
              }
              plan.done_count=flatten(plan.steps).filter(row=>row.done).length;plan.step_count=flatten(plan.steps).length;
              return success({plan,effect});
            };
            if(holdWrite)return new Promise(resolve=>{window.releaseWrite=()=>resolve(apply());});
            return apply();
          }
          return {ok:false,status:400,error:`Unexpected ${url}`};
        }};
      },{viewer});
      for(const file of ['../../issue_detail/editor/assets/editor.js','picker.js','plans.js'])await page.addScriptTag({content:fs.readFileSync(path.join(__dirname,file),'utf8')});
      await page.waitForFunction(()=>document.querySelector('[data-topcoat-plans]').getAttribute('aria-busy')==='false');
      await page.evaluate(()=>{document.querySelector('[data-topcoat-plans]')._plans.navigate=path=>destinations.push(path);});
    }
    try{
      await t.test('detail displays server progress, markdown, linked status, and activity',async()=>{
        await mount();assert.equal(await page.locator('[data-plan-progress]').textContent(),'7/10 steps');
        assert.equal(await page.locator('[data-plan-step]').count(),4);
        assert.equal(await page.locator('[data-step-description="2"] strong').textContent(),'Child markdown');
        assert.match(await page.locator('[data-plan-activity]').textContent(),/Alex create.*Deployment/);
        await page.locator('[data-plan-step="2"]>div a').click({modifiers:['Shift']});
        await page.getByRole('heading',{name:'ENG-7 · Engine'}).waitFor();
        assert.equal(await page.getByRole('link',{name:'Open issue',exact:true}).getAttribute('href'),'/ENG/issues/ENG-7');
        await page.getByRole('button',{name:'Close peek'}).click();
        await page.evaluate(()=>{dispatchEvent(new PageTransitionEvent('pagehide',{persisted:true}));plan.title='Restored plan';dispatchEvent(new PageTransitionEvent('pageshow',{persisted:true}));});
        await page.waitForFunction(()=>document.querySelector('[data-plan-title]').value==='Restored plan');
        assert.equal(await page.evaluate(()=>document.querySelector('[data-topcoat-plans]')._plans.disposed),false);
      });
      await t.test('step completion uses server effect; unchecking preserves the done issue; reopen refresh clears completion',async()=>{
        await mount();await page.locator('[data-step-done="2"]').check();
        await page.waitForFunction(()=>document.querySelector('[data-plan-progress]').textContent==='1/4 steps');
        assert.equal(await page.evaluate(()=>issue.status),'done');
        assert.match(await page.locator('[data-plans-status]').textContent(),/ENG-7 marked done/);
        await page.locator('[data-step-done="2"]').uncheck();await page.waitForFunction(()=>!plan.steps[0].children[0].done);
        assert.equal(await page.evaluate(()=>issue.status),'done');
        await page.locator('[data-step-done="2"]').check();await page.waitForFunction(()=>plan.steps[0].children[0].done);
        await page.evaluate(()=>{issue.status='active';const step=flatten(plan.steps).find(row=>row.id===2);step.done=false;step.issue_status='active';step.reopened_via_issue_at='now';plan.done_count=0;document.activeElement.blur();dispatchEvent(new CustomEvent('lific:realtime',{detail:{type:'issue.updated',project_id:3,issue_id:7}}));});
        await page.waitForFunction(()=>!document.querySelector('[data-step-done="2"]').checked);
        assert.match(await page.locator('[data-plan-step="2"]').textContent(),/ENG-7 reopened/);
      });
      await t.test('foreign linked issue and anchor events refresh authoritative completion and provenance',async()=>{
        await mount();await page.evaluate(()=>{const step=flatten(plan.steps).find(row=>row.id===2);Object.assign(step,{issue_id:99,issue_identifier:'OTHER-99',issue_status:'done',done:true});plan.issue_id=100;plan.anchor_identifier='OTHER-100';});
        await page.evaluate(()=>document.querySelector('[data-topcoat-plans]')._plans.load());
        await page.evaluate(()=>{const step=flatten(plan.steps).find(row=>row.id===2);Object.assign(step,{done:false,issue_status:'active',reopened_via_issue_at:'now'});plan.done_count=0;dispatchEvent(new CustomEvent('lific:realtime',{detail:{type:'issue.updated',project_id:4,issue_id:99}}));});
        await page.waitForFunction(()=>!document.querySelector('[data-step-done="2"]').checked);
        assert.match(await page.locator('[data-plan-step="2"]').textContent(),/OTHER-99 reopened/);
        await page.evaluate(()=>{plan.anchor_identifier='OTHER-101';dispatchEvent(new CustomEvent('lific:realtime',{detail:{type:'issue.updated',project_id:4,issue_id:100}}));});
        await page.getByRole('link',{name:'OTHER-101',exact:true}).waitFor();
        const reads=await page.evaluate(()=>calls.filter(call=>call.url==='/plans/10'&&!call.method).length);
        await page.evaluate(()=>dispatchEvent(new CustomEvent('lific:realtime',{detail:{type:'issue.updated',project_id:4,issue_id:101}})));await page.waitForTimeout(350);
        assert.equal(await page.evaluate(()=>calls.filter(call=>call.url==='/plans/10'&&!call.method).length),reads);
      });
      await t.test('nested create, title/description edits, reparent/order, and deletion send existing contracts',async()=>{
        await mount();await page.locator('[data-step-child="4"]').click();await page.getByLabel('Step title',{exact:true}).fill('Added child');await page.getByRole('button',{name:'Save',exact:true}).click();
        await page.waitForFunction(()=>!!document.querySelector('[data-plan-step="5"]'));
        const title=page.locator('[data-step-title="5"]');await title.fill('Updated child');await title.press('Enter');await page.waitForFunction(()=>flatten(plan.steps).find(row=>row.id===5).title==='Updated child');
        await page.locator('[data-step-edit-description="5"]').click();await page.getByLabel('Description',{exact:true}).fill('New **description**');await page.getByRole('button',{name:'Save',exact:true}).click();
        await page.waitForFunction(()=>document.querySelector('[data-step-description="5"] strong')?.textContent==='description');
        await page.locator('[data-step-move="1"]').click();assert.equal(await page.getByLabel('Parent').locator('option[value="2"]').count(),0);assert.equal(await page.getByLabel('Parent').locator('option[value="3"]').count(),0);await page.getByRole('button',{name:'Cancel',exact:true}).click();
        await page.locator('[data-step-move="5"]').click();await page.getByLabel('Parent').selectOption('1');await page.getByLabel('Position').fill('0');await page.getByRole('button',{name:'Save',exact:true}).click();
        await page.waitForFunction(()=>plan.steps[0].children[0].id===5);
        assert.deepEqual(await page.evaluate(()=>JSON.parse(calls.findLast(call=>call.url==='/plans/10/steps/5'&&JSON.parse(call.body).move_parent_step_id).body)),{move_parent_step_id:1,move_position:0});
        await page.locator('[data-step-order="5"][data-delta="1"]').click();await page.waitForFunction(()=>plan.steps[0].children[1].id===5);
        page.once('dialog',dialog=>dialog.accept());await page.locator('[data-step-delete="5"]').click();await page.waitForFunction(()=>!document.querySelector('[data-plan-step="5"]'));
      });
      await t.test('issue picker links and detaches step issues and sets/clears the plan anchor',async()=>{
        await mount();await page.locator('[data-step-link="4"]').click();await page.getByLabel('Search issues').fill('7');
        await page.getByRole('option').filter({hasText:'ENG-7'}).waitFor();assert.equal(await page.getByRole('option').filter({hasText:'OTHER-99'}).count(),0);
        await page.getByLabel('Search issues').press('Enter');await page.waitForFunction(()=>flatten(plan.steps).find(row=>row.id===4).issue_id===7);
        await page.locator('[data-step-detach="4"]').click();await page.waitForFunction(()=>flatten(plan.steps).find(row=>row.id===4).issue_id===null);
        await page.locator('[data-plan-anchor]').click();await page.getByLabel('Search issues').fill('Engine');await page.getByRole('option').filter({hasText:'ENG-7'}).click();await page.waitForFunction(()=>plan.anchor_identifier==='ENG-7');
        await page.locator('[data-plan-anchor]').click();await page.getByRole('button',{name:'Clear issue'}).click();await page.waitForFunction(()=>plan.issue_id===null);
      });
      await t.test('refresh reads preserve a draft started while pending and cannot overwrite a completed newer mutation',async()=>{
        await mount();await page.evaluate(()=>{window.holdRead=true;window.refreshResult=document.querySelector('[data-topcoat-plans]')._plans.load({refresh:true});});
        await page.waitForFunction(()=>typeof releaseRead==='function');
        const title=page.getByLabel('Plan title');await title.fill('Draft begun during refresh');
        await page.evaluate(async()=>{holdRead=false;releaseRead();await refreshResult;});
        assert.equal(await title.inputValue(),'Draft begun during refresh');
        assert.equal(await title.evaluate(node=>node===document.activeElement),true);await title.press('Escape');
        await page.evaluate(()=>{holdRead=true;releaseRead=null;refreshResult=document.querySelector('[data-topcoat-plans]')._plans.load({refresh:true});});
        await page.waitForFunction(()=>typeof releaseRead==='function');
        await title.fill('Saved during refresh');await title.press('Enter');await page.waitForFunction(()=>plan.title==='Saved during refresh');
        await page.evaluate(async()=>{holdRead=false;releaseRead();await refreshResult;});
        assert.equal(await title.inputValue(),'Saved during refresh');
      });
      await t.test('lists load every cursor page, create in project scope, and persist view tabs',async()=>{
        await mount({mode:'list'});assert.equal(await page.locator('[data-plans-content] li').count(),201);
        assert.equal(await page.evaluate(()=>calls.filter(call=>call.url.startsWith('/plans?')).length),2);
        await page.getByLabel('Plan title').fill('New plan');await page.getByRole('button',{name:'Create plan'}).click();await page.waitForFunction(()=>destinations.length===1);
        assert.equal(await page.evaluate(()=>destinations[0]),'/ENG/plans/20');
        await page.locator('[data-plan-tab="done"]').click();assert.equal(await page.evaluate(()=>localStorage.getItem('lific:subtab:plans:3')),'done');
      });
      await t.test('viewers have no mutation affordances; failed loads retry; stale audience write results are discarded',async()=>{
        await mount({viewer:true});assert.equal(await page.locator('[data-step-link]').count(),0);assert.equal(await page.locator('[data-step-done="2"]').isDisabled(),true);
        await mount();await page.evaluate(async()=>{failure='/plans/10';await document.querySelector('[data-topcoat-plans]')._plans.load();});
        await page.getByRole('button',{name:'Retry'}).click();await page.getByLabel('Plan title').waitFor();
        await page.evaluate(()=>{holdWrite=true;document.querySelector('[data-step-done="2"]').click();});await page.waitForFunction(()=>typeof releaseWrite==='function');
        await page.evaluate(()=>{lificSession.state.user={id:2};dispatchEvent(new CustomEvent('lific:account-change'));});await page.getByLabel('Plan title').waitFor();
        await page.evaluate(()=>{releaseWrite();});await page.waitForTimeout(50);
        assert.equal(await page.locator('[data-step-done="2"]').isChecked(),false);
      });
      assert.deepEqual(errors,[]);
    }finally{await browser.close();}
  });

const {test}=require('node:test');
const assert=require('node:assert/strict');
const fs=require('node:fs');
const path=require('node:path');

test('headless modules preserve metadata, project issue scope, assignment, and live count data',
  {skip:!process.env.PLAYWRIGHT_EXECUTABLE_PATH},async t=>{
    const {chromium}=await import(path.resolve(__dirname,'../../../../e2e/node_modules/playwright/index.mjs'));
    const browser=await chromium.launch({headless:true,executablePath:process.env.PLAYWRIGHT_EXECUTABLE_PATH});
    const page=await browser.newPage();page.setDefaultTimeout(5000);const errors=[];page.on('pageerror',error=>errors.push(error.message));
    async function mount({mode='detail',viewer=false,enforced=true,projectLead=null}={}){
      await page.route('http://modules.test/**',route=>route.fulfill({contentType:'text/html',body:`<!doctype html><section data-topcoat-modules="${mode}" data-project-identifier="ENG" data-module-id="2"><p data-modules-status role="status"></p><div data-modules-error role="alert" hidden></div><div data-modules-content></div></section>`}));
      await page.goto(`http://modules.test/ENG/modules${mode==='detail'?'/2':''}`);
      await page.evaluate(({viewer,enforced,projectLead})=>{
        window.calls=[];window.destinations=[];window.failure=null;window.role=viewer?'viewer':'maintainer';window.holdWrite=false;window.enforced=enforced;window.projectLead=projectLead;
        window.module={id:2,project_id:3,name:'Engine',description:'Description **markdown**',emoji:'🚀',status:'active',created_at:'yesterday',updated_at:'today'};
        window.issues=[{id:7,project_id:3,module_id:2,identifier:'ENG-7',title:'Engine bug',status:'done'},
          {id:8,project_id:3,module_id:2,identifier:'ENG-8',title:'Engine cancelled',status:'cancelled'},
          {id:9,project_id:3,module_id:3,identifier:'ENG-9',title:'Unassigned engine work',status:'active'},
          {id:99,project_id:4,module_id:2,identifier:'OTHER-99',title:'Foreign issue',status:'active'}];
        window.blockedIssues=[{...issues[0],blocked_by:['ENG-8']}];window.workableIssues=[];
        const clone=value=>JSON.parse(JSON.stringify(value)),success=data=>({ok:true,data:clone(data)});
        window.model={status:'ready',projectId:3,issues:Array.from({length:601},(_,index)=>({id:index+1000,module_id:2,status:index===0?'done':'active'}))};
        window.lificSync={setActiveProject:id=>{window.activeProject=id;},ensureProject:async id=>{window.ensuredProject=id;return model;},peekProject:()=>model,subscribe:listener=>{window.syncListener=listener;return ()=>{};}};
        window.lificSession={state:{user:{id:1},publicProject:null,loading:false},request:async(url,options={})=>{
          calls.push({url,...options});if(failure===url){failure=null;return {ok:false,status:503,error:'Temporary module read failure'};}
          const body=options.body?JSON.parse(options.body):{};
          if(url==='/projects')return success([{id:3,identifier:'ENG',name:'Engineering',lead_user_id:projectLead}]);
          if(url==='/projects/3/my-role')return success({role,enforced,is_admin:false});
          if(url==='/modules?project_id=3')return success([module,{id:3,project_id:3,name:'Backlog module',status:'backlog'}]);
          if(url==='/modules'&&options.method==='POST')return success({...module,id:3,...body});
          if(url==='/modules/2'&&!options.method){const snapshot=clone(module);return window.holdRead?new Promise(resolve=>{window.releaseRead=()=>resolve(success(snapshot));}):success(snapshot);}
          if(url==='/modules/2'&&options.method==='PUT'){
            const apply=()=>{Object.assign(module,body);return success(module);};
            if(holdWrite)return new Promise(resolve=>{window.releaseWrite=()=>resolve(apply());});return apply();
          }
          if(url==='/modules/2'&&options.method==='DELETE')return success({deleted:true});
          if(url.startsWith('/issues?')){const params=new URLSearchParams(url.split('?')[1]);const rows=params.has('blocked')?blockedIssues:params.has('workable')?workableIssues:issues;const offset=Number(params.get('offset')||0),limit=Math.min(500,Number(params.get('limit')||50));return success(rows.filter(issue=>issue.project_id===Number(params.get('project_id'))&&issue.module_id===Number(params.get('module_id'))).slice(offset,offset+limit));}
          if(url.startsWith('/search?'))return success([{result_type:'issue',...issues.find(issue=>issue.id===9)},{result_type:'issue',...issues.find(issue=>issue.id===99)}]);
          if(url.startsWith('/issues/resolve/')){const identifier=decodeURIComponent(url.split('/').at(-1));const issue=issues.find(issue=>issue.identifier===identifier);return issue?success(issue):{ok:false,status:404,error:'Issue not found'};}
          const match=url.match(/^\/issues\/(\d+)$/);if(match&&options.method==='PUT'){const issue=issues.find(row=>row.id===Number(match[1]));Object.assign(issue,body);return success(issue);}
          return {ok:false,status:400,error:`Unexpected ${url}`};
        }};
      },{viewer,enforced,projectLead});
      for(const file of ['../../issue_detail/editor/assets/editor.js','../../plans/assets/picker.js','modules.js'])await page.addScriptTag({content:fs.readFileSync(path.join(__dirname,file),'utf8')});
      await page.waitForFunction(()=>document.querySelector('[data-topcoat-modules]').getAttribute('aria-busy')==='false');
      await page.evaluate(()=>{document.querySelector('[data-topcoat-modules]')._modules.navigate=path=>destinations.push(path);});
    }
    try{
      await t.test('detail fetches project/module-filtered issues and renders authoritative state and Markdown',async()=>{
        await mount();assert.equal(await page.locator('[data-module-progress]').textContent(),'1/2 issues done');
        assert.equal(await page.locator('[data-module-description] strong').textContent(),'markdown');
        assert.match(await page.locator('[data-module-issue-list]').textContent(),/Blocked by ENG-8/);
        assert.doesNotMatch(await page.locator('[data-module-issue-list]').textContent(),/Workable/);
        assert.ok(await page.evaluate(()=>calls.some(call=>call.url==='/issues?project_id=3&module_id=2&limit=500&blocked=true')));
        assert.ok(await page.evaluate(()=>calls.some(call=>call.url==='/issues?project_id=3&module_id=2&limit=500&workable=true')));
        assert.equal(await page.locator('[data-module-issue-list]').getByText('Foreign issue').count(),0);
        assert.ok(await page.evaluate(()=>calls.some(call=>call.url==='/issues?project_id=3&module_id=2&limit=500')));
        assert.equal(await page.getByRole('link',{name:'New issue in module'}).getAttribute('href'),'/ENG/issues/new?module=2');
        await page.getByLabel('Search module issues').fill('cancelled');assert.equal(await page.locator('[data-module-issue-list] li').count(),1);
        assert.equal(await page.locator('[data-module-issue-list] a').getAttribute('href'),'/ENG/issues/ENG-8');
        await page.evaluate(()=>{document.activeElement.blur();dispatchEvent(new PageTransitionEvent('pagehide',{persisted:true}));module.name='Restored module';dispatchEvent(new PageTransitionEvent('pageshow',{persisted:true}));});
        await page.waitForFunction(()=>document.querySelector('[data-module-name]').value==='Restored module');
        assert.equal(await page.evaluate(()=>document.querySelector('[data-topcoat-modules]')._modules.disposed),false);
      });
      await t.test('module detail pages all issues and authoritative memberships beyond the server cap',async()=>{
        await mount();await page.evaluate(async()=>{
          issues=Array.from({length:1001},(_,index)=>({id:index+1,project_id:3,module_id:2,identifier:`ENG-${index+1}`,title:`Module work ${index+1}`,status:index===1000?'done':'active'}));
          blockedIssues=issues.map(issue=>({...issue,blocked_by:['ENG-9000']}));workableIssues=issues;
          await document.querySelector('[data-topcoat-modules]')._modules.load();
        });
        assert.equal(await page.locator('[data-module-progress]').textContent(),'1/1001 issues done');
        assert.equal(await page.locator('[data-module-issue-list] li').count(),1001);
        assert.match(await page.locator('[data-module-issue-list] li').last().textContent(),/Blocked by ENG-9000.*Workable/);
        const reads=await page.evaluate(()=>calls.filter(call=>call.url.startsWith('/issues?')).map(call=>call.url));
        for(const filter of ['', '&blocked=true', '&workable=true'])assert.ok(reads.includes(`/issues?project_id=3&module_id=2&limit=500${filter}&offset=1000`));
      });
      await t.test('blocked/workable membership is refreshed from server filters, including wait-only blockers',async()=>{
        await mount();await page.evaluate(()=>{issues[1].status='active';workableIssues=[issues[1]];blockedIssues=[{...issues[0],blocked_by:[]}];return document.querySelector('[data-topcoat-modules]')._modules.load();});
        assert.match(await page.locator('[data-module-issue-list] li').first().textContent(),/Blocked/);
        assert.match(await page.locator('[data-module-issue-list] li').nth(1).textContent(),/Workable/);
        assert.doesNotMatch(await page.locator('[data-module-issue-list] li').first().textContent(),/Workable/);
      });
      await t.test('legacy metadata stays lead-gated while issue assignment retains its distinct permission',async()=>{
        await mount({enforced:false});assert.equal(await page.getByLabel('Module name').count(),0);
        assert.equal(await page.getByLabel('Module status').isDisabled(),true);assert.equal(await page.getByRole('button',{name:'Delete module'}).count(),0);
        assert.equal(await page.getByRole('button',{name:'Edit description'}).count(),0);assert.equal(await page.getByRole('button',{name:'Assign issue'}).count(),1);
        assert.equal(await page.evaluate(()=>document.querySelector('[data-topcoat-modules]')._modules.mutate('/modules/2','PUT',{name:'Denied'})),false);
        await page.getByRole('button',{name:'Assign issue'}).click();await page.getByLabel('Search issues').fill('9');await page.getByRole('option').filter({hasText:'ENG-9'}).waitFor();await page.getByLabel('Search issues').press('Enter');
        await page.waitForFunction(()=>issues.find(issue=>issue.id===9).module_id===2);
        await mount({mode:'list',enforced:false});assert.equal(await page.getByRole('button',{name:'Create module'}).count(),0);
        await mount({enforced:false,projectLead:1});assert.equal(await page.getByLabel('Module name').count(),1);
        assert.equal(await page.getByRole('button',{name:'Delete module'}).count(),1);
      });
      await t.test('module title, status, icon clear, and explicit description Save/Cancel persist existing payloads',async()=>{
        await mount();const name=page.getByLabel('Module name');await name.fill(' ');await name.press('Enter');assert.equal(await name.inputValue(),'Engine');
        await name.fill('Updated engine');await name.press('Enter');await page.waitForFunction(()=>module.name==='Updated engine');
        await page.getByLabel('Module status').selectOption('paused');await page.waitForFunction(()=>module.status==='paused');
        await page.getByLabel('Icon or emoji').fill('');await page.getByLabel('Icon or emoji').blur();await page.waitForFunction(()=>module.emoji===null);
        await page.getByRole('button',{name:'Edit description'}).click();await page.getByLabel('Description',{exact:true}).fill('Cancelled draft');await page.getByRole('button',{name:'Cancel',exact:true}).click();
        assert.equal(await page.evaluate(()=>module.description),'Description **markdown**');
        await page.getByRole('button',{name:'Edit description'}).click();await page.getByLabel('Description',{exact:true}).fill('Saved **description**');await page.getByRole('button',{name:'Save description'}).click();
        await page.waitForFunction(()=>document.querySelector('[data-module-description] strong')?.textContent==='description');
        assert.equal(await page.evaluate(()=>module.description),'Saved **description**');
      });
      await t.test('project-scoped issue picker assigns an issue and removal sends nullable module_id',async()=>{
        await mount();await page.getByRole('button',{name:'Assign issue'}).click();await page.getByLabel('Search issues').fill('OTHER-99');
        await page.getByRole('option').filter({hasText:'ENG-9'}).waitFor();assert.equal(await page.getByRole('option').filter({hasText:'OTHER-99'}).count(),0);
        await page.getByLabel('Search issues').fill('9');await page.getByRole('option').filter({hasText:'ENG-9'}).waitFor();await page.getByLabel('Search issues').press('Enter');
        await page.waitForFunction(()=>document.querySelectorAll('[data-module-issue-list] li').length===3);
        assert.equal(await page.evaluate(()=>issues.find(issue=>issue.id===9).module_id),2);
        await page.locator('[data-module-detach="9"]').click();await page.waitForFunction(()=>document.querySelectorAll('[data-module-issue-list] li').length===2);
        assert.equal(await page.evaluate(()=>issues.find(issue=>issue.id===9).module_id),null);
      });
      await t.test('refresh reads preserve a newly opened description draft and cannot replace a newer saved title',async()=>{
        await mount();await page.evaluate(()=>{window.holdRead=true;window.refreshResult=document.querySelector('[data-topcoat-modules]')._modules.load({refresh:true});});
        await page.waitForFunction(()=>typeof releaseRead==='function');
        await page.getByRole('button',{name:'Edit description'}).click();await page.getByLabel('Description',{exact:true}).fill('Draft begun during refresh');
        await page.evaluate(async()=>{holdRead=false;releaseRead();await refreshResult;});
        assert.equal(await page.getByLabel('Description',{exact:true}).inputValue(),'Draft begun during refresh');
        assert.equal(await page.getByLabel('Description',{exact:true}).evaluate(node=>node===document.activeElement),true);
        await page.getByRole('button',{name:'Cancel',exact:true}).click();
        await page.evaluate(()=>{holdRead=true;releaseRead=null;refreshResult=document.querySelector('[data-topcoat-modules]')._modules.load({refresh:true});});
        await page.waitForFunction(()=>typeof releaseRead==='function');const name=page.getByLabel('Module name');
        await name.fill('Saved during refresh');await name.press('Enter');await page.waitForFunction(()=>module.name==='Saved during refresh');
        await page.evaluate(async()=>{holdRead=false;releaseRead();await refreshResult;});
        assert.equal(await name.inputValue(),'Saved during refresh');
      });
      await t.test('scalar and issue assignment acknowledgements retain an open description form and its Save/Cancel actions',async()=>{
        await mount();await page.getByRole('button',{name:'Edit description'}).click();const description=page.getByLabel('Description',{exact:true});
        await description.fill('Unsaved description');await page.getByLabel('Module status').selectOption('paused');
        await page.waitForFunction(()=>module.status==='paused'&&!document.querySelector('[data-topcoat-modules]')._modules.busy);
        assert.equal(await description.inputValue(),'Unsaved description');
        await page.getByRole('button',{name:'Assign issue'}).click();await page.getByLabel('Search issues').fill('9');
        await page.getByRole('option').filter({hasText:'ENG-9'}).waitFor();await page.getByLabel('Search issues').press('Enter');
        await page.waitForFunction(()=>issues.find(issue=>issue.id===9).module_id===2&&!document.querySelector('[data-topcoat-modules]')._modules.busy);
        assert.equal(await description.inputValue(),'Unsaved description');
        await page.getByRole('button',{name:'Cancel',exact:true}).click();
        assert.equal(await page.locator('[data-module-description]').isVisible(),true);assert.equal(await page.getByRole('button',{name:'Edit description'}).isVisible(),true);
        await page.getByRole('button',{name:'Edit description'}).click();await description.fill('Retained draft to save');await page.getByLabel('Module status').selectOption('active');
        await page.waitForFunction(()=>module.status==='active'&&!document.querySelector('[data-topcoat-modules]')._modules.busy);
        await page.getByRole('button',{name:'Save description'}).click();await page.waitForFunction(()=>module.description==='Retained draft to save');
        assert.equal(await page.locator('[data-module-description-form]').count(),0);
      });
      await t.test('list counts come from the complete live read model and tabs/create retain project scope',async()=>{
        await mount({mode:'list'});assert.match(await page.locator('[data-modules-content]').textContent(),/1\/601 issues done/);
        assert.equal(await page.evaluate(()=>ensuredProject),3);assert.equal(await page.evaluate(()=>calls.some(call=>call.url.startsWith('/issues?'))),false);
        await page.evaluate(()=>{model={...model,issues:model.issues.map(row=>({...row,status:'done'}))};syncListener();});
        assert.match(await page.locator('[data-modules-content]').textContent(),/601\/601 issues done/);
        await page.locator('[data-module-tab="backlog"]').click();assert.equal(await page.evaluate(()=>localStorage.getItem('lific:subtab:modules:ENG')),'backlog');
        await page.getByLabel('Module name').fill('New engine');await page.getByLabel('Icon or emoji').fill('🔧');await page.getByRole('button',{name:'Create module'}).click();await page.waitForFunction(()=>destinations.length===1);
        assert.equal(await page.evaluate(()=>destinations[0]),'/ENG/modules/3');
        assert.deepEqual(await page.evaluate(()=>JSON.parse(calls.find(call=>call.url==='/modules'&&call.method==='POST').body)),{project_id:3,name:'New engine',status:'active',emoji:'🔧'});
      });
      await t.test('viewer controls, retry, failed mutations, and stale audience responses remain recoverable',async()=>{
        await mount({viewer:true});assert.equal(await page.getByRole('button',{name:'Assign issue'}).count(),0);assert.equal(await page.getByLabel('Module status').isDisabled(),true);
        await mount();await page.evaluate(async()=>{failure='/modules/2';await document.querySelector('[data-topcoat-modules]')._modules.load();});await page.getByRole('button',{name:'Retry'}).click();await page.getByLabel('Module name').waitFor();
        await page.evaluate(()=>{failure='/modules/2';});await page.getByLabel('Module name').fill('Refused edit');await page.getByLabel('Module name').press('Enter');await page.locator('[data-modules-error]').waitFor({state:'visible'});
        assert.equal(await page.getByLabel('Module name').inputValue(),'Engine');assert.equal(await page.getByLabel('Module name').isDisabled(),false);
        await page.evaluate(()=>{holdWrite=true;});await page.getByLabel('Module name').fill('Old account edit');await page.getByLabel('Module name').press('Enter');await page.waitForFunction(()=>typeof releaseWrite==='function');
        await page.evaluate(()=>{lificSession.state.user={id:2};dispatchEvent(new CustomEvent('lific:account-change'));});await page.getByLabel('Module name').waitFor();await page.evaluate(()=>releaseWrite());await page.waitForTimeout(50);
        assert.equal(await page.getByLabel('Module name').inputValue(),'Engine');
      });
      await t.test('module deletion keeps existing issue records and returns to the list',async()=>{
        await mount();page.once('dialog',dialog=>dialog.accept());await page.getByRole('button',{name:'Delete module'}).click();await page.waitForFunction(()=>destinations.length===1);
        assert.equal(await page.evaluate(()=>destinations[0]),'/ENG/modules');assert.equal(await page.evaluate(()=>issues.length),4);
      });
      assert.deepEqual(errors,[]);
    }finally{await browser.close();}
  });

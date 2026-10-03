const {test}=require('node:test');
const assert=require('node:assert/strict');
const fs=require('node:fs');
const path=require('node:path');

test('headless activity, insights, and graph preserve data scope, aggregate meaning, and reachable actions',
  {skip:!process.env.PLAYWRIGHT_EXECUTABLE_PATH},async t=>{
    const {chromium}=await import(path.resolve(__dirname,'../../../../e2e/node_modules/playwright/index.mjs'));
    const browser=await chromium.launch({headless:true,executablePath:process.env.PLAYWRIGHT_EXECUTABLE_PATH});
    const page=await browser.newPage();page.setDefaultTimeout(6000);const errors=[];page.on('pageerror',error=>errors.push(error.message));
    async function mount(mode,{viewer=false,query=''}={}){
      await page.route('http://analytics.test/**',route=>route.fulfill({contentType:'text/html',body:`<!doctype html><section data-topcoat-analytics="${mode}" data-project-identifier="ENG"><p data-analytics-status role="status"></p><div data-analytics-error role="alert" hidden></div><div data-analytics-content></div></section>`}));
      await page.goto(`http://analytics.test/ENG/${mode}${query}`);
      await page.evaluate(({viewer})=>{
        window.calls=[];window.destinations=[];window.failPath=null;window.holdPath=null;window.releaseRead=null;window.graphRefreshFailure=false;
        window.activity=Array.from({length:51},(_,index)=>({id:51-index,ts:`2026-10-${index===50?'01':'02'} 10:00:00`,actor_user_id:index%2?7:null,actor_username:index%2?'mika':null,actor_display_name:index%2?'Mika':null,actor_is_bot:false,transport:index%2?'web':'system',entity_type:index===50?'page':'issue',entity_id:index===50?8:7,entity_label:index===50?'Knowledge':'ENG-7',project_id:3,issue_id:index===50?null:7,page_id:index===50?8:null,action:'update',field:'description',old_value:'first\nold line\nlast',new_value:'first\nnew line\nlast'}));
        window.actors=[{actor_user_id:7,username:'mika',display_name:'Mika',is_bot:false,actions:25,last_ts:'2026-10-02 10:00:00',top_transport:'web'},
          {actor_user_id:null,username:null,display_name:null,is_bot:false,actions:26,last_ts:'2026-10-02 10:00:00',top_transport:'system'}];
        window.insights={weeks:12,created_per_week:[{week_start:'2026-09-21',count:0},{week_start:'2026-09-28',count:3}],closed_per_week:[{week_start:'2026-09-28',count:1}],status_counts:{total:3,backlog:1,todo:1,active:0,done:1,cancelled:0},priority_counts:{urgent:0,high:1,medium:0,low:1,none:1},module_counts:[{module_id:2,name:'Engine',count:2}],top_actors:actors};
        window.issues=[{id:1,project_id:3,identifier:'ENG-1',title:'Blocker',status:'todo',priority:'high',description:'Full blocker description'},
          {id:2,project_id:3,identifier:'ENG-2',title:'Dependent',status:'active',priority:'medium'},
          {id:3,project_id:3,identifier:'ENG-3',title:'Unlinked work',status:'todo',priority:'none'},
          {id:4,project_id:3,identifier:'ENG-4',title:'Closed work',status:'done',priority:'low'},
          {id:99,project_id:4,identifier:'OTHER-99',title:'Foreign project',status:'todo'}];
        window.relations=[{source_id:1,target_id:2,source_identifier:'ENG-1',target_identifier:'ENG-2',relation_type:'blocks'},
          {source_id:3,target_id:4,source_identifier:'ENG-3',target_identifier:'ENG-4',relation_type:'relates_to'}];
        const clone=value=>JSON.parse(JSON.stringify(value)),ok=data=>({ok:true,data:clone(data)});
        window.lificSync={setActiveProject:id=>{window.activeProject=id;}};
        window.lificSession={state:{user:{id:1},publicProject:null,loading:false},request:async(url,options={})=>{
          calls.push({url,method:options.method,body:options.body});
          const result=()=>{
            if(url===failPath){failPath=null;return {ok:false,status:503,error:'Temporary route failure'};}
            if(graphRefreshFailure&&url.startsWith('/issues?')){graphRefreshFailure=false;return {ok:false,status:503,error:'Graph refresh unavailable'};}
            if(url==='/projects')return ok([{id:3,identifier:'ENG',name:'Engineering'}]);
            if(url==='/projects/3/my-role')return ok({enforced:true,is_admin:false,role:viewer?'viewer':'maintainer'});
            if(url==='/projects/3/activity/actors')return ok(actors);
            if(url.startsWith('/projects/3/activity?')){const query=new URLSearchParams(url.split('?')[1]),offset=Number(query.get('offset')),limit=Number(query.get('limit'));return ok({items:activity.slice(offset,offset+limit),has_more:activity.length>offset+limit});}
            if(url.startsWith('/projects/3/insights?'))return ok({...insights,weeks:Number(new URLSearchParams(url.split('?')[1]).get('weeks'))});
            if(url.startsWith('/issues?')){const params=new URLSearchParams(url.split('?')[1]),offset=Number(params.get('offset')||0),limit=Math.min(500,Number(params.get('limit')||50));return ok(issues.filter(issue=>issue.project_id===Number(params.get('project_id'))).slice(offset,offset+limit));}
            if(url==='/projects/3/relations')return ok(relations);
            if(url.startsWith('/issues/resolve/'))return ok(issues.find(row=>row.identifier===decodeURIComponent(url.split('/').at(-1))));
            const body=options.body?JSON.parse(options.body):{};
            if(url==='/issues/link'){const source=issues.find(row=>row.identifier===body.source),target=issues.find(row=>row.identifier===body.target);relations.push({source_id:source.id,target_id:target.id,source_identifier:source.identifier,target_identifier:target.identifier,relation_type:body.relation_type});return ok({linked:true});}
            if(url==='/issues/unlink'){relations=relations.filter(row=>!(row.source_identifier===body.source&&row.target_identifier===body.target));return ok({unlinked:true});}
            if(url==='/issues/reverse'){const relation=relations.find(row=>row.source_identifier===body.source&&row.target_identifier===body.target);[relation.source_id,relation.target_id]=[relation.target_id,relation.source_id];[relation.source_identifier,relation.target_identifier]=[relation.target_identifier,relation.source_identifier];return ok({reversed:true});}
            return {ok:false,status:400,error:`Unexpected ${url}`};
          };
          if(url===holdPath){const snapshot=result();holdPath=null;return new Promise(resolve=>{window.releaseRead=()=>{const release=resolve;window.releaseRead=null;release(snapshot);};});}return result();
        }};
      },{viewer});
      for(const file of ['model.js','routes.js'])await page.addScriptTag({content:fs.readFileSync(path.join(__dirname,file),'utf8')});
      await page.addStyleTag({content:fs.readFileSync(path.join(__dirname,'routes.css'),'utf8')});
      await page.waitForFunction(()=>document.querySelector('[data-topcoat-analytics]').getAttribute('aria-busy')==='false');
      await page.evaluate(()=>document.querySelector('[data-topcoat-analytics]')._analytics.navigate=destination=>destinations.push(destination));
    }
    try{
      await t.test('activity actor/system/date/query filters remain in the URL and pagination keeps entity links and diffs',async()=>{
        await mount('activity',{query:'?actor=system&unknown=keep'});assert.equal(await page.locator('[data-activity-rows] details').count(),25);
        await page.getByRole('button',{name:/Mika · 25 actions/}).click();assert.equal(await page.locator('[data-activity-rows] details').count(),25);
        assert.match(page.url(),/actor=7/);assert.match(page.url(),/unknown=keep/);
        await page.getByLabel('Search activity').fill('new line');await page.getByLabel('From date').fill('2026-10-02');await page.getByLabel('To date').fill('2026-10-02');
        assert.match(page.url(),/q=new\+line/);assert.match(page.url(),/start=2026-10-02/);assert.equal(await page.locator('[data-activity-rows] details').count(),25);
        const first=page.locator('[data-activity-rows] details').first();await first.locator('summary').press('Enter');assert.equal(await first.getAttribute('open'),'');
        assert.match(await first.locator('.tc-activity__diff').textContent(),/\+ new line/);assert.equal(await first.getByRole('link',{name:'ENG-7'}).getAttribute('href'),'/ENG/issues/ENG-7');
        await page.getByRole('button',{name:'Clear filters'}).click();await page.getByRole('button',{name:'Load more'}).click();await page.waitForFunction(()=>document.querySelectorAll('[data-activity-rows] details').length===51);
        assert.ok(await page.evaluate(()=>calls.some(call=>call.url==='/projects/3/activity?limit=50&offset=50')));
        const last=page.locator('[data-activity-rows] details').last();await last.locator('summary').click();await last.getByRole('link',{name:'Knowledge'}).click();assert.deepEqual(await page.evaluate(()=>destinations),['/ENG/pages/8']);
      });
      await t.test('activity realtime refresh ignores other projects, merges fresh entries, preserves filters, and resync clears until recovered',async()=>{
        await mount('activity');await page.getByLabel('Search activity').fill('new line');
        const before=await page.evaluate(()=>calls.length);await page.evaluate(()=>dispatchEvent(new CustomEvent('lific:realtime',{detail:{type:'issue.updated',project_id:9}})));await page.waitForTimeout(160);assert.equal(await page.evaluate(()=>calls.length),before);
        await page.evaluate(()=>{activity.unshift({...activity[0],id:99});dispatchEvent(new CustomEvent('lific:realtime',{detail:{type:'issue.updated',project_id:3}}));});
        await page.waitForFunction(()=>document.querySelectorAll('[data-activity-rows] details').length===51);assert.equal(await page.getByLabel('Search activity').inputValue(),'new line');
        await page.evaluate(()=>{holdPath='/projects';dispatchEvent(new CustomEvent('lific:realtime',{detail:{type:'resync.required'}}));});await page.waitForFunction(()=>typeof releaseRead==='function');
        assert.equal(await page.locator('[data-activity-rows] details').count(),0);await page.evaluate(()=>{holdPath=null;releaseRead();});await page.getByLabel('Search activity').waitFor();assert.equal(await page.getByLabel('Search activity').inputValue(),'new line');
        const projectReads=await page.evaluate(()=>calls.filter(call=>call.url==='/projects').length);
        await page.evaluate(()=>{dispatchEvent(new PageTransitionEvent('pagehide',{persisted:true}));dispatchEvent(new PageTransitionEvent('pageshow',{persisted:true}));});
        await page.waitForFunction(before=>calls.filter(call=>call.url==='/projects').length>before,projectReads);await page.getByLabel('Search activity').waitFor();
      });
      await t.test('insights retain server counts, missing observations, all aggregate windows, and no realtime subscription',async()=>{
        await mount('insights');await page.getByText('Weekly data',{exact:true}).click();
        assert.deepEqual(await page.locator('tbody tr').first().locator('td').allTextContents(),['0','Unavailable']);assert.deepEqual(await page.locator('tbody tr').last().locator('td').allTextContents(),['3','1']);
        assert.equal(await page.getByRole('img',{name:/2026-09-28: 3 created, 1 closed/}).count(),1);
        for(const weeks of [4,26,52,12]){await page.getByRole('button',{name:`${weeks}w`,exact:true}).click();await page.waitForFunction(weeks=>document.querySelector(`[data-weeks="${weeks}"]`)?.getAttribute('aria-pressed')==='true',weeks);}
        assert.match(await page.locator('[data-analytics-content]').textContent(),/Top actors · last 12 weeks/);
        const before=await page.evaluate(()=>calls.length);await page.evaluate(()=>dispatchEvent(new CustomEvent('lific:realtime',{detail:{type:'resync.required',project_id:3}})));await page.waitForTimeout(160);assert.equal(await page.evaluate(()=>calls.length),before);
        await page.evaluate(()=>{insights.created_per_week=[{week_start:'2026-09-28',count:0}];insights.closed_per_week=[{week_start:'2026-09-28',count:null}];});
        await page.getByRole('button',{name:'4w',exact:true}).click();await page.getByText(/trend values are unavailable/i).waitFor();assert.equal(await page.getByText(/No issues created or closed/).count(),0);
        await page.evaluate(()=>{insights.created_per_week=[{week_start:'2026-09-28',count:0}];insights.closed_per_week=[{week_start:'2026-09-28',count:0}];});
        await page.getByRole('button',{name:'26w',exact:true}).click();await page.getByText(/No issues created or closed/).waitFor();
        await page.evaluate(()=>{insights.status_counts.total=0;});await page.getByRole('button',{name:'4w',exact:true}).click();await page.getByText(/Nothing to chart yet/).waitFor();
        await page.evaluate(()=>{failPath='/projects/3/insights?weeks=26';});await page.getByRole('button',{name:'26w',exact:true}).click();await page.getByRole('button',{name:'Retry'}).click();await page.getByRole('button',{name:'26w',exact:true}).waitFor();
      });
      await t.test('graph linkage, closed visibility, layout, keyboard navigation, and hover preview preserve destinations',async()=>{
        await mount('graph');assert.equal(await page.locator('[data-graph-node]').count(),2);assert.equal(await page.locator('[data-graph-node="99"]').count(),0);
        const position=await page.evaluate(()=>[...document.querySelector('[data-topcoat-analytics]')._analytics.positions.values()]);assert.ok(position.find(row=>row.id===1).x<position.find(row=>row.id===2).x);
        await page.getByLabel('Show closed').check();assert.equal(await page.locator('[data-graph-node]').count(),4);
        await page.getByRole('button',{name:'Unlinked (0)',exact:true}).click();await page.getByText('Everything in this view is linked.').waitFor();
        await page.getByLabel('Show closed').uncheck();assert.equal(await page.locator('[data-graph-node]').count(),1);await page.getByRole('button',{name:'Linked (2)',exact:true}).click();
        const canvas=page.getByRole('region',{name:/Dependency graph canvas/});await canvas.focus();const previous=await page.evaluate(()=>document.querySelector('[data-topcoat-analytics]')._analytics.transform.x);await canvas.press('ArrowRight');assert.equal(await page.evaluate(()=>document.querySelector('[data-topcoat-analytics]')._analytics.transform.x),previous-40);
        await canvas.press('+');assert.equal(await page.locator('[data-graph-scale]').textContent(),'120%');await canvas.press('Home');
        await page.locator('[data-graph-issue="1"]').hover();await page.getByLabel('Preview ENG-1').getByText('Full blocker description').waitFor();await page.locator('[data-graph-issue="1"]').focus();await page.locator('[data-graph-issue="1"]').press('Escape');assert.equal(await page.getByLabel('Preview ENG-1').count(),0);
        const link=page.getByRole('region',{name:'Graph text alternative'}).getByRole('link',{name:'ENG-1 · Blocker',exact:true});await link.focus();await link.press('Enter');assert.deepEqual(await page.evaluate(()=>destinations),['/ENG/issues/ENG-1']);
      });
      await t.test('graph relation create, atomic reverse, and remove preserve payloads and failed actions remain recoverable',async()=>{
        await mount('graph');await page.getByRole('button',{name:'Unlinked (1)',exact:true}).click();await page.getByRole('button',{name:'Connect ENG-3',exact:true}).click();await page.getByLabel('Target issue').selectOption('1');await page.getByLabel('Relation type').selectOption('duplicate');await page.getByRole('dialog').getByRole('button',{name:'Create relation',exact:true}).click();await page.getByRole('dialog').waitFor({state:'hidden'});
        const created=await page.evaluate(()=>calls.find(call=>call.url==='/issues/link'));assert.deepEqual(JSON.parse(created.body),{source:'ENG-3',target:'ENG-1',relation_type:'duplicate'});
        await page.getByRole('button',{name:'Linked (3)',exact:true}).click();await page.getByRole('button',{name:'Manage relation ENG-1 blocks ENG-2',exact:true}).click();
        await page.evaluate(()=>{failPath='/issues/reverse';});await page.getByRole('button',{name:'Reverse direction'}).click();await page.getByRole('dialog').getByRole('alert').waitFor({state:'visible'});assert.equal(await page.evaluate(()=>relations[0].source_identifier),'ENG-1');
        await page.getByRole('button',{name:'Reverse direction'}).click();await page.getByRole('dialog').waitFor({state:'hidden'});assert.equal(await page.evaluate(()=>relations[0].source_identifier),'ENG-2');
        await page.getByRole('button',{name:'Manage relation ENG-2 blocks ENG-1',exact:true}).click();await page.getByRole('button',{name:'Remove relation'}).click();await page.getByRole('dialog').waitFor({state:'hidden'});
        assert.ok(await page.evaluate(()=>calls.filter(call=>call.url==='/issues/reverse').every(call=>JSON.parse(call.body).source==='ENG-1')));
      });
      await t.test('graph loads, refreshes, and relation acknowledgements retain endpoints beyond the server cap',async()=>{
        await mount('graph');await page.evaluate(async()=>{
          issues=Array.from({length:1001},(_,index)=>({id:index+1,project_id:3,identifier:`ENG-${index+1}`,title:`Graph work ${index+1}`,status:'active'}));
          relations=[{source_id:1,target_id:1001,source_identifier:'ENG-1',target_identifier:'ENG-1001',relation_type:'blocks'}];
          await document.querySelector('[data-topcoat-analytics]')._analytics.load();
        });
        assert.equal(await page.locator('[data-graph-node="1001"]').count(),1);
        await page.getByRole('button',{name:'Manage relation ENG-1 blocks ENG-1001',exact:true}).waitFor();
        await page.evaluate(()=>document.querySelector('[data-topcoat-analytics]')._analytics.refresh());
        assert.equal(await page.locator('[data-graph-node="1001"]').count(),1);
        await page.getByRole('button',{name:'Manage relation ENG-1 blocks ENG-1001',exact:true}).click();await page.getByRole('button',{name:'Reverse direction'}).click();
        await page.getByRole('dialog').waitFor({state:'hidden'});await page.getByRole('button',{name:'Manage relation ENG-1001 blocks ENG-1',exact:true}).waitFor();
        assert.equal(await page.locator('[data-graph-node="1001"]').count(),1);
        assert.equal(await page.evaluate(()=>calls.filter(call=>call.url==='/issues?project_id=3&limit=500&offset=1000').length),3);
      });
      await t.test('accepted relation writes with failed reload offer read retry without accidentally reversing twice',async()=>{
        await mount('graph');await page.getByRole('button',{name:'Manage relation ENG-1 blocks ENG-2',exact:true}).click();await page.evaluate(()=>{graphRefreshFailure=true;});await page.getByRole('button',{name:'Reverse direction'}).click();
        await page.getByText(/Relation saved, but the graph could not be refreshed/).waitFor();assert.equal(await page.getByRole('dialog').count(),0);await page.getByRole('button',{name:'Retry'}).click();await page.getByRole('button',{name:'Manage relation ENG-2 blocks ENG-1',exact:true}).waitFor();
        assert.equal(await page.evaluate(()=>calls.filter(call=>call.url==='/issues/reverse').length),1);
      });
      await t.test('graph refreshes defer while a relation dialog is open and discard snapshots predating a mutation',async()=>{
        await mount('graph');await page.evaluate(()=>{holdPath='/projects/3/relations';dispatchEvent(new CustomEvent('lific:realtime',{detail:{type:'issue.updated',project_id:3}}));});await page.waitForFunction(()=>typeof releaseRead==='function');
        await page.getByRole('button',{name:'Manage relation ENG-1 blocks ENG-2',exact:true}).click();await page.evaluate(()=>{holdPath=null;releaseRead();});await page.getByRole('dialog').waitFor();assert.equal(await page.getByRole('dialog').count(),1);await page.getByRole('dialog').press('Escape');
        await page.evaluate(()=>{holdPath='/projects/3/relations';dispatchEvent(new CustomEvent('lific:realtime',{detail:{type:'issue.updated',project_id:3}}));});await page.waitForFunction(()=>typeof releaseRead==='function');
        await page.getByRole('button',{name:'Manage relation ENG-1 blocks ENG-2',exact:true}).click();await page.getByRole('button',{name:'Remove relation'}).click();await page.getByRole('dialog').waitFor({state:'hidden'});
        await page.evaluate(()=>{holdPath=null;releaseRead();});await page.waitForTimeout(250);assert.equal(await page.getByRole('button',{name:'Manage relation ENG-1 blocks ENG-2',exact:true}).count(),0);
        assert.equal(await page.evaluate(()=>relations.some(row=>row.source_identifier==='ENG-1'&&row.target_identifier==='ENG-2')),false);
      });
      await t.test('viewer graph has reachable relation links without edit controls and stale account loads cannot restore prior data',async()=>{
        await mount('graph',{viewer:true});assert.equal(await page.getByRole('button',{name:'Create relation',exact:true}).count(),0);assert.equal(await page.getByRole('button',{name:/Manage relation/}).count(),0);assert.equal(await page.locator('[data-graph-source]').count(),0);
        assert.equal(await page.locator('[data-graph-relation-list] a').count(),2);
        await page.evaluate(()=>{holdPath='/issues?project_id=3&limit=500';void document.querySelector('[data-topcoat-analytics]')._analytics.load();});await page.waitForFunction(()=>typeof releaseRead==='function');
        await page.evaluate(()=>{lificSession.state.user=null;dispatchEvent(new CustomEvent('lific:account-change'));});assert.equal(await page.locator('[data-graph-node]').count(),0);await page.evaluate(()=>{holdPath=null;releaseRead();});await page.waitForTimeout(50);assert.equal(await page.locator('[data-graph-node]').count(),0);await page.getByText('Sign in to view project data.').waitFor();
      });
      assert.deepEqual(errors,[]);
    }finally{await browser.close();}
  });

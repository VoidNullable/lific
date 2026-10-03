const {test} = require('node:test');
const assert = require('node:assert/strict');
const fs = require('node:fs');
const path = require('node:path');

test('headless issue collaboration preserves drafts and emits coordinator intents for all panel actions', {skip: !process.env.PLAYWRIGHT_EXECUTABLE_PATH}, async t => {
  const {chromium} = await import(path.resolve(__dirname, '../../../../../e2e/node_modules/playwright/index.mjs'));
  const browser = await chromium.launch({headless:true, executablePath:process.env.PLAYWRIGHT_EXECUTABLE_PATH});
  try {
    const page = await browser.newPage(); page.setDefaultTimeout(5000);
    const script = fs.readFileSync(path.join(__dirname,'collaboration.js'),'utf8');
    const html = `<!doctype html><html><body><section data-topcoat-issue-detail data-project-identifier="TEAM_ALPHA" data-issue-scope="private"><main data-topcoat-collaboration data-issue-id="0" data-edit-enabled="false" data-comment-enabled="false">
      <header><h2 id="tc-comments-heading">Comments</h2><span data-comment-count></span></header><div data-comment-thread></div><button data-comments-older hidden>Load older comments</button>
      <form data-comment-compose><label>Write a comment<textarea data-comment-draft aria-label="Write a comment"></textarea></label><div data-mention-list hidden></div><button type="submit">Comment</button></form>
      <ul data-relation-list></ul><form data-relation-create><input name="target"><select name="kind"><option value="blocks">Blocks</option><option value="relates_to">Relates to</option><option value="duplicate">Duplicate</option></select><button type="submit">Link issue</button></form>
      <ul data-wait-list></ul><form data-wait-create><select name="kind"><option value="user">A person</option><option value="date">A date</option></select><label data-wait-user-field>Username<input name="user"></label><label data-wait-date-field hidden>From<input name="from"></label><label data-wait-until-field hidden>Until<input name="until"></label><input name="note"><button type="submit">Add wait</button></form>
      <div data-issue-attachments></div><ol data-issue-history></ol><button data-issue-delete>Delete issue</button><button data-issue-restore hidden>Restore issue</button><p data-collab-status></p><textarea data-description-editor aria-label="Issue description"></textarea></main></section></body></html>`;
    let comments = [{id:3,issue_id:12,user_id:4,author:'Sam',author_display_name:'Sam User',content:'Existing comment',created_at:'2026-10-01T10:00:00Z',updated_at:'2026-10-01T10:00:00Z'}];
    await page.setContent(html);
    await page.evaluate(() => {
      window.intents=[]; window.confirm=()=>true;
      window.fixtureAffordances={manage:true,edit:true,comment:true};window.lificSession={state:{user:{id:4},publicProject:null,role:{role:'maintainer',enforced:true,is_admin:false}},affordances:()=>fixtureAffordances,scopedRoute:path=>lificSession.state.publicProject?`/public${path}`:path,resolve:path=>({url:`/api${path}`}),request:async path=>{
        if(path.includes('/mention-candidates')) return {ok:true,data:[{user_id:9,username:'maria',display_name:'Maria Jones'}]};
        if(path.includes('/comments?')) {const query=new URLSearchParams(path.split('?')[1]);let rows=window.fixtureComments.slice().sort((a,b)=>b.created_at.localeCompare(a.created_at)||b.id-a.id);if(query.has('before_created_at'))rows=rows.filter(row=>row.created_at<query.get('before_created_at')||(row.created_at===query.get('before_created_at')&&row.id<Number(query.get('before_id'))));return {ok:true,data:rows.slice(0,Number(query.get('limit')||51)),headers:{get:()=>null}};}
        if(path.includes('/activity?')) return {ok:true,data:{items:[{id:1,actor_display_name:'Sam',action:'created',ts:'2026-10-01T10:00:00Z'}]}};
        if(path.startsWith('/attachments?')) return {ok:true,data:[{id:8,filename:'plan.txt',mime:'text/plain',size_bytes:5,uploader_id:4,created_at:'2026-10-01'}]};
        return {ok:true,data:[]};
      }};
      window.LificTopcoatAttachments={viewerKind:()=> 'file',createClient:({session})=>({session}),attach:(_target,options)=>{window.fixtureAttachmentOptions=options;return {dispose(){}};}};
      window.fixtureComments=[];window.addEventListener('lific:issue-detail-intent',event=>intents.push(event.detail));
    });
    await page.addScriptTag({content:script});
    await page.evaluate(rows=>{fixtureComments=rows;const root=document.querySelector('[data-topcoat-collaboration]');LificTopcoatIssueCollaboration.mount(root,{route:{issue_id:12,generation:4},issue:{id:12,project_id:7,identifier:'ENG-4',blocks:['ENG-5'],blocked_by:['ENG-3'],relates_to:[],duplicates:[],duplicated_by:[],waits:[{id:5,kind:'user',username:'maria',display_name:'Maria',state:'holding',note:'Review'}]},capabilities:{edit:true,comment:true}});},comments);
    await page.getByText('Existing comment').waitFor(); await page.getByText('Waiting for Maria').waitFor();
    await page.getByText('plan.txt').waitFor(); await page.getByText('created', {exact:false}).waitFor();
    assert.deepEqual(await page.evaluate(()=>fixtureAttachmentOptions.target),{entity_type:'issue',entity_id:12});
    await page.evaluate(async()=>fixtureAttachmentOptions.onUploaded());
    await page.locator('[data-issue-attachments] [data-attachment-id="8"]').waitFor();

    await t.test('relation links use the route project and retain public scope',async()=>{
      assert.equal(await page.locator('[data-relation-target="ENG-5"] a').getAttribute('href'),'/TEAM_ALPHA/issues/ENG-5');
      await page.evaluate(()=>{lificSession.state.publicProject='TEAM_ALPHA';LificTopcoatIssueCollaboration.renderRelations(document.querySelector('[data-topcoat-collaboration]'));});
      assert.equal(await page.locator('[data-relation-target="ENG-5"] a').getAttribute('href'),'/public/TEAM_ALPHA/issues/ENG-5');
      await page.evaluate(()=>{lificSession.state.publicProject=null;LificTopcoatIssueCollaboration.renderRelations(document.querySelector('[data-topcoat-collaboration]'));});
    });

    await t.test('comment create, mention insertion, focus and external editor draft safety',async()=>{
      const draft=page.getByRole('textbox',{name:'Write a comment'});await draft.fill('Please ask @mar');await page.locator('[data-mention-user="maria"]').waitFor();await page.locator('[data-mention-user="maria"]').click();assert.equal(await draft.inputValue(),'Please ask @maria ');
      await draft.press('End');await page.getByRole('button',{name:'Comment',exact:true}).click();
      assert.equal(await page.evaluate(()=>intents.at(-1).action.operation),'create_comment');assert.equal(await page.evaluate(()=>document.activeElement.matches('[data-comment-draft]')),true);
      assert.equal(await draft.inputValue(),'Please ask @maria ');
      await page.evaluate(()=>{const action=intents.at(-1).action;dispatchEvent(new CustomEvent('lific:issue-detail-error',{detail:{route:{issue_id:12,generation:4},panel:'comments',operation:action.operation,action,error:'Comment service unavailable.'}}));});
      assert.equal(await draft.inputValue(),'Please ask @maria ');assert.match(await page.locator('[data-collab-status]').innerText(),/Comment service unavailable/);
      await page.getByRole('button',{name:'Comment',exact:true}).click();assert.equal(await page.evaluate(()=>intents.at(-1).action.operation),'create_comment');
      await page.getByRole('textbox',{name:'Issue description'}).fill('Unsaved description draft');
      await page.evaluate(()=>{fixtureComments.push({id:4,issue_id:12,user_id:4,author:'Sam',author_display_name:'Sam User',content:'Posted',created_at:'2026-10-02T10:00:00Z',updated_at:'2026-10-02T10:00:00Z'});dispatchEvent(new CustomEvent('lific:issue-detail-applied',{detail:{route:{issue_id:12,generation:4},panel:'comments',issue:{id:12,blocks:['ENG-5'],blocked_by:['ENG-3'],relates_to:[],duplicates:[],duplicated_by:[],waits:[{id:5,kind:'user',username:'maria',display_name:'Maria',state:'holding'}]}}}));});
      await page.getByText('Posted').waitFor();assert.equal(await draft.inputValue(),'');assert.equal(await page.getByRole('textbox',{name:'Issue description'}).inputValue(),'Unsaved description draft');assert.equal(await page.getByRole('textbox',{name:'Issue description'}).evaluate(el=>el===document.activeElement),true);
    });

    await t.test('comment edit and delete use coordinator actions',async()=>{
      await page.getByRole('button',{name:'Edit',exact:true}).first().click();await page.getByRole('textbox',{name:'Edit comment'}).fill('Edited safely');await page.getByRole('button',{name:'Save comment'}).click();assert.equal(await page.evaluate(()=>intents.at(-1).action.operation),'edit_comment');
      page.once('dialog',dialog=>dialog.accept());await page.getByRole('button',{name:'Delete',exact:true}).first().click();assert.equal(await page.evaluate(()=>intents.at(-1).action.operation),'delete_comment');
    });

    await t.test('relation link, unlink and reverse plus user/date wait transitions',async()=>{
      await page.locator('[data-relation-create] [name=target]').fill('ENG-9');await page.locator('[data-relation-create] [name=kind]').selectOption('blocks');await page.getByRole('button',{name:'Link issue'}).click();assert.equal(await page.evaluate(()=>intents.at(-1).action.operation),'link_relation');
      const linkAction=await page.evaluate(()=>intents.at(-1).action);assert.equal(await page.locator('[data-relation-create] [name=target]').inputValue(),'ENG-9');
      await page.evaluate(action=>dispatchEvent(new CustomEvent('lific:issue-detail-error',{detail:{route:{issue_id:12,generation:4},panel:'relations',operation:'link_relation',action,error:'Relation write failed.'}})),linkAction);
      assert.equal(await page.locator('[data-relation-create] [name=target]').inputValue(),'ENG-9');assert.match(await page.locator('[data-collab-status]').innerText(),/Relation write failed/);
      await page.getByRole('button',{name:'Link issue'}).click();
      await page.evaluate(()=>dispatchEvent(new CustomEvent('lific:issue-detail-applied',{detail:{route:{issue_id:12,generation:4},panel:'relations',issue:{blocks:['ENG-5','ENG-9'],blocked_by:['ENG-3'],relates_to:[],duplicates:[],duplicated_by:[],waits:[{id:5,kind:'user',username:'maria',display_name:'Maria',state:'holding'}]}}})));
      assert.equal(await page.locator('[data-relation-create] [name=target]').inputValue(),'');
      await page.getByRole('button',{name:'Remove relation to ENG-3'}).click();assert.equal(await page.evaluate(()=>intents.at(-1).action.operation),'unlink_relation');
      await page.getByRole('button',{name:'Reverse relation to ENG-3'}).click();assert.equal(await page.evaluate(()=>intents.at(-1).action.operation),'reverse_relation');assert.equal(await page.evaluate(()=>intents.at(-1).action.source),'ENG-3');assert.equal(await page.evaluate(()=>intents.at(-1).action.target),'ENG-4');
      await page.locator('[data-wait-create] select[name=kind]').selectOption('date');assert.equal(await page.locator('[data-wait-date-field]').isVisible(),true);await page.locator('[name=from]').fill('2026-10-10');await page.locator('[name=until]').fill('2026-10-12');await page.locator('[data-wait-create] [name=note]').fill('Check then');await page.getByRole('button',{name:'Add wait'}).click();assert.equal(await page.evaluate(()=>intents.at(-1).action.input.from),'2026-10-10');
      const waitAction=await page.evaluate(()=>intents.at(-1).action);assert.equal(await page.locator('[name=from]').inputValue(),'2026-10-10');
      await page.evaluate(action=>dispatchEvent(new CustomEvent('lific:issue-detail-conflict',{detail:{route:{issue_id:12,generation:4},panel:'waits',operation:'add_wait',action,current:{blocks:['ENG-5','ENG-9'],blocked_by:['ENG-3'],relates_to:[],duplicates:[],duplicated_by:[],waits:[]}}})),waitAction);
      assert.equal(await page.locator('[name=from]').inputValue(),'2026-10-10');assert.match(await page.locator('[data-collab-status]').innerText(),/draft is still here/);
      await page.getByRole('button',{name:'Add wait'}).click();
      await page.evaluate(()=>dispatchEvent(new CustomEvent('lific:issue-detail-applied',{detail:{route:{issue_id:12,generation:4},panel:'waits',issue:{blocks:['ENG-5','ENG-9'],blocked_by:['ENG-3'],relates_to:[],duplicates:[],duplicated_by:[],waits:[{id:6,kind:'date',earliest:'2026-10-10',latest:'2026-10-12',state:'due',note:'Check then'}]}}})));
      assert.equal(await page.locator('[name=from]').inputValue(),'');assert.equal(await page.locator('[data-wait-user-field]').isVisible(),true);assert.equal(await page.locator('[data-wait-date-field]').isVisible(),false);
      await page.getByRole('button',{name:'Clear wait'}).click();assert.equal(await page.evaluate(()=>intents.at(-1).action.operation),'clear_wait');
    });

    await t.test('route issue refresh updates panels without replacing comment drafts',async()=>{
      const draft=page.getByRole('textbox',{name:'Write a comment'});await draft.fill('Keep this comment draft');
      await page.evaluate(()=>document.querySelector('[data-topcoat-collaboration]')._issueCollaboration.update({id:12,project_id:7,identifier:'ENG-4',blocks:['ENG-5'],blocked_by:['ENG-3'],relates_to:[],duplicates:[],duplicated_by:[],waits:[{id:6,kind:'date',earliest:'2026-10-10',latest:'2026-10-12',state:'due'}]},{edit:true,comment:true}));
      assert.equal(await draft.inputValue(),'Keep this comment draft');assert.equal(await page.locator('[data-wait-id="6"]').count(),1);
    });

    await t.test('delete and restore route intents and stale completion isolation',async()=>{
      await page.getByRole('button',{name:'Delete issue'}).click();assert.equal(await page.evaluate(()=>intents.at(-1).action.type),'delete');
      await page.evaluate(()=>dispatchEvent(new CustomEvent('lific:issue-detail-applied',{detail:{route:{issue_id:12,generation:4},kind:'deleted'}})));
      await page.getByRole('button',{name:'Restore issue'}).click();assert.equal(await page.evaluate(()=>intents.at(-1).action.type),'restore');
      await page.evaluate(()=>dispatchEvent(new CustomEvent('lific:issue-detail-applied',{detail:{route:{issue_id:99,generation:4},panel:'waits',issue:{waits:[]}}})));
      assert.equal(await page.locator('[data-wait-id="6"]').count(),1);
    });

    await t.test('comment window loads older rows by a stable timestamp and id cursor',async()=>{
      await page.evaluate(()=>{fixtureComments=Array.from({length:53},(_,i)=>({id:i+1,issue_id:12,user_id:4,author:'Sam',author_display_name:'Sam User',content:`Thread row ${i+1}`,created_at:new Date(Date.UTC(2026,0,1,0,0,i)).toISOString(),updated_at:new Date(Date.UTC(2026,0,1,0,0,i)).toISOString()}));return document.querySelector('[data-topcoat-collaboration]')._issueCollaboration.refresh();});
      await page.waitForFunction(()=>document.querySelector('[data-comment-count]').textContent==='50'&&!document.querySelector('[data-comments-older]').hidden);
      await page.getByRole('button',{name:'Load older comments'}).click();
      await page.waitForFunction(()=>document.querySelector('[data-comment-count]').textContent==='53'&&document.querySelector('[data-comments-older]').hidden);
      assert.equal(await page.locator('[data-comment-id]').count(),53);
    });

    await t.test('role revocation hides every write affordance immediately',async()=>{
      await page.evaluate(()=>{fixtureAffordances={manage:false,edit:false,comment:false};dispatchEvent(new CustomEvent('lific:account-change'));});
      await page.waitForFunction(()=>document.querySelector('[data-comment-compose]').hidden&&document.querySelector('[data-relation-create]').hidden&&document.querySelector('[data-wait-create]').hidden&&document.querySelector('[data-issue-delete]').hidden);
      assert.equal(await page.locator('[data-relation-remove]').count(),0);assert.equal(await page.locator('[data-wait-clear]').count(),0);
    });

    await t.test('empty issue snapshots clear stale relationships and waits',async()=>{
      await page.evaluate(()=>dispatchEvent(new CustomEvent('lific:issue-detail-applied',{detail:{route:{issue_id:12,generation:4},panel:'relations',issue:{waits:[]}}})));
      assert.equal(await page.locator('[data-relation-target]').count(),0);assert.equal(await page.locator('[data-wait-id]').count(),0);
    });


    await page.evaluate(()=>document.querySelector('[data-topcoat-collaboration]')._issueCollaboration.dispose());
  } finally { await browser.close(); }
});

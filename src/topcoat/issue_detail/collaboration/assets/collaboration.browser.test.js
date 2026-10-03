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
      <form data-comment-compose><label>Write a comment<textarea data-comment-draft aria-label="Write a comment"></textarea></label><div class="tc-collab__upload-tools"><label>Attach files<input type="file" multiple data-comment-files="new"></label><span data-comment-upload-status="new"></span></div><div data-mention-list hidden></div><button type="submit">Comment</button></form>
      <ul data-relation-list></ul><form data-relation-create><input name="target"><select name="kind"><option value="blocks">Blocks</option><option value="relates_to">Relates to</option><option value="duplicate">Duplicate</option></select><button type="submit">Link issue</button></form>
      <ul data-wait-list></ul><form data-wait-create><select name="kind"><option value="user">A person</option><option value="date">A date</option></select><label data-wait-user-field>Username<input name="user"></label><label data-wait-date-field hidden>From<input name="from"></label><label data-wait-until-field hidden>Until<input name="until"></label><input name="note"><button type="submit">Add wait</button></form>
      <div data-issue-attachments></div><ol data-issue-history></ol><button data-issue-delete>Delete issue</button><button data-issue-restore hidden>Restore issue</button><p data-collab-status></p><textarea data-description-editor aria-label="Issue description"></textarea></main></section></body></html>`;
    let comments = [{id:3,issue_id:12,user_id:4,author:'Sam',author_display_name:'Sam User',content:'Existing comment @sam TEAM_ALPHA-22 /api/attachments/8',created_at:'2026-10-01T10:00:00Z',updated_at:'2026-10-01T10:00:00Z'}];
    await page.setContent(html);
    await page.evaluate(() => {
      window.intents=[];window.fixtureCommentRequests=0; window.confirm=()=>true;
      window.fixtureAffordances={manage:true,edit:true,comment:true};window.lificSession={state:{user:{id:4},publicProject:null,role:{role:'maintainer',enforced:true,is_admin:false}},affordances:()=>lificSession.state.publicProject?{manage:false,edit:false,comment:false}:fixtureAffordances,scopedRoute:path=>lificSession.state.publicProject?`/public${path}`:path,resolve:path=>({url:`/api${path}`}),request:async path=>{
        if(path.includes('/mention-candidates')) return {ok:true,data:[{user_id:9,username:'maria',display_name:'Maria Jones'}]};
        if(path.includes('/comments?')) {window.fixtureCommentRequests++;const query=new URLSearchParams(path.split('?')[1]);let rows=window.fixtureComments.slice().sort((a,b)=>b.created_at.localeCompare(a.created_at)||b.id-a.id);if(query.has('before_created_at'))rows=rows.filter(row=>row.created_at<query.get('before_created_at')||(row.created_at===query.get('before_created_at')&&row.id<Number(query.get('before_id'))));return {ok:true,data:rows.slice(0,Number(query.get('limit')||51)),headers:{get:()=>null}};}
        if(path.includes('/activity?')) return {ok:true,data:{items:[{id:1,actor_display_name:'Sam',action:'created',ts:'2026-10-01T10:00:00Z'}]}};
        if(path.startsWith('/attachments?')) return {ok:true,data:[{id:8,filename:'plan.txt',mime:'text/plain',size_bytes:5,uploader_id:4,created_at:'2026-10-01'},{id:9,filename:'diagram.png',mime:'image/png',size_bytes:50,uploader_id:4},{id:10,filename:'clip.mp4',mime:'video/mp4',size_bytes:50,uploader_id:4},{id:11,filename:'voice.mp3',mime:'audio/mpeg',size_bytes:50,uploader_id:4},{id:12,filename:'change.diff',mime:'text/plain',size_bytes:50,uploader_id:4},{id:13,filename:'data.csv',mime:'text/csv',size_bytes:50,uploader_id:4},{id:14,filename:'bundle.zip',mime:'application/zip',size_bytes:50,uploader_id:4}]};
        return {ok:true,data:[]};
      }};
      window.fixtureUploadTargets=[];window.fixtureUploadNames=[];window.fixtureAttachmentDisposals=0;window.fixtureAttachmentMounts=0;
      window.LificTopcoatAttachments={viewerKind:item=>({png:'image',mp4:'video',mp3:'audio',diff:'diff',csv:'csv',zip:'zip',txt:'text'})[item.filename.split('.').pop()]||'file',markdown:item=>`[${item.filename}](/api/attachments/${item.id})`,createClient:({session})=>({session,url:id=>`/api/attachments/${id}`,upload:(file,options)=>{fixtureUploadTargets.push(options.target);fixtureUploadNames.push(file.name);return {result:new Promise(resolve=>setTimeout(()=>resolve({ok:true,data:{id:20,filename:file.name,mime:file.type||'text/plain',size:file.size}}),75)),abort(){}};},thumbnail:async()=>({ok:false,status:404,error:'missing'})}),attach:(target,options)=>{window.fixtureAttachmentOptions=options;fixtureAttachmentMounts++;return {dispose(){fixtureAttachmentDisposals++;const form=target.querySelector('[data-attachment-upload]');if(form){form.querySelector('[data-attachment-files]').disabled=true;form.querySelector('[type=submit]').disabled=true;form.querySelector('[data-attachment-cancel]').hidden=false;}for(const media of target.querySelectorAll('video,audio'))media.removeAttribute('src');}};}};
      window.fixtureComments=[];window.addEventListener('lific:issue-detail-intent',event=>intents.push(event.detail));
    });
    await page.addScriptTag({content:script});
    await page.evaluate(rows=>{fixtureComments=rows;const root=document.querySelector('[data-topcoat-collaboration]');LificTopcoatIssueCollaboration.mount(root,{route:{issue_id:12,generation:4},issue:{id:12,project_id:7,identifier:'ENG-4',blocks:['ENG-5'],blocked_by:['ENG-3'],relates_to:[],duplicates:[],duplicated_by:[],waits:[{id:5,kind:'user',username:'maria',display_name:'Maria',state:'holding',note:'Review'}]},capabilities:{edit:true,comment:true}});},comments);
    await page.getByText('Existing comment').waitFor(); await page.getByText('Waiting for Maria').waitFor();
    await page.getByText('plan.txt').waitFor(); await page.getByText('created', {exact:false}).waitFor();
    assert.equal(await page.locator('[data-issue-attachment-list] [data-attachment-id]').count(),7);
    assert.equal(await page.locator('[data-attachment-kind="image"] [data-attachment-original]').count(),1);
    assert.equal(await page.locator('[data-attachment-kind="video"] video').count(),1);assert.equal(await page.locator('[data-attachment-kind="audio"] audio').count(),1);
    assert.equal(await page.locator('[data-attachment-kind="diff"] [data-attachment-preview]').count(),1);assert.equal(await page.locator('[data-attachment-kind="csv"] [data-attachment-preview]').count(),1);assert.equal(await page.locator('[data-attachment-kind="zip"] [data-attachment-preview]').count(),1);
    await page.evaluate(()=>{fixtureMarkdownCalls=0;window.lificIssueEditor={renderMarkdown:(node,source)=>{fixtureMarkdownCalls++;node.textContent=source;}};return document.querySelector('[data-topcoat-collaboration]')._issueCollaboration.refresh();});
    assert.ok(await page.evaluate(()=>fixtureMarkdownCalls)>0,'comments delegate to the shared Topcoat Markdown renderer');
    await page.evaluate(()=>{const body=document.createElement('div'),code=document.createElement('code'),anchor=document.createElement('a');body.dataset.referenceTest='';code.textContent='CODE-1 @code';anchor.href='/already';anchor.textContent='LINK-2 @link';body.append(code,document.createTextNode(' '),anchor,document.createTextNode(' OPS-3 @person'));document.querySelector('[data-topcoat-collaboration]').append(body);LificTopcoatIssueCollaboration.decorateCommentReferences(body);});
    assert.equal(await page.locator('code a, code .tc-comment__mention').count(),0,'issue references inside code stay literal');
    assert.equal(await page.locator('a[href="/already"]').count(),1,'existing anchors are not wrapped');
    assert.equal(await page.locator('a[href="/OPS/issues/OPS-3"]').count(),1,'cross-project references use their own project route');
    assert.equal(await page.locator('[data-reference-test] .tc-comment__mention').count(),1,'mention formatting applies only to plain references');
    assert.deepEqual(await page.evaluate(()=>fixtureAttachmentOptions.target),{entity_type:'issue',entity_id:12});
    await page.evaluate(async()=>fixtureAttachmentOptions.onUploaded());
    await page.locator('[data-issue-attachments] [data-attachment-id="8"]').waitFor();
    await page.evaluate(()=>document.querySelector('[data-topcoat-collaboration]')._issueCollaboration.refresh());
    assert.equal(await page.evaluate(()=>fixtureAttachmentDisposals),0,'refresh must leave the shared uploader mount alive');

    await t.test('relation links use the route project and retain public scope',async()=>{
      assert.equal(await page.locator('[data-relation-target="ENG-5"] a').getAttribute('href'),'/ENG/issues/ENG-5');
      await page.evaluate(()=>{lificSession.state.publicProject='TEAM_ALPHA';LificTopcoatIssueCollaboration.renderRelations(document.querySelector('[data-topcoat-collaboration]'));});
      assert.equal(await page.locator('[data-relation-target="ENG-5"] a').getAttribute('href'),'/public/ENG/issues/ENG-5');
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
      await page.getByRole('button',{name:'Edit',exact:true}).first().click();const edit=page.getByRole('textbox',{name:'Edit comment'});await edit.fill('Edited safely');await edit.evaluate(el=>el.setSelectionRange(3,7));await page.evaluate(()=>document.querySelector('[data-topcoat-collaboration]')._issueCollaboration.refresh());assert.equal(await edit.inputValue(),'Edited safely','background refresh must retain the active editor draft');assert.equal(await edit.evaluate(el=>el===document.activeElement),true,'refresh keeps focus in the inline editor');assert.deepEqual(await edit.evaluate(el=>[el.selectionStart,el.selectionEnd]),[3,7],'refresh preserves the editor selection');
      await page.evaluate(()=>document.querySelector('[data-topcoat-collaboration]')._issueCollaboration.update({id:12,project_id:7,identifier:'ENG-4',blocks:['ENG-5'],blocked_by:['ENG-3'],relates_to:[],duplicates:[],duplicated_by:[],waits:[]},{edit:true,comment:true}));assert.equal(await edit.evaluate(el=>el===document.activeElement),true,'issue updates preserve focused editor');assert.deepEqual(await edit.evaluate(el=>[el.selectionStart,el.selectionEnd]),[3,7],'issue updates preserve selection');
      await page.getByRole('button',{name:'Save comment'}).click();assert.equal(await page.evaluate(()=>intents.at(-1).action.operation),'edit_comment');
      await edit.fill('Typed after submit');
      await page.evaluate(()=>{const action=intents.at(-1).action;dispatchEvent(new CustomEvent('lific:issue-detail-applied',{detail:{route:{issue_id:12,generation:4},panel:'comments',operation:action.operation,action,issue:{id:12,waits:[]},mutation:{id:3,content:action.content}}}));});
      assert.equal(await page.getByRole('textbox',{name:'Edit comment'}).inputValue(),'Typed after submit','successful ack must not discard typing newer than the submitted draft');
      page.once('dialog',dialog=>{assert.match(dialog.message(),/1 issue reference/);assert.match(dialog.message(),/1 attachment reference/);assert.match(dialog.message(),/1 mention/);return dialog.accept();});await page.getByRole('button',{name:'Delete',exact:true}).first().click();assert.equal(await page.evaluate(()=>intents.at(-1).action.operation),'delete_comment');
    });

    await t.test('cancel edit aborts queued uploads and ignores the late completion after reopening',async()=>{
      await page.getByRole('button',{name:'Edit',exact:true}).first().click();const edit=page.getByRole('textbox',{name:'Edit comment'});await edit.fill('Draft to cancel');
      const before=await page.evaluate(()=>fixtureUploadNames.length);await page.locator('[data-comment-files="3"]').setInputFiles([
        {name:'cancel-active.txt',mimeType:'text/plain',buffer:Buffer.from('active')},
        {name:'cancel-queued.txt',mimeType:'text/plain',buffer:Buffer.from('queued')},
      ]);await page.waitForFunction(count=>fixtureUploadNames.length===count+1,before);
      await page.getByRole('button',{name:'Cancel'}).click();await page.getByRole('button',{name:'Edit',exact:true}).first().click();const reopened=page.getByRole('textbox',{name:'Edit comment'});await page.waitForTimeout(110);
      assert.equal(await reopened.inputValue(),'Existing comment @sam TEAM_ALPHA-22 /api/attachments/8','cancelled upload cannot overwrite the reopened editor');
      assert.equal(await page.evaluate(()=>fixtureUploadNames.length),before+1,'cancel drops the queued second file');assert.equal(await page.evaluate(()=>document.querySelector('[data-topcoat-collaboration]')._commentEdits.get(3).content),'Existing comment @sam TEAM_ALPHA-22 /api/attachments/8');
      await page.getByRole('button',{name:'Cancel'}).click();
    });

    await t.test('comment attachment picker inserts at the caret and gates submit until upload completes',async()=>{
      const draft=page.getByRole('textbox',{name:'Write a comment'});await draft.fill('left right');await draft.evaluate(el=>el.setSelectionRange(5,5));
      await page.locator('[data-comment-files="new"]').setInputFiles({name:'snippet.txt',mimeType:'text/plain',buffer:Buffer.from('hello')});
      assert.equal(await page.locator('[data-comment-compose] [type=submit]').isDisabled(),true);
      await page.waitForTimeout(100);assert.equal(await draft.inputValue(),'left [snippet.txt](/api/attachments/20) right');
      assert.equal(await page.evaluate(()=>fixtureUploadTargets.at(-1)),null,'comment uploads stay unlinked until the comment write reconciles markdown references');
      assert.equal(await page.locator('[data-comment-compose] [type=submit]').isDisabled(),false);
      await draft.evaluate(el=>{el.focus();el.setSelectionRange(0,4);const files=new DataTransfer();files.items.add(new File(['pasted'],'paste.txt',{type:'text/plain'}));const event=new Event('paste',{bubbles:true,cancelable:true});Object.defineProperty(event,'clipboardData',{value:files});el.dispatchEvent(event);});
      await page.waitForFunction(()=>document.querySelector('[data-comment-draft]').value.startsWith('[paste.txt](/api/attachments/20) '));
      await draft.evaluate(el=>{el.focus();el.setSelectionRange(el.value.length,el.value.length);const files=new DataTransfer();files.items.add(new File(['dropped'],'drop.txt',{type:'text/plain'}));const event=new Event('drop',{bubbles:true,cancelable:true});Object.defineProperty(event,'dataTransfer',{value:files});el.dispatchEvent(event);});
      await page.waitForFunction(()=>document.querySelector('[data-comment-draft]').value.endsWith('[drop.txt](/api/attachments/20)'));
      await draft.fill('Concurrent typing');const before=await page.evaluate(()=>fixtureUploadNames.length);await page.locator('[data-comment-files="new"]').setInputFiles({name:'concurrent.txt',mimeType:'text/plain',buffer:Buffer.from('pending')});await page.waitForFunction(count=>fixtureUploadNames.length===count+1,before);await draft.fill('Text changed during upload');await page.waitForFunction(()=>document.querySelector('[data-comment-draft]').value.includes('[concurrent.txt](/api/attachments/20)'));
      assert.match(await draft.inputValue(),/^Text changed during upload \[concurrent\.txt\]/,'upload completion inserts at the current selection without replacing concurrent typing');
    });

    await t.test('audience changes discard queued comment uploads and ignore late completions',async()=>{
      const draft=page.getByRole('textbox',{name:'Write a comment'}),before=await page.evaluate(()=>fixtureUploadNames.length);
      await page.locator('[data-comment-files="new"]').setInputFiles([
        {name:'active.txt',mimeType:'text/plain',buffer:Buffer.from('active')},
        {name:'queued.txt',mimeType:'text/plain',buffer:Buffer.from('queued')},
      ]);
      await page.waitForFunction(count=>fixtureUploadNames.length===count+1,before);
      await page.evaluate(()=>{lificSession.state.publicProject='TEAM_ALPHA';dispatchEvent(new CustomEvent('lific:scope-change'));});
      await page.waitForTimeout(120);
      assert.equal(await page.evaluate(()=>fixtureUploadNames.length),before+1,'only the active transfer started before audience revocation');
      assert.doesNotMatch(await page.evaluate(()=>document.querySelector('[data-comment-draft]').value),/active\.txt|queued\.txt/,'late completion cannot insert markdown after the audience changes');
      assert.equal(await page.evaluate(()=>document.querySelector('[data-topcoat-collaboration]')._commentUploads.size),0,'queued files are released on audience change');
      await page.evaluate(()=>{lificSession.state.publicProject=null;dispatchEvent(new CustomEvent('lific:scope-change'));});
      await page.waitForFunction(()=>document.querySelector('[data-comment-compose] [type=submit]').disabled===false);
      const editButton=page.getByRole('button',{name:'Edit',exact:true}).first();await editButton.click();
      const edit=page.getByRole('textbox',{name:'Edit comment'}),save=page.locator('[data-comment-save="3"]');
      const editUploadCount=await page.evaluate(()=>fixtureUploadNames.length);
      await page.locator('[data-comment-files="3"]').setInputFiles({name:'edit-active.txt',mimeType:'text/plain',buffer:Buffer.from('edit')});
      await page.waitForFunction(count=>fixtureUploadNames.length===count+1,editUploadCount);
      assert.equal(await save.isDisabled(),true);
      await page.evaluate(()=>{lificSession.state.publicProject='TEAM_ALPHA';dispatchEvent(new CustomEvent('lific:scope-change'));});
      await page.waitForTimeout(100);
      await page.evaluate(()=>{lificSession.state.publicProject=null;dispatchEvent(new CustomEvent('lific:scope-change'));});
      await page.waitForFunction(()=>document.querySelector('[data-comment-edit-draft="3"]')!==null);
      assert.equal(await page.evaluate(()=>document.querySelector('[data-topcoat-collaboration]')._commentEdits.get(3).uploading),false);
      assert.equal(await page.locator('[data-comment-save="3"]').isDisabled(),false,'save becomes available after the private scope returns');
      assert.equal(await edit.inputValue(),'Existing comment @sam TEAM_ALPHA-22 /api/attachments/8');
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
      const draft=page.locator('[data-comment-draft]');await draft.fill('Keep this comment draft');
      await page.evaluate(()=>document.querySelector('[data-topcoat-collaboration]')._issueCollaboration.update({id:12,project_id:7,identifier:'ENG-4',blocks:['ENG-5'],blocked_by:['ENG-3'],relates_to:[],duplicates:[],duplicated_by:[],waits:[{id:6,kind:'date',earliest:'2026-10-10',latest:'2026-10-12',state:'due'}]},{edit:true,comment:true}));
      assert.equal(await draft.inputValue(),'Keep this comment draft');assert.equal(await page.locator('[data-wait-id="6"]').count(),1);
      await page.getByRole('button',{name:'Edit',exact:true}).first().click();const edit=page.getByRole('textbox',{name:'Edit comment'});await edit.fill('Keep the restricted editor draft');
      await page.evaluate(()=>document.querySelector('[data-topcoat-collaboration]')._issueCollaboration.setCapabilities({edit:false,comment:false}));
      assert.equal(await draft.inputValue(),'Keep this comment draft');assert.equal(await page.locator('[data-comment-compose]').isHidden(),true);
      assert.equal(await edit.inputValue(),'Keep the restricted editor draft');assert.equal(await edit.evaluate(el=>el.readOnly),true);assert.equal(await page.getByRole('button',{name:'Save comment'}).isDisabled(),true);assert.equal(await page.locator('[data-comment-files="3"]').isDisabled(),true);
      await page.evaluate(()=>document.querySelector('[data-topcoat-collaboration]')._issueCollaboration.setCapabilities({edit:true,comment:true}));assert.equal(await edit.evaluate(el=>el.readOnly),false);assert.equal(await edit.inputValue(),'Keep the restricted editor draft');
      await page.getByRole('button',{name:'Cancel'}).click();
      await page.evaluate(()=>document.querySelector('[data-topcoat-collaboration]')._issueCollaboration.setCapabilities({edit:true,comment:true}));
      assert.equal(await draft.inputValue(),'Keep this comment draft');assert.equal(await page.locator('[data-comment-compose]').isHidden(),false);
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
      await page.evaluate(()=>{fixtureComments=fixtureComments.filter(row=>row.id!==1).map(row=>row.id===2?{...row,content:'Reconciled edit'}:row);return document.querySelector('[data-topcoat-collaboration]')._issueCollaboration.refresh();});
      await page.waitForFunction(()=>document.querySelector('[data-comment-count]').textContent==='52');
      assert.equal(await page.locator('#comment-1').count(),0,'background reconciliation removes comments deleted elsewhere');
      assert.match(await page.locator('#comment-2 [data-comment-content]').innerText(),/Reconciled edit/,'background reconciliation refreshes stale loaded comment bodies');
      assert.equal(await page.evaluate(()=>document.querySelector('[data-topcoat-collaboration]')._nextCommentCursor.id),2,'older cursor follows the refreshed oldest row');
      assert.equal(await page.locator('[data-comments-older]').isHidden(),true,'has-older follows the refreshed API window');
      await page.evaluate(()=>{fixtureCommentRequests=0;fixtureComments=Array.from({length:1200},(_,i)=>({id:i+1,issue_id:12,user_id:4,author:'Sam',author_display_name:'Sam User',content:`Bounded ${i+1}`,created_at:new Date(Date.UTC(2024,0,1,0,0,i)).toISOString(),updated_at:new Date(Date.UTC(2024,0,1,0,0,i)).toISOString()}));const root=document.querySelector('[data-topcoat-collaboration]');root._comments=[];root._commentsWindowExpanded=false;location.hash='#comment-1';return root._issueCollaboration.refresh();});
      await page.waitForTimeout(300);
      assert.equal(await page.evaluate(()=>fixtureCommentRequests),11,'hash lookup must stop after the initial page plus ten older pages');
      assert.equal(await page.locator('#comment-1').count(),0);
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


    const attachmentMounts=await page.evaluate(()=>fixtureAttachmentMounts);await page.evaluate(()=>document.querySelector('[data-topcoat-collaboration]')._issueCollaboration.dispose());
    await page.evaluate(()=>LificTopcoatIssueCollaboration.mount(document.querySelector('[data-topcoat-collaboration]'),{route:{issue_id:12,generation:5},issue:{id:12,project_id:7,identifier:'ENG-4',blocks:[],blocked_by:[],relates_to:[],duplicates:[],duplicated_by:[],waits:[]},capabilities:{edit:true,comment:true}}));
    await page.waitForFunction(count=>fixtureAttachmentMounts===count+1,attachmentMounts);await page.waitForFunction(()=>document.querySelectorAll('[data-issue-attachment-list] [data-attachment-id]').length===7);assert.equal(await page.evaluate(()=>fixtureAttachmentMounts),attachmentMounts+1,'remount after dispose attaches a fresh issue uploader');assert.notEqual(await page.locator('[data-attachment-id="10"] video').getAttribute('src'),null,'remount restores media URLs');assert.equal(await page.locator('[data-attachment-files]').isDisabled(),false,'remount restores upload controls');assert.equal(await page.locator('[data-attachment-upload] [type=submit]').isDisabled(),false);assert.equal(await page.locator('[data-attachment-cancel]').isHidden(),true);await page.evaluate(()=>document.querySelector('[data-topcoat-collaboration]')._issueCollaboration.dispose());
  } finally { await browser.close(); }
});

const {test}=require('node:test');
const assert=require('node:assert/strict');
const fs=require('node:fs');
const path=require('node:path');

test('headless pages keep scope, permissions, markdown safety, explicit saves and comment editing',{skip:!process.env.PLAYWRIGHT_EXECUTABLE_PATH},async t=>{
 const {chromium}=await import(path.resolve(__dirname,'../../../..','e2e/node_modules/playwright/index.mjs'));
 const browser=await chromium.launch({headless:true,executablePath:process.env.PLAYWRIGHT_EXECUTABLE_PATH});
 const page=await browser.newPage({viewport:{width:1100,height:850}});page.setDefaultTimeout(5000);const failures=[];page.on('pageerror',error=>failures.push(error.stack||error.message));
 const css=fs.readFileSync(`${__dirname}/pages.css`,'utf8');const js=fs.readFileSync(`${__dirname}/pages.js`,'utf8');
 const frame=`<!doctype html><html><head><style>${css}</style></head><body><main id="mount"></main></body></html>`;
 const list=`<section class="tc-pages" data-topcoat-pages="list" data-project-identifier="ENG" data-page-scope="private" aria-busy="true"><header><h1>Pages</h1><button data-page-create hidden>New page</button><details data-page-create-presets hidden><summary>New page as</summary><button data-page-create-preset="draft">Draft</button><button data-page-create-preset="active">Active</button><button data-page-create-preset="complete">Complete</button></details></header><p data-pages-status></p><div data-pages-error hidden></div><form data-pages-filters><input data-pages-search type="search"><select data-pages-status-filter><option value="__active" selected>Active</option><option value="">All</option><option>active</option><option>draft</option><option>complete</option><option>archived</option></select><select data-pages-folder><option value="">All</option></select><select data-pages-label-filter><option value="">All labels</option></select><button data-pages-folder-create hidden>New folder</button></form><nav data-pages-tabs><button data-pages-tab="browse" aria-current="page">Browse</button><button data-pages-tab="recent">Recent</button><button data-pages-tab="drafts">Drafts</button><button data-pages-tab="archived">Archived</button></nav><ul data-pages-folder-tree></ul><div data-pages-content></div><dialog data-pages-create-dialog><form data-pages-create-form><input name="title" required><select name="status"><option value="draft">Draft</option><option value="active">Active</option><option value="complete">Complete</option></select><select name="folder_id"></select><button value="cancel">Cancel</button><button type="submit">Create</button></form></dialog><dialog data-pages-peek-dialog><h2 data-pages-peek-title></h2><article data-pages-peek-content></article><a data-pages-peek-open>Open page</a><button data-pages-peek-close>Close</button></dialog></section>`;
 const detail=`<section class="tc-pages tc-page-detail" data-topcoat-pages="detail" data-project-identifier="ENG" data-page-id="1" data-page-scope="private" aria-busy="true"><p data-page-status-message></p><div data-page-error hidden></div><article data-page-content hidden><nav><a href="/ENG/pages">Pages</a><span data-page-folder-crumb></span></nav><input data-page-title><div><select data-page-lifecycle><option value="draft">Draft</option><option value="active">Active</option></select><button data-page-pin hidden></button><button data-page-export>Export Markdown</button><button data-page-delete hidden>Delete</button></div><label data-page-folder-control hidden>Folder <select data-page-folder></select></label><div data-page-labels></div><section data-page-editor><button data-page-edit hidden>Edit</button><button data-page-preview hidden>Edit Markdown</button><button data-page-save disabled>Save</button><button data-page-cancel hidden>Cancel</button><textarea data-page-body hidden></textarea><article data-page-preview-content></article><p data-page-save-status></p></section><section data-page-attachments><ul data-page-attachment-list></ul><label data-page-attachment-upload hidden><input data-page-files type="file" multiple></label><p data-page-attachment-status></p><div data-page-attachment-viewer hidden></div></section><section data-page-comments><ol data-page-comment-list></ol><button data-page-comments-older hidden>Load older comments</button><form data-page-comment-form><textarea name="content" required></textarea><input data-page-comment-files type="file" multiple><button type="submit">Comment</button></form></section><section><ol data-page-activity></ol></section></article></section>`;
 const setup=async(mode,publicMode=false,pageContent=null,roleValue=null,rowCount=1,targetHash='',skipReadyWait=false,pageProjectId=7,failThumbnail=false,delayAttachmentList=false,failOlderComments=false,delayImagePreview=false)=>{
  await page.setContent(frame);await page.locator('#mount').evaluate((el,{html,publicMode})=>el.innerHTML=publicMode?html.replace('data-page-scope="private"','data-page-scope="public"'):html,{html:mode==='list'?list:detail,publicMode});
  await page.addScriptTag({content:fs.readFileSync(`${__dirname}/../../attachments/assets/attachments.js`,'utf8')});
  await page.evaluate(({publicMode,pageContent,roleValue,rowCount,pageProjectId,failThumbnail,delayAttachmentList,failOlderComments,delayImagePreview})=>{
   window.rows=Array.from({length:rowCount},(_,index)=>({id:index+1,project_id:index===0?pageProjectId:7,identifier:`ENG-PG-${index+1}`,folder_id:2,title:index===0?'Page one':`Page ${index+1}`,content:index===0?(pageContent||'# Start\n\n<script>bad()</script>'):'Preview needle',preview:index===0?'Start':'Preview needle',status:['active','draft','archived'][index%3],pinned:index===0,labels:['docs']}));
   window.comments=[{id:9,page_id:1,user_id:3,author:'riley',author_display_name:'Riley',content:'Read **this** @riley',created_at:'2026-10-01T00:00:00Z'}];window.writes=[];window.failPageSave=false;window.delayComment=false;window.role=roleValue||{role:'maintainer',enforced:true,is_admin:false};
   window.lificSession={state:{publicProject:publicMode?'ENG':null,user:{id:3,is_admin:false}},request:async(path,options={})=>{
    writes.push([path,options.method||'GET',options.body]);
    if(path==='/projects')return {ok:true,data:[{id:7,identifier:'ENG'},{id:8,identifier:'OPS'}]};
    if(path==='/auth/me')return {ok:true,data:{is_admin:false}};
    if(path.endsWith('/my-role'))return {ok:true,data:role};
    if(path==='/projects/7/mention-candidates')return {ok:true,data:[{user_id:3,username:'riley',display_name:'Riley'}]};
    if(path==='/projects/7/index')return {ok:true,data:{pages:rows.map(({id,project_id,identifier,folder_id,title,status,pinned,labels,content})=>({id,project_id,identifier,folder_id,title,status,pinned,labels,preview:content.slice(0,200)}))}};
    if(path.startsWith('/pages?')){const params=new URLSearchParams(path.split('?')[1]);const offset=Number(params.get('offset')||0);const limit=Number(params.get('limit')||500);return {ok:true,data:rows.slice(offset,offset+limit)};}
    if(path==='/pages'&&options.method==='POST')return {ok:true,data:{id:2,project_id:7,identifier:'ENG-PG-2'}};
    if(path==='/folders'&&options.method==='POST')return {ok:true,data:{id:3,...JSON.parse(options.body)}};
    if(path==='/folders/2'&&options.method==='DELETE')return {ok:true,data:null};
    if(path==='/folders?project_id=7'||path==='/folders?project_id=8')return {ok:true,data:[{id:2,parent_id:null,name:'Guides'}]};
    if(path.startsWith('/attachments?'))return {ok:true,data:path.includes('entity_type=comment')?(path.includes('entity_id=9')?[{id:11,filename:'comment.txt',mime:'text/plain',size:4}]:[]):[{id:8,filename:'sample.webp',mime:'image/webp',size:4},{id:12,filename:'notes.txt',mime:'text/plain',size:4},{id:13,filename:'archive.zip',mime:'application/zip',size:4}]};
    if(path==='/labels?project_id=7'||path==='/labels?project_id=8')return {ok:true,data:[{name:'docs'},{name:'design'}]};
    if(path==='/labels'&&options.method==='POST')return {ok:true,data:{name:JSON.parse(options.body).name}};
    if(path.startsWith('/pages/1/comments?')){const params=new URLSearchParams(path.split('?')[1]);if(params.has('before_id')&&window.failOlderComments)return {ok:false,status:500,error:'Could not load older comments.'};return {ok:true,data:[...comments].reverse(),headers:new Headers({'x-comment-has-more':window.failOlderComments?'true':'false'})};}
    if(path==='/pages/1/activity?limit=100')return {ok:true,data:{items:[{action:'updated',created_at:'Today'}]}};
    if(path==='/pages/1'&&options.method==='PUT'){if(window.failPageSave)return {ok:false,status:500,error:'Server rejected the edit'};const patch=JSON.parse(options.body);if(window.delayPageSave)await new Promise(resolve=>window.releasePageSave=()=>{rows[0]={...rows[0],...patch};resolve();});else rows[0]={...rows[0],...patch};return {ok:true,data:rows[0]};}
    if(path==='/pages/1'&&window.lificSession.state.publicProject&&window.blockPublicDetail)return {ok:false,status:404,error:'No public page'};
    if(path==='/pages/1')return {ok:true,data:rows[0]};
    if(path==='/pages/1/comments'&&options.method==='POST'){const comment={id:10,page_id:1,user_id:3,author:'riley',content:JSON.parse(options.body).content,created_at:'2026-10-02T00:00:00Z'};if(window.delayComment)await new Promise(resolve=>window.releaseComment=()=>{comments.push(comment);resolve();});else comments.push(comment);return {ok:true,data:comment};}
    if(path.startsWith('/comments/')&&options.method==='PUT'){const id=Number(path.split('/')[2]);const submitted=JSON.parse(options.body).content;const save=()=>{const index=comments.findIndex(item=>item.id===id);const comment={...comments[index],content:submitted};comments[index]=comment;return comment;};if(window.delayCommentUpdate)return new Promise(resolve=>window.releaseCommentUpdate=()=>resolve({ok:true,data:save()}));return {ok:true,data:save()};}
    return {ok:false,status:404,error:`Unexpected ${options.method||'GET'} ${path}`};
   }};
   window.attachmentEvents=[];window.exportEvents=[];window.delayPageAttachmentList=delayAttachmentList;window.releaseAttachmentLists=[];window.failThumbnail=failThumbnail;window.failOlderComments=failOlderComments;window.delayImagePreview=delayImagePreview;window.delayCommentUpdate=false;window.LificTopcoatAttachments={createComposer:window.LificTopcoatAttachments?.createComposer,createClient:({session,win})=>({audience:()=>session.state.publicProject?'public:'+session.state.publicProject:'private:'+session.state.user.id,list:target=>{const path=`/attachments?${new URLSearchParams(target)}`;if(target.entity_type==='page'&&window.delayPageAttachmentList)return new Promise(resolve=>releaseAttachmentLists.push(()=>session.request(path).then(resolve)));return session.request(path);},streamDownload:async(id,{variant,open})=>{attachmentEvents.push([id,variant]);if(id===8&&window.delayImagePreview){await new Promise(resolve=>window.releaseImagePreview=resolve);window.delayImagePreview=false;}if(variant==='thumbnail'&&(id===12||window.failThumbnail))return {ok:false,error:'Thumbnail unavailable'};const destination=await open({filename:id===12?'notes.txt':'sample.webp',contentType:id===12?'text/plain':'image/webp'});await destination.write(new Uint8Array([1,2,3]));await destination.close();return {ok:true,filename:'sample.webp',contentType:'image/webp'};},text:async id=>({ok:true,text:`Text attachment ${id}`}),upload:(file,options)=>{attachmentEvents.push(['upload',file.name,options.target]);return {result:Promise.resolve({ok:true,data:{id:9,filename:file.name,mime:file.type}}),abort(){}};}} )};window.fetch=async url=>{exportEvents.push(url);return new Response('metadata\ntitle: Page one',{status:200,headers:{'Content-Disposition':'attachment; filename="ENG-PG-1.zip"'}})};
   window.addEventListener('lific:navigate',event=>window.navigatedTo=event.detail.href);
   window.confirm=()=>true;window.prompt=()=> 'Research';window.scrollTargets=[];Element.prototype.scrollIntoView=function(){window.scrollTargets.push(this.id);};
  },{publicMode,pageContent,roleValue,rowCount,pageProjectId,failThumbnail,delayAttachmentList,failOlderComments,delayImagePreview});
  if(targetHash)await page.evaluate(hash=>history.replaceState({},'',hash),targetHash);
  await page.evaluate(()=>{window.lificSession.resolve=(path,method='GET')=>({kind:window.lificSession.state.publicProject?'public':'private',url:`/api${path}`});});
  await page.addScriptTag({content:js});if(!skipReadyWait)await page.waitForFunction(()=>document.querySelector('[data-topcoat-pages]').getAttribute('aria-busy')==='false');
 };
 try{
  await t.test('project page list filters, folders and creates within the current project',async()=>{
   await setup('list');await page.getByRole('link',{name:'Page one'}).waitFor();assert.match(await page.locator('[data-pages-content]').innerText(),/Guides/);
   await page.getByRole('searchbox').fill('missing');assert.match(await page.locator('[data-pages-content]').innerText(),/No pages match/);await page.getByRole('searchbox').fill('');
   await page.getByRole('button',{name:'New page'}).click();await page.locator('[data-pages-create-form] [name="title"]').fill('New guide');await page.locator('[data-pages-create-form] [type="submit"]').click();
   await page.waitForFunction(()=>window.navigatedTo==='/ENG/pages/2');assert.ok(await page.evaluate(()=>writes.some(([path,method])=>path==='/pages'&&method==='POST')));
  });
  await t.test('public list reads its scoped index and keeps links in public scope',async()=>{
   await setup('list',true);const link=page.getByRole('link',{name:'Page one'});await link.waitFor();assert.equal(await link.getAttribute('href'),'/public/ENG/pages/1');assert.ok(await page.evaluate(()=>writes.some(([path])=>path==='/projects/7/index')));
  });
  await t.test('public page peek fetches complete page content beyond the index preview',async()=>{
   const fullContent=`# Public page\n\n${'Long public content. '.repeat(20)}FULL PAGE END`;
   await setup('list',true,fullContent);await page.getByRole('button',{name:'Peek Page one'}).click();
   await page.getByText('FULL PAGE END').waitFor();
   assert.ok(await page.evaluate(()=>writes.some(([path])=>path==='/pages/1')));
   assert.ok((await page.locator('[data-pages-peek-content]').innerText()).includes('FULL PAGE END'));
  });
  await t.test('page peek is available from the list and renders content safely',async()=>{
   await setup('list');await page.getByRole('button',{name:'Peek Page one'}).click();assert.equal(await page.locator('[data-pages-peek-dialog]').evaluate(dialog=>dialog.open),true);
   assert.equal(await page.locator('[data-pages-peek-content] script').count(),0);assert.match(await page.locator('[data-pages-peek-content]').innerText(),/bad\(\)/);
  });
  await t.test('account transition closes and clears a private page peek',async()=>{
   await setup('list');await page.getByRole('button',{name:'Peek Page one'}).click();
   await page.evaluate(()=>{lificSession.state.user=null;window.dispatchEvent(new Event('lific:session-change'));});
   await page.waitForFunction(()=>!document.querySelector('[data-pages-peek-dialog]').open);
   assert.equal(await page.locator('[data-pages-peek-title]').innerText(),'');
   assert.equal(await page.locator('[data-pages-peek-content]').innerText(),'');
   assert.equal(await page.locator('[data-pages-peek-open]').getAttribute('href'),null);
  });
  await t.test('creation presets persist the selected lifecycle status',async()=>{
   await setup('list');await page.locator('[data-page-create-presets] summary').click();await page.locator('[data-page-create-preset="active"]').click();await page.locator('[data-pages-create-form] [name="title"]').fill('Active page');await page.locator('[data-pages-create-form] [type="submit"]').click();
   const body=await page.evaluate(()=>writes.find(([path,method])=>path==='/pages'&&method==='POST')[2]);assert.equal(JSON.parse(body).status,'active');
  });
  await t.test('page lists paginate beyond 200 rows and restore tree, create, delete, touch move and label search',async()=>{
   await setup('list',false,null,null,401);await page.waitForFunction(()=>document.querySelector('[data-pages-status]').textContent==='401 pages');
   assert.equal(await page.evaluate(()=>rows.length),401);assert.ok(await page.evaluate(()=>writes.some(([path])=>path.includes('offset=200'))));assert.ok(await page.evaluate(()=>writes.some(([path])=>path.includes('offset=400'))));
   await page.getByRole('searchbox').fill('Previw needle');assert.ok(await page.locator('[data-pages-content] .tc-pages__row').count()>0);
   await page.getByRole('button',{name:'Add subfolder'}).click();await page.waitForFunction(()=>writes.some(([path,method])=>path==='/folders'&&method==='POST'));
   assert.equal(JSON.parse((await page.evaluate(()=>writes.find(([path,method])=>path==='/folders'&&method==='POST')[2]))).parent_id,2);
  });
  await t.test('default active lifecycle filter does not suppress the Archived tab',async()=>{
   await setup('list',false,null,null,3);assert.equal(await page.locator('[data-pages-content] .tc-pages__row').count(),2);
   await page.locator('[data-pages-tab="archived"]').click();assert.equal(await page.locator('[data-pages-content] .tc-pages__row').count(),1);assert.match(await page.locator('[data-pages-content]').innerText(),/archived/);
  });
  await t.test('page body stays editable until explicit save and failed save keeps draft',async()=>{
   await setup('detail');const body=page.locator('[data-page-body]');assert.match(await page.locator('[data-page-preview-content]').innerText(),/Start/);await page.locator('[data-page-edit]').click();await body.fill('## Draft\n\n<script>not executable</script>');
   assert.equal(await page.evaluate(()=>writes.filter(([path,method])=>path==='/pages/1'&&method==='PUT').length),0);
   await page.getByRole('button',{name:'Preview'}).click();assert.equal(await page.locator('[data-page-preview-content] script').count(),0);
   await page.getByRole('button',{name:'Save'}).click();await page.waitForFunction(()=>writes.some(([path,method])=>path==='/pages/1'&&method==='PUT'));
   assert.equal(await body.inputValue(),'## Draft\n\n<script>not executable</script>');assert.match(await page.locator('[data-page-save-status]').innerText(),/Saved/);
   await page.locator('[data-page-edit]').click();await body.fill('A draft that the server rejects');await page.evaluate(()=>window.failPageSave=true);
   await page.getByRole('button',{name:'Save'}).click();await page.waitForFunction(()=>document.querySelector('[data-page-save-status]').textContent.includes('Could not save'));
   assert.equal(await body.inputValue(),'A draft that the server rejects');
  });
  await t.test('metadata saves preserve an unsent comment draft',async()=>{
   await setup('detail');const composer=page.locator('[data-page-comment-form] textarea[name="content"]');await composer.fill('Unsaved comment draft');
   await page.locator('[data-page-title]').fill('Updated title');await page.locator('[data-page-title]').dispatchEvent('change');await page.waitForFunction(()=>writes.some(([path,method])=>path==='/pages/1'&&method==='PUT'));
   assert.equal(await composer.inputValue(),'Unsaved comment draft');
  });
  await t.test('cancel restores saved markdown and edits during save remain dirty',async()=>{
   await setup('detail');const body=page.locator('[data-page-body]');await page.locator('[data-page-edit]').click();await body.fill('Canceled draft');await page.locator('[data-page-cancel]').click();await page.locator('[data-page-edit]').click();assert.equal(await body.inputValue(),'# Start\n\n<script>bad()</script>');
   await body.fill('Submitted version');await page.evaluate(()=>window.delayPageSave=true);await page.getByRole('button',{name:'Save'}).click();await page.waitForFunction(()=>typeof window.releasePageSave==='function');await body.fill('Newer unsaved version');await page.evaluate(()=>window.releasePageSave());
   await page.waitForFunction(()=>document.querySelector('[data-page-save-status]').textContent.includes('Unsaved changes'));assert.equal(await body.inputValue(),'Newer unsaved version');assert.equal(await page.locator('[data-page-body]').isDisabled(),false);
  });
  await t.test('unenforced project pages stay editable and public detail links keep public scope',async()=>{
   await setup('detail',false,null,{role:null,enforced:false,is_admin:false});assert.equal(await page.locator('[data-page-edit]').isVisible(),true);
   await setup('detail',true);assert.equal(await page.locator('nav a').getAttribute('href'),'/public/ENG/pages');
  });
  await t.test('page detail derives permissions and breadcrumbs from the page project',async()=>{
   await setup('detail',false,null,null,1,'',false,8);assert.equal(await page.locator('nav a').getAttribute('href'),'/OPS/pages');
   assert.ok(await page.evaluate(()=>writes.some(([path])=>path==='/projects/8/my-role')));
  });
  await t.test('page attachment previews fetch thumbnail image bytes and uploads stay page scoped',async()=>{
   await setup('detail',false,'![sample](/attachments/8)',null,1,'#att8-L1-2');await page.locator('[data-page-preview-content] img').waitFor();
   assert.ok(await page.locator('[data-page-preview-content] img').getAttribute('src').then(src=>src.startsWith('blob:')));
   assert.deepEqual(await page.evaluate(()=>attachmentEvents[0]),[8,'thumbnail']);
   assert.equal(await page.locator('#attachment-8').getAttribute('class'),'tc-page-attachment--target');
   await page.locator('[data-page-files]').setInputFiles({name:'new.txt',mimeType:'text/plain',buffer:Buffer.from('new')});
   await page.waitForFunction(()=>document.querySelector('[data-page-attachment-status]').textContent==='Uploaded new.txt');
  });
  await t.test('attachment viewer supports text and reports archive preview status',async()=>{
   await setup('detail');
   await page.locator('#attachment-12 button').filter({hasText:'View'}).click();
   assert.equal(await page.locator('[data-page-attachment-viewer] pre').innerText(),'Text attachment 12');
   await page.locator('#attachment-13 button').filter({hasText:'View'}).click();
   assert.match(await page.locator('[data-page-attachment-status]').innerText(),/Preview unavailable for archive\.zip/);
   assert.equal(await page.locator('[data-page-attachment-viewer]').evaluate(viewer=>viewer.hidden),false);
  });
  await t.test('late attachment preview cannot replace a newer unsupported selection',async()=>{
   await setup('detail',false,null,null,1,'',false,7,false,false,false,true);
   await page.locator('#attachment-8 button').filter({hasText:'View'}).click();
   await page.waitForFunction(()=>typeof window.releaseImagePreview==='function');
   await page.locator('#attachment-13 button').filter({hasText:'View'}).click();
   const status=page.locator('[data-page-attachment-status]');
   assert.match(await status.innerText(),/Preview unavailable for archive\.zip/);
   await page.evaluate(()=>window.releaseImagePreview());await page.waitForTimeout(50);
   assert.equal(await page.locator('[data-page-attachment-viewer] img').count(),0);
   assert.match(await status.innerText(),/Preview unavailable for archive\.zip/);
  });
  await t.test('failed image thumbnails fall back to original bytes',async()=>{
   await setup('detail',false,'![sample](/attachments/8)',null,1,'',false,7,true);
   const image=page.locator('[data-page-preview-content] img');await image.waitFor();
   assert.ok((await image.getAttribute('src')).startsWith('blob:'));
   assert.deepEqual(await page.evaluate(()=>attachmentEvents.slice(0,2)),[[8,'thumbnail'],[8,'original']]);
  });
  await t.test('comment attachments render and comment deep links scroll to their target',async()=>{
   await setup('detail',false,null,null,1,'#comment-9');await page.locator('#comment-attachment-11').waitFor();
   await page.waitForFunction(()=>scrollTargets.includes('comment-9'));assert.equal(await page.locator('#comment-attachment-11').innerText(),'comment.txtView');
  });
  await t.test('deep-link pagination failure stops retrying and reveals the page with an error',async()=>{
   await setup('detail',false,null,null,1,'#comment-999',false,7,false,false,true);
   await page.locator('[data-page-content]').waitFor({state:'visible'});
   await page.locator('[data-page-error]').waitFor({state:'visible'});
   await page.waitForTimeout(50);
   assert.match(await page.locator('[data-page-error]').innerText(),/Could not load older comments/);
   assert.equal(await page.evaluate(()=>writes.filter(([path])=>path.startsWith('/pages/1/comments?')).length),2);
   assert.equal(await page.locator('[data-page-comments-older]').isVisible(),true);
  });
  await t.test('manual older-comment failure keeps a retry action visible',async()=>{
   await setup('detail',false,null,null,1,'',false,7,false,false,true);
   await page.locator('[data-page-content]').waitFor({state:'visible'});
   await page.locator('[data-page-comments-older]').click();
   await page.locator('[data-page-error]').waitFor({state:'visible'});
   assert.equal(await page.locator('[data-page-comments-older]').innerText(),'Retry loading older comments');
   assert.equal(await page.locator('[data-page-error] button').innerText(),'Retry older comments');
   await page.evaluate(()=>window.failOlderComments=false);await page.locator('[data-page-comments-older]').click();
   await page.waitForFunction(()=>document.querySelector('[data-page-comments-older]').hidden);
   assert.equal(await page.locator('[data-page-error]').isHidden(),true);
  });
  await t.test('stale private page load cannot reveal the page after scope changes',async()=>{
   await setup('detail',false,null,null,1,'',true,7,false,true);await page.waitForFunction(()=>releaseAttachmentLists.length===1);
   await page.evaluate(()=>{window.blockPublicDetail=true;lificSession.state.publicProject='ENG';window.dispatchEvent(new Event('lific:scope-change'));});
   await page.waitForFunction(()=>document.querySelector('[data-page-error]').hidden===false);await page.evaluate(()=>releaseAttachmentLists.shift()());
   await page.waitForTimeout(50);assert.equal(await page.locator('[data-page-content]').isVisible(),false);
  });
  await t.test('Control-S saves the active page edit',async()=>{
   await setup('detail');await page.locator('[data-page-edit]').click();const body=page.locator('[data-page-body]');await body.fill('Keyboard saved');await body.press('Control+s');
   await page.waitForFunction(()=>writes.some(([path,method])=>path==='/pages/1'&&method==='PUT'));assert.equal(JSON.parse((await page.evaluate(()=>writes.find(([path,method])=>path==='/pages/1'&&method==='PUT')[2]))).content,'Keyboard saved');
  });
  await t.test('export uses the existing page bundle endpoint with page identifier',async()=>{
   await setup('detail');await page.locator('[data-page-export]').click();await page.waitForFunction(()=>exportEvents.length===1);assert.equal(await page.evaluate(()=>exportEvents[0]),'/api/export/pages/ENG-PG-1');
  });
  await t.test('folder and label changes commit directly and retain page identity',async()=>{
   await setup('detail');await page.locator('[data-page-folder]').selectOption('2');
   await page.waitForFunction(()=>writes.some(([path,method])=>path==='/pages/1'&&method==='PUT'));
   await page.locator('[data-page-labels] input[name="name"]').fill('release-notes');await page.locator('[data-page-labels] button[type="submit"]').click();
   await page.waitForFunction(()=>writes.some(([path,method])=>path==='/labels'&&method==='POST'));
   const patches=await page.evaluate(()=>writes.filter(([path,method])=>path==='/pages/1'&&method==='PUT').map(([, ,body])=>JSON.parse(body)));
   assert.ok(patches.some(patch=>patch.folder_id===2));assert.ok(patches.some(patch=>patch.labels?.includes('release-notes')));
  });
  await t.test('comments resolve mentions, offer keyboard autocomplete, and preserve edit drafts',async()=>{
   await setup('detail');assert.equal(await page.locator('[data-page-comment-list] strong').innerText(),'this');assert.equal(await page.locator('[data-page-comment-list] [data-page-mention]').innerText(),'@Riley');
   const composer=page.locator('[data-page-comment-form] textarea');await composer.fill('Review @ri');await composer.press('Enter');assert.equal(await composer.inputValue(),'Review @riley ');
   await page.locator('#comment-9 button').filter({hasText:'Edit'}).click();const input=page.getByRole('textbox',{name:'Edit comment'});await input.fill('**edited**');
   assert.equal(await input.inputValue(),'**edited**');await page.locator('#comment-9 button').filter({hasText:'Save'}).click();await page.locator('[data-page-comment-list] strong').waitFor();assert.equal(await page.locator('[data-page-comment-list] strong').innerText(),'edited');
   await composer.fill('Thanks @ri');await composer.press('Enter');await page.locator('[data-page-comment-form] button[type="submit"]').click();
   await page.waitForFunction(()=>writes.some(([path,method])=>path==='/pages/1/comments'&&method==='POST'));
  });
  await t.test('comment edit acknowledgement preserves newer text typed during the request',async()=>{
   await setup('detail');await page.locator('#comment-9 button').filter({hasText:'Edit'}).click();
   const input=page.getByRole('textbox',{name:'Edit comment'});await input.fill('Submitted edit');await page.evaluate(()=>window.delayCommentUpdate=true);
   await page.locator('#comment-9 button').filter({hasText:'Save'}).click();await page.waitForFunction(()=>typeof window.releaseCommentUpdate==='function');
   await input.fill('Newer unsaved edit');await page.evaluate(()=>window.releaseCommentUpdate());
   await page.waitForFunction(()=>document.querySelector('[data-comment-save-status]').textContent.includes('Newer changes remain unsaved'));
   assert.equal(await input.inputValue(),'Newer unsaved edit');
  });
  await t.test('comment submission keeps newer text and files typed while the POST is pending',async()=>{
   await setup('detail');const composer=page.locator('[data-page-comment-form] textarea[name="content"]');const files=page.locator('[data-page-comment-files]');await composer.fill('Submitted comment');await files.setInputFiles({name:'submitted.txt',mimeType:'text/plain',buffer:Buffer.from('first')});
   await page.evaluate(()=>window.delayComment=true);await page.locator('[data-page-comment-form] button[type="submit"]').click();await page.waitForFunction(()=>typeof window.releaseComment==='function');
   await composer.fill('Newer unsent draft');await files.setInputFiles({name:'newer.txt',mimeType:'text/plain',buffer:Buffer.from('second')});await page.evaluate(()=>window.releaseComment());
   await page.locator('#comment-10').waitFor();assert.match(await composer.inputValue(),/^Newer unsent draft/);assert.equal(await files.evaluate(input=>input.files[0].name),'newer.txt');
  });
  await t.test('late comment save acknowledgement cannot replace a reopened editor',async()=>{
   await setup('detail');await page.locator('#comment-9 button').filter({hasText:'Edit'}).click();
   const first=page.getByRole('textbox',{name:'Edit comment'});await first.fill('Submitted edit');await page.evaluate(()=>window.delayCommentUpdate=true);
   await page.locator('#comment-9 button').filter({hasText:'Save'}).click();await page.waitForFunction(()=>typeof window.releaseCommentUpdate==='function');
   await page.locator('#comment-9 button').filter({hasText:'Cancel'}).click();
   await page.locator('#comment-9 button').filter({hasText:'Edit'}).click();
   const reopened=page.getByRole('textbox',{name:'Edit comment'});await reopened.fill('Replacement editor draft');
   await page.evaluate(()=>window.releaseCommentUpdate());await page.waitForTimeout(50);
   assert.equal(await reopened.inputValue(),'Replacement editor draft');
  });
  await t.test('public page render remains read-only and never sends writes',async()=>{
   await setup('detail',true);assert.equal(await page.locator('[data-page-edit]:visible').count(),0);assert.equal(await page.locator('[data-page-comment-form]').isVisible(),false);
   assert.equal(await page.evaluate(()=>writes.some(([,method])=>method!=='GET')),false);
  });
  await t.test('shared page composer inserts Markdown into the draft and comment uploads publish references once',async()=>{
   await setup('detail');await page.locator('[data-page-edit]').click();await page.locator('[data-page-body]').fill('Draft');
   await page.locator('[data-page-files]').setInputFiles({name:'notes.txt',mimeType:'text/plain',buffer:Buffer.from('notes')});
   await page.waitForFunction(()=>document.querySelector('[data-page-body]').value.includes('[notes.txt](/api/attachments/9)'));
   assert.equal(await page.evaluate(()=>writes.some(([,method])=>method==='PUT')),false);
   const comment=page.locator('[data-page-comment-form] textarea');await comment.fill('Comment');
   await page.locator('[data-page-comment-files]').setInputFiles({name:'comment.txt',mimeType:'text/plain',buffer:Buffer.from('comment')});
   await page.waitForFunction(()=>document.querySelector('[data-page-comment-form] textarea').value.includes('[comment.txt](/api/attachments/9)'));
   await page.locator('[data-page-comment-form] button[type=submit]').click();await page.locator('#comment-10').waitFor();
   assert.match(await page.evaluate(()=>comments.at(-1).content),/\[comment.txt\]\(\/api\/attachments\/9\)/);
   assert.equal(await page.evaluate(()=>attachmentEvents.filter(event=>event[0]==='upload'&&event[1]==='comment.txt').length),1);
  });
  assert.deepEqual(failures,[]);
 }finally{await browser.close();}
});

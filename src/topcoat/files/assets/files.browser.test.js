const {test}=require('node:test');
const assert=require('node:assert/strict');
const fs=require('node:fs');
const path=require('node:path');

test('headless project Files preserve scope, filters, paging, permissions, orphan management and download fallback',{skip:!process.env.PLAYWRIGHT_EXECUTABLE_PATH},async()=>{
 const {chromium}=await import(path.resolve(__dirname,'../../../..','e2e/node_modules/playwright/index.mjs'));
 const browser=await chromium.launch({headless:true,executablePath:process.env.PLAYWRIGHT_EXECUTABLE_PATH});
 try{
  const page=await browser.newPage({viewport:{width:1150,height:850}});page.setDefaultTimeout(5000);const pageErrors=[];page.on('pageerror',error=>pageErrors.push(error.message));
  const js=fs.readFileSync(`${__dirname}/files.js`,'utf8');
  const attachments=fs.readFileSync(path.resolve(__dirname,'../../attachments/assets/attachments.js'),'utf8');
  const css=fs.readFileSync(`${__dirname}/files.css`,'utf8');
  const html=`<main><section class="tc-files" data-topcoat-files data-project-identifier="ENG" aria-busy="true">
   <p data-files-status></p><div data-files-error hidden></div><div data-files-filters>
    ${['','image','video','audio','text','pdf','archive','other'].map(v=>`<button data-files-mime="${v}">${v||'All'}</button>`).join('')}
    <select data-files-uploader><option value="">All uploaders</option></select><select data-files-sort><option value="created_at">Newest</option><option value="size">Largest</option><option value="filename">Filename</option></select>
   </div><span data-files-count></span><span data-files-bytes></span><div data-files-list></div><button data-files-more hidden>Load more</button>
   <button data-files-orphans-toggle aria-expanded="false">Unlinked</button><span data-files-orphan-count></span><div data-files-orphans hidden></div>
   <dialog data-files-viewer><h2 data-files-viewer-title></h2><p data-files-viewer-status></p><div data-files-viewer-content></div><a data-files-download>Download</a><button data-files-viewer-close>Close</button></dialog>
  </section></main>`;
  const rows=Array.from({length:52},(_,i)=>({id:i+1,filename:i===0?'01-image.png':i===1?'00-notes.txt':`file-${String(i+1).padStart(2,'0')}.png`,mime:i===1?'text/plain':'image/png',mime_class:i===1?'text':'image',size_bytes:i+1,uploader_id:i===0?4:8,uploader:i===0?'riley':'sam',uploader_display_name:i===0?'Riley':'Sam',created_at:`2026-01-${String((i%28)+1).padStart(2,'0')}T00:00:00Z`,entities:i===0?[]:i===2?[{entity_type:'issue',entity_id:3,identifier:'ENG-3',title:'Linked issue'},{entity_type:'comment',entity_id:77,identifier:'OTHER-DOC-17',title:'Comment on page',page_id:17}]:[{entity_type:'issue',entity_id:3,identifier:'ENG-3',title:'Linked issue'}]}));
  const orphans=[{id:90,filename:'loose.txt',mime:'text/plain',size_bytes:4,uploader_id:8,uploader:'sam',seconds_until_sweep:3601}];
  await page.setContent(`<style>${css}</style>${html}`);
  await page.evaluate(({rows,orphans,attachments,js})=>{
   Object.defineProperty(window,'localStorage',{configurable:true,value:{getItem(){return null;},setItem(){},removeItem(){}}});
   window.apiEvents=[];window.deleted=[];window.role={role:'maintainer',enforced:true,is_admin:false};window.holdImageFilter=false;window.filterPending=[];
   window.lificSession={state:{publicProject:null,user:{id:4}},resolve(path,method='GET'){return {kind:'private',url:`/api${path}`};},clearSession(){},request:async(path,options={})=>{
    const method=options.method||'GET';apiEvents.push([path,method]);
    if(path==='/projects')return {ok:true,data:[{id:7,identifier:'ENG'}]};
    if(path==='/projects/7/my-role')return {ok:true,data:role};
    if(path==='/auth/me')return {ok:true,data:{id:4,is_admin:false}};
    if(path.startsWith('/projects/7/attachments?')){const q=new URLSearchParams(path.split('?')[1]);const filtered=rows.filter(r=>(!q.has('mime_class')||r.mime_class===q.get('mime_class'))&&(!q.has('uploader')||r.uploader===q.get('uploader')));if(q.get('sort')==='filename')filtered.sort((a,b)=>a.filename.localeCompare(b.filename));const offset=Number(q.get('offset')||0),limit=Number(q.get('limit')||50);const data={items:filtered.slice(offset,offset+limit),total_count:filtered.length,total_bytes:filtered.reduce((n,r)=>n+r.size_bytes,0),has_more:offset+limit<filtered.length};if(q.get('mime_class')==='image'&&window.holdImageFilter)await new Promise(resolve=>window.filterPending.push(resolve));return {ok:true,data};}
    if(path==='/projects/7/attachments/orphans')return {ok:true,data:{items:orphans,total_bytes:4,grace_seconds:86400}};
    if(path==='/attachments/3/links')return {ok:true,data:{entities:[{entity_type:'issue',entity_id:3,identifier:'ENG-3',title:'Linked issue'},{entity_type:'comment',entity_id:77,identifier:null,title:'Comment excerpt'}],duplicates:[{attachment_id:4,filename:'copy.png',entities:[{entity_type:'issue',entity_id:4,identifier:'OTHER-4',title:'Duplicate issue'},{entity_type:'page',entity_id:17,identifier:'OTHER-DOC-17',title:'Duplicate page'}]}]}};
    if(path.endsWith('/links'))return {ok:true,data:{entities:[],duplicates:[]}};
    if(path==='/attachments/90'&&method==='DELETE'||path==='/attachments/1'&&method==='DELETE'){deleted.push(path);return {ok:true,data:{deleted:true}};}
    return {ok:false,status:404,error:`Unexpected ${method} ${path}`};
   }};
   window.fetch=async(url,options={})=>{
    if(String(url).includes('/attachments/1/thumbnail'))return new Response('missing',{status:404});
    if(String(url).includes('/attachments/1'))return new Response(new Uint8Array([137,80,78,71]),{status:200,headers:{'Content-Type':'image/png','Content-Length':'4'}});
    if(String(url).includes('/attachments/2'))return new Response('sample text',{status:200,headers:{'Content-Type':'text/plain','Content-Length':'11'}});
    return new Response('missing',{status:404});
   };
   eval(attachments+js);
  },{rows,orphans,attachments,js});
  await page.waitForFunction(()=>document.querySelector('[data-files-count]').textContent==='52 files');
  assert.equal(await page.locator('[data-file-id]').count(),50);
  assert.equal(await page.locator('[data-files-more]').isVisible(),true);
  await page.evaluate(()=>{window.dispatchEvent(new PageTransitionEvent('pagehide',{persisted:true}));window.dispatchEvent(new PageTransitionEvent('pageshow',{persisted:true}));});
  await page.locator('[data-files-more]').click();assert.equal(await page.locator('[data-file-id]').count(),52,'a BFCache-restored Files controller still handles pagination');
  await page.locator('[data-files-mime=""]').click();
  await page.waitForFunction(()=>document.querySelectorAll('[data-file-id]').length===50);
  await page.evaluate(()=>{window.holdImageFilter=true;window.filterPending=[];});
  await page.locator('[data-files-mime="image"]').click();await page.waitForFunction(()=>window.filterPending.length===1);
  assert.equal(await page.locator('[data-files-more]').isVisible(),false,'pagination stays unavailable while replacing the filtered page');
  await page.evaluate(()=>window.filterPending[0]());await page.waitForFunction(()=>document.querySelectorAll('[data-file-id]').length===50);
  await page.evaluate(()=>{window.holdImageFilter=false;});
  assert.equal(await page.locator('[data-files-count]').textContent(),'51 files');
  await page.locator('[data-files-uploader]').selectOption('sam');assert.equal(await page.locator('[data-file-id]').count(),50);
  await page.locator('[data-files-sort]').selectOption('filename');
  assert.equal(await page.locator('[data-file-id]').first().getAttribute('data-file-id'),'3');
  await page.locator('[data-files-expand="3"]').click();await page.getByRole('link',{name:'ENG-3'}).waitFor();
  assert.equal(await page.getByRole('link',{name:'ENG-3'}).getAttribute('href'),'/ENG/issues/ENG-3');
  assert.equal(await page.getByRole('link',{name:'OTHER-4'}).getAttribute('href'),'/OTHER/issues/OTHER-4');
  assert.equal(await page.getByRole('link',{name:'OTHER-DOC-17',exact:true}).getAttribute('href'),'/OTHER/pages/17');
  assert.equal(await page.getByRole('link',{name:'OTHER-DOC-17 (comment)'}).getAttribute('href'),'/OTHER/pages/17');
  assert.match(await page.locator('[data-file-id="3"]').innerText(),/Identical content also appears/);
  await page.locator('[data-files-orphans-toggle]').click();assert.equal(await page.locator('[data-files-orphans]').isVisible(),true);
  assert.match(await page.locator('[data-files-orphans]').innerText(),/swept in 1h/);
  assert.equal(await page.locator('[data-files-orphan-view="90"] + span').count(),1);
  assert.equal(await page.locator('[data-files-delete="90"]').count(),0,'a project maintainer cannot delete another uploader’s unlinked file');
  assert.equal(await page.locator('[data-files-delete="3"]').isVisible(),true,'the project maintainer can delete linked project files');
  await page.evaluate(()=>{window.role={role:'viewer',enforced:true,is_admin:false};window.lificSession.state.user.id=8;window.dispatchEvent(new Event('lific:session-change'));window.confirm=()=>true;});
  await page.waitForFunction(()=>document.querySelector('[data-files-delete="90"]')!==null);
  await page.locator('[data-files-delete="90"]').click();await page.waitForFunction(()=>deleted.length===1);
  await page.evaluate(()=>{window.role={role:'maintainer',enforced:true,is_admin:false};window.lificSession.state.user.id=4;window.dispatchEvent(new Event('lific:session-change'));});
  await page.waitForFunction(()=>document.querySelectorAll('[data-file-id]').length===50);
  await page.locator('[data-files-uploader]').selectOption('');await page.locator('[data-files-mime=""]').click();
  await page.waitForFunction(()=>document.querySelectorAll('[data-file-id]').length===50);
  assert.equal(await page.locator('[data-files-delete="1"]').isVisible(),true);
  await page.locator('[data-files-view="1"]').click();
  try{await page.waitForFunction(()=>document.querySelector('[data-files-viewer-status]').textContent!=='Loading preview…');}
  catch{throw new Error(`preview did not settle; status=${await page.locator('[data-files-viewer-status]').textContent()}, errors=${pageErrors.join('; ')}`);}
  await page.locator('[data-files-viewer] img').waitFor({state:'attached'});
  assert.equal(await page.locator('[data-files-viewer] img').getAttribute('alt'),'01-image.png');
  assert.equal(await page.locator('[data-files-download]').getAttribute('href'),'/api/attachments/1');
  await page.locator('[data-files-viewer-close]').click();
  assert.equal(await page.locator('[data-file-id="2"]').count(),1,`rows after preview close: ${await page.locator('[data-file-id]').allInnerTexts()}`);
  await page.locator('[data-files-view="2"]').click();await page.locator('[data-files-viewer] pre').waitFor();
  assert.equal(await page.locator('[data-files-viewer] pre').innerText(),'sample text');
  await page.evaluate(()=>{window.role={role:'viewer',enforced:true,is_admin:false};window.lificSession.state.user.id=5;window.dispatchEvent(new Event('lific:session-change'));});
  await page.waitForFunction(()=>document.querySelectorAll('[data-file-id]').length===50);
  assert.equal(await page.locator('[data-files-viewer]').evaluate(dialog=>dialog.open),false);
  assert.equal(await page.locator('[data-files-viewer-title]').textContent(),'');
  assert.equal(await page.locator('[data-files-delete="1"]').isVisible(),false);
 }finally{await browser.close();}
});

test('headless Files previews keep structured data safe, sortable, lazy, and recoverable',{skip:!process.env.PLAYWRIGHT_EXECUTABLE_PATH},async()=>{
 const {chromium}=await import(path.resolve(__dirname,'../../../..','e2e/node_modules/playwright/index.mjs'));
 const browser=await chromium.launch({headless:true,executablePath:process.env.PLAYWRIGHT_EXECUTABLE_PATH});
 try{
  const page=await browser.newPage();const errors=[];page.on('pageerror',error=>errors.push(error.message));
  await page.setContent('<section class="tc-files" data-topcoat-files data-mounted="true" data-project-identifier="ENG"><p data-files-status></p><div data-files-error hidden></div><span data-files-count></span><span data-files-bytes></span><div data-files-list></div><button data-files-more></button><select data-files-uploader></select><span data-files-orphan-count></span><div data-files-orphans></div><dialog data-files-viewer><h2 data-files-viewer-title></h2><p data-files-viewer-status role="status"></p><div data-files-viewer-content></div><a data-files-download>Download original</a><button data-files-viewer-close>Close</button></dialog></section>');
  await page.evaluate(()=>{
   const rows=[['clip.mp4','video'],['clip.mp3','audio'],['table.csv','csv'],['table.tsv','csv'],['dump.json','json'],['change.patch','diff'],['bad.json','json']].map(([filename,kind],i)=>({id:i+1,filename,kind,entities:[],size_bytes:100,mime:'application/octet-stream',uploader_id:1,created_at:'2026-01-01'}));
   window.previewRows=rows;window.LificTopcoatAttachments={viewerKind:row=>row.kind};
   window.lificSession={state:{user:{id:1},publicProject:null},resolve:path=>({kind:'private',url:`/api${path}`}),request:async path=>({ok:true,data:path==='/projects'?[{id:7,identifier:'ENG'}]:path.includes('my-role')?{enforced:false}:path.includes('orphans')?{items:[]}:{items:rows,total_count:rows.length,total_bytes:700,has_more:false}})};
   window.previewText={3:'name,count,note\n<svg onload=bad()>,10,"comma,inside"\nsecond,2,"line\nbreak"',4:'name\tcount\nfirst\t10\nsecond\t2',5:JSON.stringify({nested:{unsafe:'<img src=x onerror=bad()>'},items:Array.from({length:205},(_,i)=>i)}),6:'--- a/change.txt\n+++ b/change.txt\n@@ -1 +1 @@\n-<script>old</script>\n+<img src=x onerror=bad()>\n',7:'{invalid'};
  });
  await page.addScriptTag({content:fs.readFileSync(`${__dirname}/files.js`,'utf8')});
  await page.evaluate(()=>{
   window.previewController=new LificTopcoatFiles.FilesController(document.querySelector('[data-topcoat-files]'),{attachmentClient:{text:async id=>({ok:true,text:previewText[id]})}});
  });
  await page.waitForFunction(()=>previewController.rows.length===7);
  for(const [id,kind] of [[1,'video'],[2,'audio']]){
   const media=await page.evaluate(async({id,kind})=>{await previewController.preview(id);const element=document.querySelector(`[data-files-viewer] ${kind}`);const initial={preload:element.preload,controls:element.controls};element.dispatchEvent(new Event('error'));return initial;},{id,kind});
   assert.deepEqual(media,{preload:'metadata',controls:true});assert.equal(await page.locator(`[data-files-viewer] ${kind}`).count(),0);
   assert.match(await page.locator('[data-files-viewer-status]').textContent(),/Playback not supported.*Download original/);assert.equal(await page.locator('[data-files-download]').getAttribute('href'),`/api/attachments/${id}`);
  }
  await page.evaluate(()=>previewController.preview(3));
  assert.deepEqual(await page.locator('[data-files-viewer] tbody tr').first().locator('td').allTextContents(),['<svg onload=bad()>','10','comma,inside']);
  await page.getByRole('button',{name:'Sort by count'}).click();assert.equal(await page.locator('[data-files-viewer] tbody tr').first().locator('td').nth(1).textContent(),'2');
  assert.equal(await page.getByRole('columnheader',{name:'Sort by count'}).getAttribute('aria-sort'),'ascending');
  await page.getByRole('button',{name:'Sort by count'}).click();assert.equal(await page.locator('[data-files-viewer] tbody tr').first().locator('td').nth(1).textContent(),'10');
  await page.evaluate(()=>previewController.preview(4));assert.deepEqual(await page.locator('[data-files-viewer] tbody tr').first().locator('td').allTextContents(),['first','10']);
  await page.evaluate(()=>previewController.preview(5));
  assert.equal(await page.locator('[data-files-viewer] [data-json-scalar]').count(),0);
  await page.locator('[data-files-viewer] summary').filter({hasText:/^nested:/}).click();assert.match(await page.locator('[data-json-scalar]').textContent(),/<img src=x onerror=bad\(\)>/);
  await page.locator('[data-files-viewer] summary').filter({hasText:/^items:/}).click();await page.waitForFunction(()=>document.querySelectorAll('[data-json-scalar]').length===101);assert.equal(await page.locator('[data-files-viewer] [data-json-scalar]').count(),101);
  await page.getByRole('button',{name:'Show 100 more items'}).click();assert.equal(await page.locator('[data-files-viewer] [data-json-scalar]').count(),201);
  await page.getByRole('button',{name:'Show 5 more items'}).click();assert.equal(await page.locator('[data-files-viewer] [data-json-scalar]').count(),206);
  await page.locator('[data-files-viewer] summary').filter({hasText:/^items:/}).click();await page.waitForFunction(()=>document.querySelectorAll('[data-json-scalar]').length===1);assert.equal(await page.locator('[data-files-viewer] [data-json-scalar]').count(),1);
  await page.evaluate(()=>previewController.preview(6));
  assert.equal(await page.locator('[data-files-viewer] [data-diff-kind="del"] code').textContent(),'- <script>old</script>');
  assert.equal(await page.locator('[data-files-viewer] [data-diff-kind="add"] code').textContent(),'+ <img src=x onerror=bad()>');
  assert.match(await page.locator('[data-files-viewer-content]').textContent(),/1 file changed, \+1 -1/);
  await page.evaluate(()=>previewController.preview(7));assert.match(await page.locator('[data-files-viewer-status]').textContent(),/Invalid JSON.*Download original/);
  assert.equal(await page.locator('[data-files-viewer] script,img,svg').count(),0);assert.deepEqual(errors,[]);
 }finally{await browser.close();}
});

const {test}=require('node:test');
const assert=require('node:assert/strict');
const fs=require('node:fs');
const path=require('node:path');

test('headless issue creation loads project metadata, enforces roles and keeps failed drafts',{skip:!process.env.PLAYWRIGHT_EXECUTABLE_PATH},async t=>{
 const {chromium}=await import(path.resolve(__dirname,'../../../..','e2e/node_modules/playwright/index.mjs'));
 const browser=await chromium.launch({headless:true,executablePath:process.env.PLAYWRIGHT_EXECUTABLE_PATH});
 const page=await browser.newPage();page.setDefaultTimeout(5000);const errors=[];page.on('pageerror',error=>errors.push(error.message));
 const html=`<!doctype html><html><head><style>${fs.readFileSync(`${__dirname}/issue-create.css`,'utf8')}</style></head><body><section class="tc-issue-create" data-topcoat-issue-create data-project-identifier="ENG" aria-busy="true" aria-label="Create issue"><div data-issue-create-loading role="status">Loading project…</div><div data-issue-create-denied hidden><h1>You can't create issues here</h1><a data-issue-create-back>Back to issues</a></div><form data-issue-create-form hidden><header class="tc-issue-create__header"><a data-issue-create-back>Issues</a><h1>New issue</h1><p data-issue-create-error role="alert"></p><button type="button" data-issue-create-discard>Discard</button><button type="submit" data-issue-create-submit disabled>Create issue</button></header><main class="tc-issue-create__body"><input data-issue-create-title><textarea data-issue-create-description></textarea><input type="file" multiple data-issue-create-files><span data-issue-create-upload-status></span><ul data-issue-create-uploads></ul><select data-issue-create-status><option value="backlog">Backlog</option><option value="done">Done</option></select><select data-issue-create-priority><option value="none">No priority</option><option value="high">High</option></select><select data-issue-create-module><option value="">None</option></select><div data-issue-create-labels></div><input data-issue-create-label-name><input type="color" data-issue-create-label-color value="#6b7280"><button type="button" data-issue-create-label-submit>Create label</button></main></form><div data-issue-create-load-error hidden><p data-issue-create-load-message></p><button data-issue-create-retry>Try again</button><a data-issue-create-back>Back to issues</a></div></section></body></html>`;
 await page.route('http://localhost/**',route=>route.fulfill({contentType:'text/html',body:html}));
 await page.goto('http://localhost/ENG/issues/new?module=7&status=done');
 await page.evaluate(()=>{
  window.calls=[];window.role={role:'maintainer',enforced:true,is_admin:false};window.createFailure=true;
  window.lificSession={state:{user:{id:4},role},request:async(path,options={})=>{
   calls.push({path,options});if(path==='/projects')return {ok:true,data:[{id:9,identifier:'ENG'}]};
   if(path.endsWith('/my-role'))return {ok:true,data:role};if(path.startsWith('/modules'))return {ok:true,data:[{id:7,name:'Engine'},{id:8,name:'UX'}]};
   if(path.startsWith('/labels')&&options.method==='POST')return {ok:true,data:{id:99,name:'launch',color:JSON.parse(options.body).color}};
   if(path.startsWith('/labels'))return {ok:true,data:[{id:1,name:'bug'},{id:2,name:'urgent'}]};if(path==='/issues')return window.createFailure?{ok:false,error:'Try again'}:{ok:true,data:{identifier:'ENG-12'}};
   return {ok:false,error:`Unexpected ${path}`};
  }};
  window.uploadAttempts=0;window.failNextUpload=false;
  window.LificTopcoatAttachments={markdown:item=>`![${item.filename}](/api/attachments/${item.id})`,createClient:()=>({upload(file,{onProgress}={}){window.uploadAttempts++;let resolve;const result=new Promise(done=>resolve=done);onProgress?.({loaded:1,total:3});window.finishUpload=()=>resolve(window.failNextUpload?{ok:false,error:'Network failed'}:{ok:true,data:{id:22,filename:file.name,mime:'image/png'}});return {result,abort(){window.uploadAborted=true;resolve({ok:false,canceled:true,error:'Upload canceled.'});}};}})};
 });
 await page.addScriptTag({content:fs.readFileSync(`${__dirname}/issue-create.js`,'utf8')});await page.locator('[data-issue-create-form]').waitFor({state:'visible'});
 await t.test('query defaults and selected project metadata appear in the form',async()=>{
  assert.equal(await page.locator('[data-issue-create-status]').inputValue(),'done');assert.equal(await page.locator('[data-issue-create-module]').inputValue(),'7');
  assert.equal(await page.locator('[data-issue-create-labels] input').count(),2);assert.equal(await page.getByRole('button',{name:'Create issue'}).isDisabled(),true);
 });
 await t.test('upload insertion uses the current selection after textarea edits and title focus',async()=>{
  await page.locator('[data-issue-create-title]').fill('A new issue');await page.locator('[data-issue-create-description]').fill('BeforeXafter');
  await page.locator('[data-issue-create-description]').evaluate(el=>{el.focus();el.setSelectionRange(6,7);});
  await page.locator('[data-issue-create-files]').evaluate(el=>el.dispatchEvent(new MouseEvent('click',{bubbles:true})));
  await page.locator('[data-issue-create-files]').setInputFiles({name:'screen.png',mimeType:'image/png',buffer:Buffer.from('png')});
  assert.equal(await page.getByRole('button',{name:'Create issue'}).isDisabled(),true);
  await page.locator('[data-issue-create-description]').fill('Prefix BeforeXafter');
  await page.locator('[data-issue-create-description]').evaluate(el=>{el.focus();el.setSelectionRange(13,14);});
  await page.locator('[data-issue-create-title]').evaluate(el=>el.focus());
  assert.equal(await page.evaluate(()=>document.activeElement===document.querySelector('[data-issue-create-title]')),true);
  await page.evaluate(()=>finishUpload());
  await page.waitForFunction(()=>document.querySelector('[data-issue-create-description]').value.includes('screen.png'));
  const markdown='![screen.png](/api/attachments/22)';
  assert.equal(await page.locator('[data-issue-create-description]').inputValue(),`Prefix Before\n${markdown}\nafter`);
  assert.equal(await page.locator('[data-issue-create-description]').evaluate(el=>el.selectionStart),14+markdown.length);
  await page.locator('[data-issue-create-labels] input').first().check();
  await page.locator('[data-issue-create-label-name]').fill('launch');await page.locator('[data-issue-create-label-color]').evaluate(el=>{el.value='#123456';});
  await page.getByRole('button',{name:'Create label'}).click();await page.waitForFunction(()=>document.querySelector('[data-issue-create-labels]').textContent.includes('launch'));
  assert.equal(await page.locator('[data-issue-create-labels] input[value="launch"]').isChecked(),true);
  await page.getByRole('button',{name:'Create issue'}).click();
  await page.waitForFunction(()=>document.querySelector('[data-issue-create-error]').textContent==='Try again');
  assert.equal(await page.locator('[data-issue-create-title]').inputValue(),'A new issue');
  await page.evaluate(()=>window.createFailure=false);
  await page.getByRole('button',{name:'Create issue'}).click();
  await page.waitForFunction(()=>location.hash.includes('/ENG/issues/ENG-12'));
  const body=JSON.parse((await page.evaluate(()=>calls.find(call=>call.path==='/issues').options.body)));
  assert.deepEqual(body,{project_id:9,title:'A new issue',description:`Prefix Before\n${markdown}\nafter`,status:'done',priority:'none',labels:['bug','launch'],module_id:7});
 });
 await t.test('per-file upload progress, cancellation and retry keep failures recoverable',async()=>{
  await page.evaluate(()=>window.failNextUpload=true);
  await page.locator('[data-issue-create-files]').setInputFiles({name:'retry.txt',mimeType:'text/plain',buffer:Buffer.from('retry')});
  const row=page.locator('[data-issue-create-uploads] [data-upload-id]').last();
  await page.waitForFunction(()=>window.uploadAttempts===2);await page.evaluate(()=>finishUpload());
  await page.waitForFunction(()=>document.querySelector('[data-issue-create-uploads]').textContent.includes('Failed: Network failed'));
  assert.equal(await row.getByRole('button',{name:'Retry retry.txt'}).isVisible(),true);
  await page.evaluate(()=>window.failNextUpload=false);await row.getByRole('button',{name:'Retry retry.txt'}).click();
  await page.waitForFunction(()=>document.querySelector('[data-issue-create-uploads]').textContent.includes('Uploading 33%'));
  assert.equal(await row.getByRole('progressbar',{name:'Upload progress for retry.txt'}).getAttribute('value'),'33');
  await row.getByRole('button',{name:'Cancel retry.txt'}).click();
  await page.waitForFunction(()=>document.querySelector('[data-issue-create-uploads]').textContent.includes('Canceled; retry available'));
  await row.getByRole('button',{name:'Retry retry.txt'}).click();await page.waitForFunction(()=>window.uploadAttempts===4);
  await page.evaluate(()=>finishUpload());
  await page.waitForFunction(()=>document.querySelector('[data-issue-create-uploads]').textContent.includes('Uploaded'));
 });
 await t.test('discard aborts an active upload',async()=>{
  await page.locator('[data-issue-create-files]').setInputFiles({name:'pending.txt',mimeType:'text/plain',buffer:Buffer.from('pending')});await page.getByRole('button',{name:'Discard'}).click();
  assert.equal(await page.evaluate(()=>uploadAborted),true);assert.match(await page.evaluate(()=>location.hash),/ENG\/issues$/);
 });
 assert.deepEqual(errors,[]);await browser.close();
});

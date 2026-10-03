const {test}=require('node:test');
const assert=require('node:assert/strict');
const fs=require('node:fs');
const path=require('node:path');

test('headless public routes use only public reads, render scrubbed DTOs, and expose no mutations',{skip:!process.env.PLAYWRIGHT_EXECUTABLE_PATH},async()=>{
 const {chromium}=await import(path.resolve(__dirname,'../../../..','e2e/node_modules/playwright/index.mjs'));
 const browser=await chromium.launch({headless:true,executablePath:process.env.PLAYWRIGHT_EXECUTABLE_PATH});
 try{
  const page=await browser.newPage({viewport:{width:1100,height:850}});page.setDefaultTimeout(5000);const errors=[];page.on('pageerror',error=>errors.push(error.message));
  const js=fs.readFileSync(`${__dirname}/public.js`,'utf8');const css=fs.readFileSync(`${__dirname}/public.css`,'utf8');
  const attachments=fs.readFileSync(path.resolve(__dirname,'../../attachments/assets/attachments.js'),'utf8');
  const shell=`<style>${css}</style><main id="mount"></main>`;
  const issue={id:11,identifier:'ENG-1',title:'Public title',status:'active',priority:'high',description:'# Public body\n\n**safe** <script>bad()</script> ![chart](/attachments/31) [attachment download](/api/attachments/31)',seq:900,assignee_id:999,private_notes:'must not render'};
  const pageRow={id:22,title:'Public page',status:'active',content:'# Public page body\n\n*visible* <img src=x onerror=bad()> ![chart](/attachments/31)',private_owner_id:987};
  const comments=[{id:91,author:'writer',author_display_name:'Writer',content:'Public comment **visible** ![comment chart](/attachments/31)',created_at:'2026-01-02T00:00:00Z',user_id:800}];
  const requests=[];const transports=[];
  const mount=async(kind,identifier='')=>{
   await page.setContent(shell);await page.locator('#mount').evaluate((root,{kind,identifier})=>root.innerHTML=`<main class="tc-public" data-topcoat-public="${kind}" data-public-project="ENG" data-public-identifier="${identifier}" aria-busy="true" aria-readonly="true"><header><a href="/public/ENG/issues">ENG</a><h1>${kind}</h1></header><p data-public-status></p><div data-public-error hidden></div><section data-public-content hidden></section></main>`,{kind,identifier});
   await page.evaluate(({js,attachments})=>{
    Object.defineProperty(window,'localStorage',{configurable:true,value:{getItem(){return null;},setItem(){},removeItem(){}}});
    window.publicRequests=[];window.publicFetches=[];
    const project='ENG';
    window.lificSession={state:{publicProject:project,user:null},resolve(path,method='GET'){
      const allowed=method==='GET'&&(/^\/(projects|issues|pages|attachments)(\/|\?|$)/.test(path));
      return allowed?{kind:'public',url:`/public/api/projects/${project}${path==='/projects'?'':path}`}:{kind:'refused'};
    },request:async(path,options={})=>{
      window.publicRequests.push([path,options.method||'GET',options.credentials||'omit',options.headers||null,options.body||null]);
      const method=options.method||'GET';if(method!=='GET'||options.body||options.headers?.Authorization||options.headers?.Cookie)return {ok:false,status:403,error:'public write refused'};
      if(path==='/projects')return {ok:true,data:[{id:7,identifier:'ENG',name:'Engineering'}]};
      if(path==='/projects/7/index')return {ok:true,data:{issues:[{id:11,identifier:'ENG-1',title:'Public title',status:'active',priority:'high'}],pages:[{id:22,title:'Public page',status:'active',preview:'Public preview'}]}};
      if(path==='/issues/resolve/ENG-1')return {ok:true,data:{id:11,identifier:'ENG-1'}};
      if(path==='/issues/11')return {ok:true,data:{id:11,identifier:'ENG-1',title:'Public title',status:'active',priority:'high',description:'# Public body\n\n**safe** <script>bad()</script> ![chart](/attachments/31)'}};
      if(path.startsWith('/issues/11/comments?'))return {ok:true,data:[{id:91,author:'writer',author_display_name:'Writer',content:'Public comment **visible**',created_at:'2026-01-02T00:00:00Z'}],headers:new Headers({'x-comment-has-more':'false'})};
      if(path==='/pages/22')return {ok:true,data:{id:22,title:'Public page',status:'active',content:'# Public page body\n\n*visible* <img src=x onerror=bad()> ![chart](/attachments/31)'}};
      if(path.startsWith('/pages/22/comments?'))return {ok:true,data:[{id:91,author:'writer',author_display_name:'Writer',content:'Public comment **visible**',created_at:'2026-01-02T00:00:00Z'}],headers:new Headers({'x-comment-has-more':'false'})};
      if(path.startsWith('/attachments?'))return {ok:true,data:[{id:31,filename:'chart.png',mime:'image/png',size_bytes:4}]};
      return {ok:false,status:404,error:`Unexpected ${method} ${path}`};
    }};
    window.fetch=async(url,options={})=>{window.publicFetches.push([String(url),options.credentials,options.headers&&Array.from(new Headers(options.headers).entries())]);return String(url).includes('/thumbnail')?new Response(new Uint8Array([137,80,78,71]),{status:200,headers:{'Content-Type':'image/png','Content-Length':'4'}}):new Response('file bytes',{status:200,headers:{'Content-Type':'image/png','Content-Length':'10'}});};
    eval(attachments+js);
   },{js,attachments});
   await page.waitForFunction(()=>document.querySelector('[data-topcoat-public]').getAttribute('aria-busy')==='false');
  };
  await mount('issues');
  assert.equal(await page.getByRole('link',{name:'ENG-1 · Public title'}).getAttribute('href'),'/public/ENG/issues/ENG-1');
  assert.equal(await page.locator('form,input,textarea,button,[draggable="true"]').count(),0);
  assert.equal(await page.evaluate(()=>publicRequests.every(([path,method])=>method==='GET'&&!path.includes('/my-role'))),true);

  const afterTables=await page.evaluate(()=>{
    const output=document.createElement('article');
    LificTopcoatPublic.renderMarkdown(document,output,'| Name | Count |\n| --- | --- |\n| Widget | 2 |\nParagraph immediately after the table.\n\n| Name | Count |\n| --- | --- |\n# Heading after an empty table\n- Following item','ENG');
    return {tables:output.querySelectorAll('table').length,paragraph:output.querySelector('p')?.textContent,heading:output.querySelector('h1')?.textContent,item:output.querySelector('li')?.textContent};
  });
  assert.deepEqual(afterTables,{tables:2,paragraph:'Paragraph immediately after the table.',heading:'Heading after an empty table',item:'Following item'});

  await mount('board');
  assert.equal(await page.locator('[data-public-lane="active"] a').innerText(),'ENG-1 · Public title');
  assert.equal(await page.locator('form,input,textarea,button').count(),0);

  await page.setContent(shell);
  await page.locator('#mount').evaluate(root=>root.innerHTML='<main class="tc-public" data-topcoat-public="issue-detail" data-public-project="ENG" data-public-identifier="ENG-1" aria-busy="true"><p data-public-status></p><div data-public-error hidden></div><section data-public-content hidden></section></main>');
  await page.evaluate(()=>{location.hash='#comment-70';});
  await page.evaluate(({js,attachments})=>{
    Object.defineProperty(window,'localStorage',{configurable:true,value:{getItem(){return null;},setItem(){},removeItem(){}}});
    window.publicRequests=[];window.publicFetches=[];window.lificSession={state:{publicProject:'ENG',user:null},resolve(path,method='GET'){return method==='GET'?{kind:'public',url:`/public/api/projects/ENG${path}`}:{kind:'refused'};},request:async(path,options={})=>{
     publicRequests.push([path,options.method||'GET',options.credentials||'omit',options.headers||null,options.body||null]);if((options.method||'GET')!=='GET'||options.body)return {ok:false,status:403,error:'public write refused'};
     if(path==='/projects')return {ok:true,data:[{id:7,identifier:'ENG'}]};if(path==='/projects/7/index')return {ok:true,data:{issues:[],pages:[]}};
     if(path==='/issues/resolve/ENG-1')return {ok:true,data:{id:11,identifier:'ENG-1'}};
     if(path==='/issues/11')return {ok:true,data:{id:11,identifier:'ENG-1',title:'Public title',status:'active',priority:'high',description:'# Public body\n\n**safe** <script>bad()</script> ![chart](/attachments/31) [attachment download](/api/attachments/31)\n\n> quoted text\n\n| Name | Count |\n| --- | ---: |\n| Widget | 2 |\n\n- [x] Done\n- [ ] Open\n\n~~removed~~'}};
     if(path.startsWith('/issues/11/comments?')){const before=new URLSearchParams(path.split('?')[1]).get('before_id');const rows=before?[{id:70,author:'writer',author_display_name:'Writer',content:'Public comment **visible** ![comment chart](/attachments/31)',created_at:'2026-01-01T00:00:00Z'}]:Array.from({length:51},(_,index)=>({id:120-index,author:'writer',author_display_name:'Writer',content:`Comment ${120-index}`,created_at:`2026-01-${String(31-index%28).padStart(2,'0')}T00:00:00Z`}));return {ok:true,data:rows,headers:new Headers({'x-comment-has-more':before?'false':'true'})};}
     if(path.startsWith('/attachments?')){const params=new URLSearchParams(path.split('?')[1]);return {ok:true,data:params.get('entity_type')==='issue'||params.get('entity_id')==='70'?[{id:31,filename:'chart.png',mime:'image/png',size_bytes:4}]:[]};}
     return {ok:false,status:404,error:`Unexpected ${path}`};
    }};
    window.fetch=async(url,options={})=>{const original=String(url).endsWith('/attachments/31');if(original&&window.delayDownloadFetch){window.pendingDownloadFetch=true;await new Promise(resolve=>window.releaseDownloadFetch=resolve);}publicFetches.push([String(url),options.credentials,options.headers&&Array.from(new Headers(options.headers).entries())]);const response=new Response(new Uint8Array([137,80,78,71]),{status:200,headers:{'Content-Type':'image/png','Content-Length':'4'}});if(original&&window.delayDownloadFetch)window.downloadFetchComplete=true;return response;};eval(attachments+js);
  },{js,attachments});
  await page.waitForFunction(()=>document.querySelector('[data-topcoat-public]').getAttribute('aria-busy')==='false');
  assert.equal(await page.locator('[data-public-content] h2').innerText(),'Public title');
  assert.equal(await page.locator('[data-public-content] script').count(),0);
  assert.equal(await page.locator('[data-public-content] img').count(),2);
  assert.equal(await page.locator('.tc-public__markdown [data-public-download="31"]').innerText(),'attachment download');
  assert.equal(await page.locator('.tc-public__markdown blockquote').innerText(),'quoted text');
  assert.equal(await page.locator('.tc-public__markdown table tbody tr').count(),1);
  assert.equal(await page.locator('.tc-public__markdown del').innerText(),'removed');
  assert.deepEqual(await page.locator('.tc-public__markdown input[type=checkbox]').evaluateAll(nodes=>nodes.map(node=>[node.checked,node.disabled])),[[true,true],[false,true]]);
  assert.equal(await page.locator('#comment-70').innerText().then(value=>value.includes('Public comment visible')),true);
  await page.waitForFunction(()=>document.querySelector('#comment-70 img')?.src.startsWith('blob:'));
  assert.equal(await page.locator('form,textarea,button').count(),0);
  const publicCalls=await page.evaluate(()=>publicRequests);
  assert.ok(publicCalls.every(([,method,credentials,headers,body])=>method==='GET'&&credentials==='omit'&&!body&&!headers?.Authorization&&!headers?.Cookie));
  const publicFetches=await page.evaluate(()=>publicFetches);
  assert.ok(publicFetches.every(([,credentials,headers])=>credentials==='omit'&&!headers?.some(([name])=>name.toLowerCase()==='authorization')));
  await page.locator('#attachment-31 [data-public-download="31"]').click();
  await page.waitForFunction(()=>publicFetches.some(([url])=>url.endsWith('/attachments/31')));
  assert.equal(await page.evaluate(()=>publicFetches.filter(([url])=>url.endsWith('/attachments/31')).length),1);
  const readsBeforeRestore=await page.evaluate(()=>publicRequests.filter(([path])=>path==='/projects').length);
  await page.evaluate(()=>{
    window.dispatchEvent(new PageTransitionEvent('pagehide',{persisted:true}));
    window.dispatchEvent(new PageTransitionEvent('pageshow',{persisted:true}));
  });
  await page.waitForFunction(before=>publicRequests.filter(([path])=>path==='/projects').length>before,readsBeforeRestore);
  await page.waitForFunction(()=>document.querySelector('[data-topcoat-public]').getAttribute('aria-busy')==='false');
  assert.equal(await page.locator('[data-public-content] h2').innerText(),'Public title');
  await page.evaluate(()=>{
    window.delayDownloadFetch=true;window.pendingDownloadFetch=false;window.downloadFetchComplete=false;window.downloadObjectUrls=0;const create=URL.createObjectURL.bind(URL);
    URL.createObjectURL=blob=>{window.downloadObjectUrls++;return create(blob);};
  });
  await page.locator('#attachment-31 [data-public-download="31"]').click();await page.waitForFunction(()=>pendingDownloadFetch);
  await page.evaluate(()=>{dispatchEvent(new PageTransitionEvent('pagehide',{persisted:false}));releaseDownloadFetch();});await page.waitForFunction(()=>downloadFetchComplete);
  assert.equal(await page.evaluate(()=>downloadObjectUrls),0);
  assert.ok(errors.length===0,errors.join('\n'));

  // A public session refuses every mutating method before transport.
  const refused=await page.evaluate(()=>lificSession.resolve('/issues/11','POST').kind);
  assert.equal(refused,'refused');

  await page.setContent(shell);
  await page.locator('#mount').evaluate(root=>root.innerHTML='<main class="tc-public" data-topcoat-public="pages" data-public-project="ENG" data-public-identifier="" aria-busy="true"><p data-public-status></p><div data-public-error hidden></div><section data-public-content hidden></section></main>');
  await page.evaluate(({js,attachments})=>{
   Object.defineProperty(window,'localStorage',{configurable:true,value:{getItem(){return null;}}});
   window.lificSession={state:{publicProject:'ENG',user:null},resolve:(path,method='GET')=>method==='GET'?{kind:'public',url:`/public/api/projects/ENG${path}`}:{kind:'refused'},request:async path=>path==='/projects'?{ok:true,data:[{id:7,identifier:'ENG'}]}:path==='/projects/7/index'?{ok:true,data:{issues:[],pages:[{id:22,title:'Public page',status:'active',preview:'Public preview'}]}}:{ok:false,status:404,error:path}};
   eval(attachments+js);
  },{js,attachments});
  await page.waitForFunction(()=>document.querySelector('[data-topcoat-public]').getAttribute('aria-busy')==='false');
  assert.equal(await page.getByRole('link',{name:'Public page'}).getAttribute('href'),'/public/ENG/pages/22');

  await page.setContent(shell);
  await page.locator('#mount').evaluate(root=>root.innerHTML='<main class="tc-public" data-topcoat-public="page-detail" data-public-project="ENG" data-public-identifier="22" aria-busy="true"><p data-public-status></p><div data-public-error hidden></div><section data-public-content hidden></section></main>');
  await page.evaluate(({js,attachments})=>{
   Object.defineProperty(window,'localStorage',{configurable:true,value:{getItem(){return null;}}});
   window.publicRequests=[];window.publicFetches=[];
   window.lificSession={
    state:{publicProject:'ENG',user:null},
    resolve:(path,method='GET')=>method==='GET'?{kind:'public',url:`/public/api/projects/ENG${path}`}:{kind:'refused'},
    request:async path=>path==='/projects'?{ok:true,data:[{id:7,identifier:'ENG'}]}:
     path==='/projects/7/index'?{ok:true,data:{issues:[],pages:[]}}:
     path==='/pages/22'?{ok:true,data:{id:22,title:'Public page',status:'active',content:'# Public page body\n\n*visible* <img src=x onerror=bad()> ![chart](/attachments/31)'}}:
     path.startsWith('/pages/22/comments?')?{ok:true,data:[],headers:new Headers({'x-comment-has-more':'false'})}:
     path.startsWith('/attachments?')?{ok:true,data:[{id:31,filename:'chart.png',mime:'image/png',size_bytes:4}]}:
     {ok:false,status:404,error:path}
   };
   window.fetch=async(url,options={})=>{publicFetches.push([String(url),options.credentials]);return new Response(new Uint8Array([137,80,78,71]),{status:200,headers:{'Content-Type':'image/png','Content-Length':'4'}});};
   eval(attachments+js);
  },{js,attachments});
  await page.waitForFunction(()=>document.querySelector('[data-topcoat-public]').getAttribute('aria-busy')==='false');
  assert.equal(await page.locator('[data-public-content] h2').innerText(),'Public page');
  assert.equal(await page.locator('[data-public-content] img').count(),1);
  assert.equal(await page.locator('form,input,textarea,button').count(),0);
  assert.equal(await page.evaluate(()=>lificSession.resolve('/pages/22','DELETE').kind),'refused');
 }finally{await browser.close();}
});

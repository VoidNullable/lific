const {test} = require('node:test');
const assert = require('node:assert/strict');
const fs = require('node:fs');
const path = require('node:path');

test('headless identity screens preserve auth policy, credential errors, and admin visibility',
  {skip: !process.env.PLAYWRIGHT_EXECUTABLE_PATH}, async t => {
    const {chromium}=await import(path.resolve(__dirname,'../../../..','e2e/node_modules/playwright/index.mjs'));
    const browser=await chromium.launch({headless:true,executablePath:process.env.PLAYWRIGHT_EXECUTABLE_PATH});
    const page=await browser.newPage(); page.setDefaultTimeout(5000);
    const errors=[]; page.on('pageerror',error=>errors.push(error.stack||error.message));
    const script=fs.readFileSync(`${__dirname}/identity.js`,'utf8');
    async function mount(mode,user=null,instance={allow_signup:true,has_users:true,instance_name:'Lific',login_message:''}) {
      await page.route('http://identity.test/**',route=>route.fulfill({contentType:'text/html',body:`<!doctype html><html><body>
        <section class="tc-identity" data-topcoat-identity="${mode}" aria-busy="true"><p data-identity-status role="status">Loading</p><div data-identity-content></div></section></body></html>`}));
      await page.goto('http://identity.test/');
      await page.evaluate(({user,instance})=>{
        window.calls=[];window.navigated=[];
        window.lificSession={state:{user,loading:false},request:async(path,options={})=>{
          calls.push({path,options});
          if(path==='/instance')return {ok:true,data:instance};
          if(path==='/auth/me')return user?{ok:true,data:user}:{ok:false,status:401,error:'not signed in'};
          if(path==='/auth/keys'||path==='/auth/bots'||path==='/users')return {ok:true,data:[]};
          if(path==='/instance/settings')return {ok:true,data:{instance_name:'Lific',allow_signup:true,signup_email_domains:[],session_lifetime_days:30,login_message:'',web_auto_login:false,authz_enforced:true}};
          if(path==='/auth/login')return {ok:false,status:400,error:'Invalid username or password'};
          return {ok:false,status:403,error:'Unexpected request'};
        },refreshAccount:async()=>user?{ok:true,data:user}:{ok:false,status:401,error:'not signed in'},
          saveSession(){},clearSession(){},logout:async()=>({ok:true})};
      },{user,instance});
      await page.addScriptTag({content:script});
      await page.waitForFunction(()=>document.querySelector('[data-identity-content]')?.children.length>0);
    }
    try {
      await t.test('login exposes recoverable credential failures and password visibility control',async()=>{
        await mount('login');
        await page.getByLabel('Username or email').fill('alex');
        await page.locator('input[name="password"]').fill('bad-password');
        await page.getByRole('button',{name:'Log in'}).click();
        await page.getByRole('alert').waitFor();
        assert.match(await page.getByRole('alert').textContent(),/Invalid username or password/);
        await page.getByRole('button',{name:'Show password'}).click();
        assert.equal(await page.locator('input[name="password"]').getAttribute('type'),'text');
      });
      await t.test('first account setup remains available when public signup is closed',async()=>{
        await mount('signup',null,{allow_signup:false,has_users:false,instance_name:'New Lific',login_message:''});
        assert.equal(await page.getByRole('heading',{name:'Set up your instance'}).count(),1);
        assert.equal(await page.getByLabel('Username').count(),1);
      });
      await t.test('ordinary accounts get no instance administration requests or controls',async()=>{
        await mount('instance',{id:7,username:'reader',is_admin:false});
        assert.equal(await page.getByRole('alert').textContent(),'Administrator access is required.');
        assert.equal(await page.evaluate(()=>calls.some(call=>call.path==='/users'||call.path==='/instance/settings')),false);
        assert.equal(await page.getByRole('button',{name:'Create account'}).count(),0);
      });
      assert.deepEqual(errors,[]);
    } finally {await browser.close();}
  });

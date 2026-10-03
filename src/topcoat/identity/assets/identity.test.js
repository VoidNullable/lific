const {test} = require('node:test');
const assert = require('node:assert/strict');
const vm = require('node:vm');
const fs = require('node:fs');
const path = require('node:path');

const context = {console, location:{assign(){}}, URL, encodeURIComponent, globalThis:null};
context.globalThis=context;
vm.runInNewContext(fs.readFileSync(path.join(__dirname,'identity.js'),'utf8'),context);
const identity=context.LificTopcoatIdentity;
const ok=data=>({ok:true,data});

function setup(handler=async()=>ok({})) {
  const calls=[], sessions=[], routes=[];
  const session={state:{user:null},request:async(path,options={})=>{calls.push({path,...options});return handler(path,options);},
    saveSession:token=>sessions.push(token),clearSession(){sessions.push(null);},refreshAccount:async()=>ok(session.state.user),
    logout:async()=>{sessions.push(null);}};
  const app=identity.controller({session,navigate:path=>routes.push(path),storage:{getItem:()=>null,setItem(){}},preferences:null});
  return {app,calls,sessions,routes,session};
}

test('login stores the returned session and navigates to My Work',async()=>{
  const {app,calls,sessions,routes}=setup(async(path)=>path==='/auth/login'?ok({token:'session-a'}):ok({}));
  assert.equal((await app.login('  alex@example.com ','pw')).ok,true);
  assert.equal(calls[0].path,'/auth/login');
  assert.deepEqual(JSON.parse(calls[0].body),{identity:'alex@example.com',password:'pw'});
  assert.deepEqual(sessions,['session-a']);
  assert.deepEqual(routes,['/']);
});

test('signup validates fields locally, permits first account setup, and keeps signup-closed errors',async()=>{
  const first=setup(async path=>path==='/auth/signup'?ok({token:'first'}):ok({}));
  first.app.state.instance={allow_signup:false,has_users:false};
  assert.equal((await first.app.signup({username:'x',email:'x@x.co',password:'12345678'})).ok,false);
  assert.equal(first.calls.length,0);
  assert.equal((await first.app.signup({username:'alex_1',email:'alex@example.com',password:'12345678'})).ok,true);
  assert.deepEqual(first.sessions,['first']);
  const closed=setup(); closed.app.state.instance={allow_signup:false,has_users:true};
  assert.match((await closed.app.signup({username:'alex',email:'alex@example.com',password:'12345678'})).error,/closed/);
  assert.equal(closed.calls.length,0);
});

test('failed credentials preserve the session and expose the server error',async()=>{
  const {app,sessions,routes}=setup(async()=>({ok:false,status:400,error:'Invalid username or password'}));
  const result=await app.login('alex','wrong');
  assert.equal(result.error,'Invalid username or password');
  assert.deepEqual(sessions,[]); assert.deepEqual(routes,[]);
});

test('password change adopts the replacement token before reloading connected tools',async()=>{
  const {app,calls,sessions}=setup(async path=>path==='/auth/me/password'?ok({token:'replacement'}):ok([]));
  assert.equal((await app.changePassword('old-password','new-password')).ok,true);
  assert.deepEqual(sessions,['replacement']);
  assert.deepEqual(calls.map(call=>call.path),['/auth/me/password','/auth/keys','/auth/bots']);
  assert.equal((await app.changePassword('old','short')).ok,false);
});

test('instance settings park a refused patch, merge edits, refresh once, and replay the full patch',async()=>{
  let patches=0;
  const {app,calls,sessions}=setup(async(path,options)=>{
    if(path==='/instance/settings'&&options.method==='PATCH'){
      patches++;
      return patches===1?{ok:false,status:403,code:'recent_auth_required',error:'Recent authentication required'}:ok({instance_name:'Lific',allow_signup:false});
    }
    if(path==='/auth/me/refresh')return ok({token:'fresh'});
    return ok([]);
  });
  app.state.user={id:1,is_admin:true}; app.state.settings={instance_name:'Lific',allow_signup:true};
  assert.equal((await app.saveSettings({allow_signup:false})).pending,true);
  await app.saveSettings({instance_name:'Acme'});
  assert.deepEqual(JSON.parse(JSON.stringify(app.state.pendingPatch)),{allow_signup:false,instance_name:'Acme'});
  assert.equal((await app.reauthenticate('password')).ok,true);
  assert.deepEqual(sessions,['fresh']);
  assert.deepEqual(JSON.parse(calls.at(-1).body),{allow_signup:false,instance_name:'Acme'});
  assert.equal(app.state.pendingAction,null);
});

test('instance admin data is never requested for an ordinary account',async()=>{
  const {app,calls}=setup(); app.state.user={id:3,is_admin:false};
  assert.equal(await app.loadAdmin(),false);
  assert.deepEqual(calls,[]);
});

test('appearance uses the existing local preference keys and font-size values',()=>{
  const stored=new Map([['lific_theme','dark'],['lific_font_scale','lg']]);
  const {app}=setup();
  app.appearance=undefined;
  const ctx=identity.controller({session:{},navigate(){},storage:{getItem:key=>stored.get(key)||null,setItem:(key,value)=>stored.set(key,value)}});
  assert.equal(ctx.appearance().theme,'dark');
  assert.equal(ctx.appearance().fontScale,'large');
  ctx.saveAppearance('fontScale','small');
  assert.equal(stored.get('lific_font_scale'),'sm');
});

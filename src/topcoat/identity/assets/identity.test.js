const {test} = require('node:test');
const assert = require('node:assert/strict');
const vm = require('node:vm');
const fs = require('node:fs');
const path = require('node:path');

const context = {console, location:{origin:'https://lific.test',assign(){}}, URL, encodeURIComponent, globalThis:null};
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
  assert.deepEqual(calls.map(call=>call.path),['/auth/me/password','/instance','/auth/keys','/auth/bots']);
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

test('passwordless effective auth mode loads instance policy and refreshes without a password',async()=>{
  const calls=[];
  const session={request:async(path,options={})=>{calls.push({path,...options});
    if(path==='/instance')return ok({web_auto_login:true});
    if(path==='/auth/me/refresh')return ok({token:'passwordless-refresh'});
    if(path==='/auth/keys'||path==='/auth/bots')return ok([]);
    return ok({});},saveSession(){}};
  const app=identity.controller({session,navigate(){}});
  app.state.pendingAction={kind:'action',action:async()=>ok({})};
  assert.equal((await app.reauthenticate('')).ok,true);
  assert.ok(calls.some(call=>call.path==='/instance'));
  const refresh=calls.find(call=>call.path==='/auth/me/refresh');
  assert.equal(refresh.body,undefined);
});

test('settings edits made during the reauthentication replay are sent before clearing pending state',async()=>{
  let releaseReplay, patchNumber=0;
  const calls=[];
  const session={state:{},saveSession(){},request:async(path,options={})=>{
    calls.push({path,...options});
    if(path==='/instance/settings'&&options.method==='PATCH') {
      patchNumber++;
      if(patchNumber===1)return {ok:false,status:403,code:'recent_auth_required',error:'Recent authentication required'};
      if(patchNumber===2)return new Promise(resolve=>{releaseReplay=()=>resolve(ok({instance_name:'Lific',allow_signup:false,login_message:''}));});
      return ok({instance_name:'Lific',allow_signup:false,login_message:'later'});
    }
    if(path==='/auth/me/refresh')return ok({token:'fresh'});
    return ok({});
  }};
  const app=identity.controller({session,navigate(){}});
  app.state.user={id:1,is_admin:true};
  app.state.settings={instance_name:'Lific',allow_signup:true,login_message:''};
  await app.saveSettings({allow_signup:false});
  const replay=app.reauthenticate('password');
  while(!releaseReplay) await new Promise(resolve=>setTimeout(resolve,0));
  await app.saveSettings({login_message:'later'});
  releaseReplay();
  assert.equal((await replay).ok,true);
  assert.equal(patchNumber,3);
  assert.deepEqual(JSON.parse(calls.filter(call=>call.path==='/instance/settings').at(-1).body),{login_message:'later'});
  assert.equal(app.state.pendingAction,null);
});

test('tool setup returns each supported clients configuration and guidance',()=>{
  const setup=identity.toolSetup('codex','https://lific.test','one-time-key','linux');
  assert.match(setup.config,/transport\.bearer_token_env_var = "LIFIC_API_KEY"/);
  assert.match(setup.environment,/export LIFIC_API_KEY="one-time-key"/);
  assert.match(setup.environmentNote,/\.bashrc|\.profile/);
  assert.match(identity.toolSetup('codex','https://lific.test','key','windows').environment,/setx LIFIC_API_KEY/);
  assert.match(identity.toolSetup('vscode','https://lific.test','key','linux').config,/"servers"/);
  assert.match(identity.toolSetup('zed','https://lific.test','key','linux').config,/"context_servers"/);
  const opencode=identity.toolSetup('opencode','https://lific.test','key','linux');
  assert.match(opencode.config,/"remote"/);
  assert.match(opencode.instructions,/"mcp"/);
  const claude=identity.toolSetup('claude','https://lific.test','key','mac');
  assert.match(claude.config,/"mcp-remote"/);
  assert.match(claude.instructions,/restart Claude Desktop/i);
  assert.equal(identity.toolSetup('claude','https://lific.test','key','linux').path,null);
  assert.equal(identity.detectOs({userAgent:'Mozilla Windows'}),'windows');
});

test('Cursor and Claude Code configure HTTP MCP while Pi reads its bearer token from the environment',()=>{
  const cursor=identity.toolSetup('cursor','https://lific.test','client-key','windows');
  assert.equal(cursor.name,'Cursor');
  assert.deepEqual(JSON.parse(cursor.config),{lific:{url:'https://lific.test/mcp',headers:{Authorization:'Bearer client-key'}}});
  assert.match(cursor.path,/%USERPROFILE%\\\.cursor\\mcp\.json/);
  assert.match(cursor.instructions,/mcpServers.*reload Cursor/);
  const claude=identity.toolSetup('claude-code','https://lific.test','client-key','linux');
  assert.equal(claude.path,'~/.claude.json (user scope)');
  assert.deepEqual(JSON.parse(claude.config),{lific:{type:'http',url:'https://lific.test/mcp',headers:{Authorization:'Bearer client-key'}}});
  assert.equal(claude.commands[0],'claude mcp add --transport http --scope user lific https://lific.test/mcp --header "Authorization: Bearer <key>"');
  const pi=identity.toolSetup('pi','https://lific.test','client-key','linux');
  assert.equal(pi.path,'~/.pi/agent/mcp.json');
  assert.deepEqual(JSON.parse(pi.config),{lific:{url:'https://lific.test/mcp',auth:'bearer',bearerTokenEnv:'LIFIC_API_KEY',lifecycle:'keep-alive'}});
  assert.equal(pi.commands[0],'pi install npm:pi-mcp-adapter');
  assert.match(pi.instructions,/restart Pi.*mcpServers/);
  assert.match(pi.environment,/export LIFIC_API_KEY="client-key"/);
  assert.equal(pi.config.includes('client-key'),false);
});

test('custom tool IDs that name prototype properties retain their issued key and generic setup',async()=>{
  for(const tool of ['constructor','toString','__proto__','hasOwnProperty']) {
    const {app}=setup(async path=>path==='/auth/bots'?ok({key:'custom-secret'}):ok({}));
    assert.equal((await app.connectBot(tool)).ok,true);
    assert.equal(app.state.secret.value,'custom-secret');
    assert.equal(app.state.secret.name,'Custom MCP client');
    assert.equal(app.state.secret.toolId,tool);
    assert.deepEqual(JSON.parse(app.state.secret.config),{url:'https://lific.test/mcp',headers:{Authorization:'Bearer custom-secret'}});
  }
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

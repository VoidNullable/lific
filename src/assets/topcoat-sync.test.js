const {test} = require('node:test');
const assert = require('node:assert/strict');
const fs = require('node:fs');
const vm = require('node:vm');
const context = {globalThis:{}, console,AbortController};
vm.runInNewContext(fs.readFileSync(`${__dirname}/topcoat-sync.js`, 'utf8'),context);
const {createClient} = context.globalThis.LificSync;
const tick = () => new Promise(resolve => setImmediate(resolve));
function fixture(request,overrides={}) {
    let token='first';
    const state={publicProject:null,user:{id:1}};
    const sockets=[];
    const client=createClient({session:{state,request},token:()=>token,notify:()=>{},
        socket:()=>{const listeners={};const ws={readyState:0,frames:[],addEventListener:(name,fn)=>listeners[name]=fn,send:frame=>ws.frames.push(JSON.parse(frame)),close:()=>ws.closed=true};ws.emit=(name,data)=>listeners[name]?.(data);sockets.push(ws);return ws;},
        delay:setTimeout,cancel:clearTimeout,interval:setInterval,clearInterval,
        websocketUrl:()=> 'ws://localhost/api/events/ws',...overrides});
    return {client,state,sockets,setToken:value=>token=value};
}
const row=(id,seq,title='live')=>({kind:'issue',id,seq,deleted:false,title});
test('snapshot races, sparse sequences, filtered pages, tombstones and replay duplicates reconcile',async()=>{
    const paths=[];
    const f=fixture(async path=>{paths.push(path);if(path.endsWith('/index'))return {ok:true,data:{cursor:10,issues:[row(1,15)],pages:[]}};
        if(path.includes('since=10')) return {ok:true,data:{cursor:20,changes:[],has_more:true}};
        return {ok:true,data:{cursor:40,changes:[row(1,15,'older'),{kind:'issue',id:1,seq:30,deleted:true},row(2,40)],has_more:false}};});
    await f.client.ensureProject(7);
    const model=f.client.peekProject(7);
    assert.equal(model.cursor,40);assert.deepEqual(Array.from(model.issues, x=>x.id),[2]);
    assert.equal(paths.length,3);
    f.client.handleEvent({type:'issue.updated',project_id:7,seq:35});await tick();assert.equal(paths.length,3);
    f.client.dispose();
});
test('token and public scope changes clear caches and reject stale responses',async()=>{
    let resolve;
    const f=fixture(()=>new Promise(done=>resolve=done));
    const loading=f.client.ensureProject(7);
    f.setToken('second');f.client.audienceChanged();
    resolve({ok:true,data:{cursor:4,issues:[row(1,4)],pages:[]}});await loading;
    assert.equal(f.client.peekProject(7),null);
    f.state.publicProject='LIF';f.client.audienceChanged();assert.equal(f.client.peekProject(7),null);
    f.client.connect();await tick();assert.equal(f.sockets.length,0);f.client.dispose();
});
test('socket events only pull their project and never advance cursors; reconnect resumes',async()=>{
    let cursor=2;const paths=[];
    const f=fixture(async path=>{paths.push(path);return {ok:true,data:path.endsWith('/index')?{cursor,issues:[],pages:[]}:{cursor,changes:[],has_more:false}}});
    await f.client.ensureProject(7);f.client.setActiveProject(7);await f.client.connect();
    const ws=f.sockets[0];ws.readyState=1;ws.emit('open');await tick();
    assert.ok(ws.frames.some(frame=>frame.type==='resume'&&frame.project_id===7&&frame.cursor===2));
    const before=paths.length;f.client.handleEvent({type:'issue.updated',project_id:8,seq:100});assert.equal(paths.length,before);
    f.client.handleEvent({type:'issue.updated',project_id:7,seq:100});assert.equal(f.client.peekProject(7).cursor,2);
    f.client.dispose();
});
test('unsafe cursors fail without applying rounded stream positions',async()=>{
    const f=fixture(async()=>({ok:true,data:{cursor:Number.MAX_SAFE_INTEGER+1,issues:[],pages:[]}}));
    await f.client.ensureProject(7);const model=f.client.peekProject(7);
    assert.equal(model.cursor,0);assert.equal(model.status,'cold');assert.match(model.error,/safe integer/);f.client.dispose();
});
test('revoked project access clears cached rows and its cursor',async()=>{
    const f=fixture(async path=>{
        if(path.endsWith('/index'))return {ok:true,data:{cursor:1,issues:[{...row(1,1),labels:[{name:'urgent'}],waits:[{issue_id:2}]}],pages:[]}};
        return {ok:false,status:403,error:'forbidden'};
    });
    await f.client.ensureProject(7);
    const model=f.client.peekProject(7);
    assert.equal(model.status,'cold');assert.equal(model.cursor,0);assert.deepEqual(Array.from(model.issues),[]);
    f.client.dispose();
});
test('nested row fields are immutable in cached read-model snapshots',async()=>{
    const f=fixture(async path=>({ok:true,data:path.endsWith('/index')?
        {cursor:1,issues:[{...row(1,1),labels:['urgent'],waits:[{issue_id:2}]}],pages:[]}:
        {cursor:1,changes:[],has_more:false}}));
    await f.client.ensureProject(7);
    const issue=f.client.peekProject(7).issues[0];
    assert.equal(Object.isFrozen(issue.labels),true);assert.equal(Object.isFrozen(issue.waits),true);
    assert.equal(Object.isFrozen(issue.waits[0]),true);f.client.dispose();
});
test('bootstrap queues concurrent invalidations and rejects a stale delta after scope changes',async()=>{
    let resolveIndex,resolveDelta;
    const paths=[];
    const f=fixture(path=>{paths.push(path);return new Promise(resolve=>{if(path.endsWith('/index'))resolveIndex=resolve;else resolveDelta=resolve;});});
    const loading=f.client.ensureProject(7);
    f.client.handleEvent({type:'issue.created',project_id:7,seq:12});
    resolveIndex({ok:true,data:{cursor:10,issues:[],pages:[]}});await tick();
    assert.match(paths[1],/since=10/);
    f.state.publicProject='OTHER';f.client.audienceChanged();
    resolveDelta({ok:true,data:{cursor:12,changes:[row(1,12)],has_more:false}});await loading;
    assert.equal(f.client.peekProject(7),null);f.client.dispose();
});
test('pagination must advance; a failed pull retries from a fresh snapshot',async()=>{
    const paths=[];let indexCount=0;
    const f=fixture(async path=>{paths.push(path);return {ok:true,data:path.endsWith('/index')?
        {cursor:++indexCount,issues:[],pages:[]}:{cursor:indexCount,changes:[],has_more:true}};});
    await f.client.ensureProject(7);
    assert.equal(f.client.peekProject(7).status,'cold');
    assert.match(f.client.peekProject(7).error,/did not advance/);
    await f.client.refreshProject(7);
    assert.equal(indexCount,2);assert.equal(f.client.peekProject(7).cursor,2);f.client.dispose();
});
test('awaited refresh drains an in-flight invalidation without debounce',async()=>{
    let delta=0,resolvePending;
    const f=fixture(async path=>{
        if(path.endsWith('/index'))return {ok:true,data:{cursor:1,issues:[],pages:[]}};
        delta++;
        if(delta===1)return {ok:true,data:{cursor:1,changes:[],has_more:false}};
        if(delta===2)return new Promise(resolve=>resolvePending=resolve);
        return {ok:true,data:{cursor:3,changes:[],has_more:false}};
    });
    await f.client.ensureProject(7);
    const first=f.client.refreshProject(7);await tick();
    const second=f.client.refreshProject(7);await tick();
    resolvePending({ok:true,data:{cursor:2,changes:[],has_more:false}});
    await Promise.all([first,second]);
    assert.equal(delta,3);assert.equal(f.client.peekProject(7).cursor,3);f.client.dispose();
});
test('socket closes reconnect with bounded backoff and resume from applied REST cursor',async()=>{
    const timers=[];
    const f=fixture(async path=>({ok:true,data:path.endsWith('/index')?{cursor:8,issues:[],pages:[]}:{cursor:8,changes:[],has_more:false}}),
        {delay:(callback,ms)=>{const timer={callback,ms};timers.push(timer);return timer;},cancel:timer=>{if(timer)timer.cancelled=true;},interval:()=>null,clearInterval:()=>{}});
    await f.client.ensureProject(7);f.client.setActiveProject(7);await f.client.connect();
    const first=f.sockets[0];first.readyState=1;first.emit('open');first.readyState=3;first.emit('close');
    const retry=timers.find(timer=>timer.ms===1000);assert.ok(retry);retry.callback();
    const next=f.sockets[1];next.readyState=1;next.emit('open');
    assert.ok(next.frames.some(frame=>frame.type==='resume'&&frame.cursor===8));
    assert.ok(next.frames.some(frame=>frame.type==='activity.baseline.request'));
    f.client.handleEvent({type:'activity.baseline',day_count:15});assert.equal(f.client.state.activityBaseline,15);
    f.client.handleEvent({type:'resync.required'});assert.equal(f.client.state.activityBaseline,null);
    f.client.dispose();
});
function sharedTransport() {
    const channels=new Map();const queue=[];let held=false;
    function grant() {
        if(held||!queue.length)return;
        const entry=queue.shift();held=true;
        Promise.resolve(entry.callback()).finally(()=>{held=false;entry.resolve();grant();});
    }
    return {
        fingerprint:async()=> 'same-audience',
        locks:{request:(_name,options,callback)=>new Promise(resolve=>{
            const entry={callback,resolve};queue.push(entry);
            options.signal.addEventListener('abort',()=>{const at=queue.indexOf(entry);if(at>=0){queue.splice(at,1);resolve();}});
            grant();
        })},
        channel:name=>{
            const members=channels.get(name)||new Set();channels.set(name,members);
            const channel={listener:null,addEventListener:(_name,listener)=>channel.listener=listener,
                postMessage:data=>{for(const peer of members)if(peer!==channel)peer.listener?.({data});},
                close:()=>members.delete(channel)};
            members.add(channel);return channel;
        }
    };
}
test('one leader socket serves two tabs and promotes its follower after teardown',async()=>{
    const shared=sharedTransport();
    const request=async path=>({ok:true,data:path.endsWith('/index')?{cursor:9,issues:[],pages:[]}:{cursor:9,changes:[],has_more:false}});
    const a=fixture(request,shared),b=fixture(request,shared);
    await a.client.ensureProject(7);await b.client.ensureProject(7);
    a.client.setActiveProject(7);b.client.setActiveProject(7);
    await a.client.connect();await b.client.connect();
    assert.equal(a.sockets.length,1);assert.equal(b.sockets.length,0);
    a.sockets[0].readyState=1;a.sockets[0].emit('open');
    assert.equal(b.client.state.connected,true);assert.equal(b.client.state.leader,false);
    assert.ok(a.sockets[0].frames.filter(frame=>frame.type==='resume').length>=2);
    a.client.dispose();await tick();
    assert.equal(b.sockets.length,1);b.sockets[0].readyState=1;b.sockets[0].emit('open');
    assert.equal(b.client.state.leader,true);
    assert.ok(b.sockets[0].frames.some(frame=>frame.type==='resume'&&frame.cursor===9));b.client.dispose();
});

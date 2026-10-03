(() => {
    'use strict';

    class SyncWireError extends Error {}

    function freezeValue(value) {
        if(Array.isArray(value)) return Object.freeze(value.map(freezeValue));
        if(value&&typeof value==='object') return Object.freeze(Object.fromEntries(Object.entries(value).map(([key,child])=>[key,freezeValue(child)])));
        return value;
    }

    // REST snapshots and deltas alone advance cursors. Socket messages are
    // invalidations, because their sparse, instance-wide seq carries no row.
    function createClient(env) {
        const models=new Map();
        const listeners=new Set();
        let audience=identity();
        let generation=0;
        let active=null;
        let disposed=false;
        let transport=null;
        let activityBaseline=null;
        function identity() { return `${env.session.state.publicProject ?? ''}:${env.token() ?? ''}`; }
        function notify() { for(const listener of listeners) listener(); env.notify?.(); }
        function valid(term) { return !disposed && term===generation && audience===identity(); }
        function integer(value,name) {
            if(!Number.isSafeInteger(value)||value<0) throw new SyncWireError(`${name} must be a nonnegative safe integer; reload the snapshot.`);
            return value;
        }
        function parseRow(row) {
            integer(row.id,'row id');integer(row.seq,'row seq');
            if(!['issue','page','comment'].includes(row.kind)||typeof row.deleted!=='boolean') throw new SyncWireError('Invalid sync row.');
            return freezeValue({...row});
        }
        function parsePage(data,snapshot) {
            integer(data.cursor,'cursor');
            const rows=(snapshot?[...data.issues,...data.pages]:data.changes).map(parseRow);
            if(snapshot && (rows.some(row=>row.deleted||row.kind==='comment') || data.issues.some(row=>row.kind!=='issue') || data.pages.some(row=>row.kind!=='page'))) throw new SyncWireError('Invalid snapshot rows.');
            if(!snapshot && (typeof data.has_more!=='boolean'||rows.some(row=>row.seq>data.cursor))) throw new SyncWireError('Invalid delta cursor.');
            return {cursor:data.cursor,rows,hasMore:snapshot?false:data.has_more};
        }
        function view(model) {
            return Object.freeze({projectId:model.id,cursor:model.cursor,status:model.status,error:model.error,
                issues:Object.freeze([...model.issues.values()]),pages:Object.freeze([...model.pages.values()])});
        }
        function reconcile(model,page,snapshot) {
            const issues=snapshot?new Map():new Map(model.issues);
            const pages=snapshot?new Map():new Map(model.pages);
            for(const row of page.rows) {
                const target=row.kind==='issue'?issues:row.kind==='page'?pages:null;
                const previous=target?.get(row.id);
                // Snapshot lists can contain rows newer than its watermark.
                // A replayed tombstone must not delete such a newer row.
                if(target && (!previous||row.seq>=previous.seq)) {
                    if(row.deleted) target.delete(row.id);else target.set(row.id,row);
                }
            }
            model.issues=issues;model.pages=pages;model.cursor=Math.max(model.cursor,page.cursor);
            model.status='ready';model.error='';notify();
        }
        function audienceChanged() {
            const next=identity();
            if(next!==audience) {
                generation++;audience=next;
                stopTransport();
                for(const model of models.values()) {env.cancel(model.timer);model.controller?.abort();}
                models.clear();active=null;activityBaseline=null;notify();
            }
        }
        function ownedModel(id) {
            audienceChanged();integer(id,'project id');
            let model=models.get(id);
            if(!model) {
                model={id,term:generation,cursor:0,status:'cold',error:'',issues:new Map(),pages:new Map(),pending:null,queued:false,timer:null,controller:null};
                models.set(id,model);
            }
            return model;
        }
        async function reconcileProject(model) {
            if(!valid(model.term)) return;
            if(model.pending) {model.queued=true;await model.pending;return;}
            const snapshot=model.status==='cold';
            const controller=new AbortController();model.controller=controller;
            if(snapshot) {model.status='loading';notify();}
            model.pending=(async()=>{
                try {
                    let bootstrap=snapshot;
                    for(;;) {
                        const since=model.cursor;
                        const path=bootstrap?`/projects/${model.id}/index`:`/projects/${model.id}/changes?since=${since}`;
                        const response=await env.session.request(path,{signal:controller.signal});
                        if(!valid(model.term)) return;
                        if(!response.ok) {
                            if([401,403,404].includes(response.status)) {
                                model.issues.clear();model.pages.clear();model.cursor=0;model.status='cold';
                            }
                            throw new Error(response.error||'Sync request failed.');
                        }
                        const page=parsePage(response.data,bootstrap);
                        if(!bootstrap && (page.cursor<since || (page.hasMore&&page.cursor===since))) throw new SyncWireError('Delta pagination did not advance. Reload the snapshot.');
                        reconcile(model,page,bootstrap);
                        // A pull immediately after every snapshot catches writes
                        // racing the cursor-before-lists bootstrap contract.
                        if(bootstrap) {bootstrap=false;continue;}
                        if(!page.hasMore) break;
                    }
                } catch(error) {
                    if(valid(model.term)) {
                        model.error=error.message;
                        if(model.status==='loading'||error instanceof SyncWireError) model.status='cold';
                        notify();
                    }
                }
            })();
            await model.pending;
            model.pending=null;model.controller=null;
            if(model.queued&&valid(model.term)) schedule(model);
        }
        function schedule(model) {
            if(model.timer===null&&valid(model.term)) model.timer=env.delay(()=>{model.timer=null;model.queued=false;void reconcileProject(model);},150);
        }
        function handleEvent(event) {
            audienceChanged();
            if(env.session.state.publicProject!==null) return;
            if(event.type==='activity.baseline') {activityBaseline=event.day_count;notify();return;}
            if(event.type==='resync.required') {
                activityBaseline=null;
                for(const model of models.values())schedule(model);
                send({type:'activity.baseline.request'});
                notify();env.event?.(event);return;
            }
            const model=models.get(event.project_id);
            if(model && (event.type==='sync_required'||event.seq===undefined||!Number.isSafeInteger(event.seq)||event.seq>model.cursor)) schedule(model);
            env.event?.(event);
        }
        function send(frame) {
            const term=transport;
            if(term?.socket?.readyState===1) {term.socket.send(JSON.stringify(frame));return true;}
            if(term?.channel&&term.open&&!term.leader) {term.channel.postMessage({kind:'outbound',frame});return true;}
            return false;
        }
        function opened(term) {
            if(transport!==term||!valid(term.generation)) return;
            term.open=true;term.attempt=0;
            const model=models.get(active);
            if(model?.status==='ready') send({type:'resume',project_id:model.id,cursor:model.cursor});
            send({type:'activity.baseline.request'});
            for(const replica of models.values()) schedule(replica);
            notify();
        }
        function openSocket(term) {
            if(transport!==term||!valid(term.generation)) return;
            const ws=env.socket(env.websocketUrl());term.socket=ws;
            ws.addEventListener('open',()=>{
                if(term.socket!==ws||transport!==term) return;
                term.channel?.postMessage({kind:'open'});opened(term);
                term.heartbeat=env.interval(()=>send({type:'heartbeat'}),20000);
            });
            ws.addEventListener('message',message=>{
                if(term.socket!==ws||transport!==term) return;
                try {
                    const event=JSON.parse(message.data);
                    if(typeof event.type==='string') {term.channel?.postMessage({kind:'event',event});handleEvent(event);}
                } catch { /* REST remains authoritative after malformed frames. */ }
            });
            ws.addEventListener('close',()=>{
                if(term.socket!==ws||transport!==term) return;
                term.socket=null;term.open=false;env.clearInterval(term.heartbeat);
                term.channel?.postMessage({kind:'close'});
                term.retry=env.delay(()=>openSocket(term),Math.min(30000,1000*2**Math.min(term.attempt++,5)));
                notify();
            });
            ws.addEventListener('error',()=>ws.close());
        }
        function stopTransport() {
            const previous=transport;transport=null;
            if(previous) {
                if(previous.leader) previous.channel?.postMessage({kind:'close'});
                previous.controller?.abort();previous.release?.();
                env.cancel(previous.retry);env.clearInterval(previous.heartbeat);
                previous.socket?.close(1000,'scope changed');previous.channel?.close();
            }
        }
        async function connect() {
            audienceChanged();
            if(disposed||transport||env.session.state.publicProject!==null||!env.session.state.user||!env.token()) return;
            const term={generation,leader:false,open:false,socket:null,attempt:0,controller:new AbortController()};transport=term;
            if(env.locks&&env.channel&&env.fingerprint) {
                const fingerprint=await env.fingerprint(env.token());
                if(transport!==term||!valid(term.generation)) return;
                term.channel=env.channel(`lific-topcoat-sync-${fingerprint}`);
                term.channel.addEventListener('message',message=>{
                    if(transport!==term||!valid(term.generation)) return;
                    const data=message.data;
                    if(term.leader) {
                        if(data.kind==='hello'&&term.open) term.channel.postMessage({kind:'open'});
                        if(data.kind==='outbound'&&['resume','activity.baseline.request','heartbeat'].includes(data.frame?.type)) send(data.frame);
                    } else if(data.kind==='open') opened(term);
                    else if(data.kind==='close') {term.open=false;notify();}
                    else if(data.kind==='event') handleEvent(data.event);
                });
                term.channel.postMessage({kind:'hello'});
                env.locks.request(`lific-topcoat-sync-${fingerprint}`,{mode:'exclusive',signal:term.controller.signal},()=>new Promise(resolve=>{
                    if(transport===term&&valid(term.generation)) {term.release=resolve;term.leader=true;openSocket(term);}else resolve();
                })).catch(error=>{if(transport===term&&error.name!=='AbortError') {stopTransport();env.error?.(error);}});
            } else {term.leader=true;openSocket(term);}
        }
        return {
            async ensureProject(id) {const model=ownedModel(id);await reconcileProject(model);return valid(model.term)?view(model):null;},
            peekProject(id) {audienceChanged();const model=models.get(id);return model?view(model):null;},
            setActiveProject(id) {audienceChanged();if(id!==null)integer(id,'project id');active=id;const model=models.get(id);if(model?.status==='ready')send({type:'resume',project_id:id,cursor:model.cursor});},
            async refreshProject(id) {
                audienceChanged();const model=models.get(id);if(!model)return;
                env.cancel(model.timer);model.timer=null;
                if(model.pending) {
                    model.queued=true;await model.pending;
                    if(!valid(model.term))return;
                    if(model.queued) {
                        model.queued=false;env.cancel(model.timer);model.timer=null;
                        await reconcileProject(model);
                    } else if(model.pending) await model.pending;
                } else await reconcileProject(model);
            },
            focus() {audienceChanged();const model=models.get(active);if(model)schedule(model);void connect();},
            subscribe(listener) {listeners.add(listener);return ()=>listeners.delete(listener);},
            audienceChanged,connect,handleEvent,disconnect:stopTransport,
            get state() {return {connected:transport?.open===true,leader:transport?.leader===true,activityBaseline};},
            dispose() {disposed=true;generation++;stopTransport();for(const model of models.values()){env.cancel(model.timer);model.controller?.abort();}models.clear();listeners.clear();}
        };
    }
    globalThis.LificSync={createClient};
    if(typeof window==='undefined') return;
    const client=createClient({session:window.lificSession,token:()=>localStorage.getItem('lific_token'),
        delay:window.setTimeout.bind(window),cancel:window.clearTimeout.bind(window),
        interval:window.setInterval.bind(window),clearInterval:window.clearInterval.bind(window),
        socket:url=>new WebSocket(url),locks:navigator.locks,
        channel:typeof BroadcastChannel==='function'?name=>new BroadcastChannel(name):null,
        fingerprint:globalThis.crypto?.subtle?async token=>Array.from(new Uint8Array(await crypto.subtle.digest('SHA-256',new TextEncoder().encode(token))),byte=>byte.toString(16).padStart(2,'0')).join(''):null,
        websocketUrl:()=>`${location.protocol==='https:'?'wss:':'ws:'}//${location.host}/api/events/ws`,
        notify:()=>window.dispatchEvent(new CustomEvent('lific:sync-change')),
        event:event=>window.dispatchEvent(new CustomEvent('lific:realtime',{detail:event}))});
    window.lificSync=client;
    window.addEventListener('lific:account-change',()=>{client.audienceChanged();void client.connect();});
    window.addEventListener('lific:session-change',()=>{client.audienceChanged();void client.connect();});
    window.addEventListener('lific:scope-change',()=>{client.audienceChanged();void client.connect();});
    window.addEventListener('storage',event=>{if(event.key==='lific_token'||event.key===null){client.audienceChanged();void client.connect();}});
    window.addEventListener('focus',()=>client.focus());
    window.addEventListener('online',()=>client.focus());
    document.addEventListener('visibilitychange',()=>{if(!document.hidden)client.focus();});
    window.addEventListener('pagehide',()=>client.disconnect());
    window.addEventListener('pageshow',()=>client.focus());
    void client.connect();
})();

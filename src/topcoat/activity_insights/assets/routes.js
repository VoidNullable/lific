(() => {
  'use strict';
  const model=globalThis.LificTopcoatAnalyticsModel;
  const escapeHtml=value=>String(value??'').replace(/[&<>"']/g,c=>({'&':'&amp;','<':'&lt;','>':'&gt;','"':'&quot;',"'":'&#39;'}[c]));
  const identity=session=>session?.state?.publicProject==null&&session?.state?.user?`private:${session.state.user.id}`:null;
  const editable=role=>role&&(!role.enforced||role.is_admin||['lead','maintainer','admin'].includes(role.role));
  const href=(project,issue)=>`/${encodeURIComponent(project)}/issues/${encodeURIComponent(issue.identifier)}`;
  const relationName=type=>type==='duplicate'?'duplicates':type==='blocks'?'blocks':'relates to';
  const ACTIVITY_PAGE=50;
  const geometry={nodeWidth:200,nodeHeight:58,gapX:90,gapY:18,componentGap:48};
  function queryOf(win){const hash=win.location.hash;return new URLSearchParams((hash.startsWith('#/')?hash.split('?')[1]:win.location.search.slice(1))||'');}
  class Controller {
    constructor(root,{window:win=globalThis.window,session=win.lificSession}={}){
      this.root=root;this.win=win;this.doc=root.ownerDocument;this.session=session;this.mode=root.dataset.topcoatAnalytics;this.identifier=root.dataset.projectIdentifier;
      this.content=root.querySelector('[data-analytics-content]');this.status=root.querySelector('[data-analytics-status]');this.error=root.querySelector('[data-analytics-error]');
      this.audience=identity(session);this.generation=0;this.disposed=false;this.project=null;this.role=null;this.items=[];this.actors=[];this.issues=[];this.relations=[];this.data=null;
      this.hasMore=false;this.loadingMore=false;this.busy=false;this.refreshing=false;this.refreshPending=false;this.graphMutationEpoch=0;this.listeners=[];this.aborters=new Set();this.dialog=null;this.previewTurn=0;
      const query=queryOf(win);this.filters={actor:query.get('actor')||'all',query:query.get('q')||'',start:query.get('start')||'',end:query.get('end')||''};
      this.weeks=12;this.showClosed=query.get('closed')==='1';this.view=query.get('view')==='unlinked'?'unlinked':'linked';this.transform={x:24,y:24,scale:1};
      const listen=(target,type,handler)=>{target.addEventListener(type,handler);this.listeners.push(()=>target.removeEventListener(type,handler));};
      listen(root,'click',event=>{const link=event.target.closest('a[href]');if(link&&root.contains(link)&&!event.ctrlKey&&!event.metaKey&&!event.shiftKey&&!event.altKey&&event.button===0){event.preventDefault();this.navigate(link.getAttribute('href'));}});
      for(const name of ['lific:account-change','lific:session-change','lific:scope-change'])listen(win,name,()=>this.transition());
      listen(win,'pagehide',event=>{if(!event.persisted)this.dispose();});
      listen(win,'pageshow',event=>{if(event.persisted)this.restore();});
      listen(root,'keydown',event=>{if(event.key==='Escape'&&this.preview){event.preventDefault();this.hidePreview();}});
      if(this.mode!=='insights'){
        listen(win,'lific:realtime',event=>this.realtime(event.detail));
        listen(win,'focus',()=>this.scheduleRefresh());
        listen(this.doc,'visibilitychange',()=>{if(!this.doc.hidden)this.scheduleRefresh();});
      }
      if(this.mode==='activity')this.baseline=win.setInterval(()=>{if(!this.doc.hidden)this.scheduleRefresh();},15000);
      void this.load();
    }
    current(turn){return !this.disposed&&turn===this.generation&&this.audience===identity(this.session);}
    async request(path,options={}){
      const aborter=new AbortController();this.aborters.add(aborter);
      try{const response=await this.session.request(path,{...options,signal:aborter.signal});if(!response?.ok){const error=new Error(response?.error||'Request failed');error.status=response?.status;throw error;}return response.data;}
      finally{this.aborters.delete(aborter);}
    }
    async graphIssues(project,turn=this.generation){
      const issues=[];
      for(let offset=0;;offset+=500){
        const page=await this.request(`/issues?project_id=${project.id}&limit=500${offset?`&offset=${offset}`:''}`);
        if(!this.current(turn))return [];
        issues.push(...page);
        if(page.length<500)return issues;
      }
    }
    clear(){
      this.items=[];this.actors=[];this.issues=[];this.relations=[];this.data=null;this.project=null;this.role=null;this.hasMore=false;
      this.content.replaceChildren();this.error.hidden=true;this.closeDialog();this.hidePreview();this.loadingMore=false;this.busy=false;
    }
    transition(){const next=identity(this.session);if(next===this.audience)return;this.audience=next;this.generation++;for(const aborter of this.aborters)aborter.abort();this.clear();void this.load();}
    restore(){this.disposed=false;this.audience=identity(this.session);this.generation++;for(const aborter of this.aborters)aborter.abort();this.aborters.clear();this.win.clearTimeout(this.refreshTimer);this.refreshTimer=null;this.clear();void this.load();}
    async load(){
      const turn=++this.generation;this.clear();this.root.setAttribute('aria-busy','true');this.status.textContent=`Loading ${this.mode}…`;
      try{
        if(!this.audience)throw new Error('Sign in to view project data.');
        const projects=await this.request('/projects');if(!this.current(turn))return;
        const project=projects.find(row=>row.identifier.toLowerCase()===this.identifier.toLowerCase());if(!project)throw new Error(`Project ${this.identifier} not found`);
        this.project=project;
        if(this.mode==='graph'){
          const [role,issues,relations]=await Promise.all([this.request(`/projects/${project.id}/my-role`),this.graphIssues(project,turn),this.request(`/projects/${project.id}/relations`)]);
          if(!this.current(turn))return;this.role=role;this.issues=issues.filter(row=>Number(row.project_id)===Number(project.id));this.relations=relations;
        }else if(this.mode==='insights'){
          const data=await this.request(`/projects/${project.id}/insights?weeks=${this.weeks}`);if(!this.current(turn))return;this.data=data;
        }else {
          const [feed,actors]=await Promise.all([this.request(`/projects/${project.id}/activity?limit=${ACTIVITY_PAGE}&offset=0`),this.request(`/projects/${project.id}/activity/actors`)]);
          if(!this.current(turn))return;this.items=feed.items;this.hasMore=feed.has_more;this.actors=actors;
        }
        if(this.mode!=='insights')this.win.lificSync?.setActiveProject?.(project.id);
        this.render();this.root.setAttribute('aria-busy','false');this.status.textContent='';
      }catch(error){if(this.current(turn)){this.clear();this.root.setAttribute('aria-busy','false');this.status.textContent='';this.showError(error,true);}}
    }
    showError(error,retry=false){this.error.replaceChildren();this.error.hidden=false;this.error.append(error.message||String(error));if(retry){const button=this.doc.createElement('button');button.type='button';button.textContent='Retry';button.addEventListener('click',()=>void this.load());this.error.append(' ',button);}}
    saveQuery(values){
      const location=this.win.location,url=new URL(location.href);const hashed=location.hash.startsWith('#/');
      const route=hashed?location.hash.slice(1).split('?')[0]:url.pathname,query=queryOf(this.win);
      for(const [key,value] of Object.entries(values)){if(value!==''&&value!=null)query.set(key,value);else query.delete(key);}
      if(hashed)url.hash=`${route}${query.size?'?'+query:''}`;else url.search=query.toString();this.win.history.replaceState(this.win.history.state,'',url);
    }
    navigate(path){this.win.dispatchEvent(new this.win.CustomEvent('lific:navigate',{detail:{href:path,history:'push'}}));}
    render(){if(this.mode==='activity')this.renderActivity();else if(this.mode==='insights')this.renderInsights();else this.renderGraph();}
    realtime(event){if(event?.type==='resync.required'){this.generation++;this.clear();void this.load();}else if(event?.project_id===this.project?.id)this.scheduleRefresh();}
    scheduleRefresh(){
      if(this.disposed||!this.project||this.mode==='insights')return;
      this.refreshPending=true;if(this.refreshTimer||this.refreshing||this.busy||this.loadingMore||this.dialog||this.doc.hidden)return;
      this.refreshTimer=this.win.setTimeout(()=>{this.refreshTimer=null;void this.refresh();},100);
    }
    async refresh(){
      if(this.disposed||!this.project||this.mode==='insights')return;
      if(this.refreshing||this.busy||this.loadingMore||this.dialog||this.doc.hidden){this.refreshPending=true;return;}
      this.refreshing=true;this.refreshPending=false;const turn=this.generation,project=this.project,mutationEpoch=this.graphMutationEpoch;
      try{
        if(this.mode==='activity'){
          const [feed,actors]=await Promise.all([this.request(`/projects/${project.id}/activity?limit=${ACTIVITY_PAGE}&offset=0`),this.request(`/projects/${project.id}/activity/actors`)]);
          if(!this.current(turn))return;if(this.busy||this.loadingMore||this.dialog){this.refreshPending=true;return;}const known=new Set(feed.items.map(row=>row.id));this.items=[...feed.items,...this.items.filter(row=>!known.has(row.id))];this.actors=actors;
          if(this.items.length<=ACTIVITY_PAGE)this.hasMore=feed.has_more;this.renderActivityRows();this.renderActors();
        }else {
          const [role,issues,relations]=await Promise.all([this.request(`/projects/${project.id}/my-role`),this.graphIssues(project,turn),this.request(`/projects/${project.id}/relations`)]);
          if(!this.current(turn))return;if(this.graphMutationEpoch!==mutationEpoch||this.busy||this.dialog){this.refreshPending=true;return;}this.role=role;this.issues=issues.filter(row=>Number(row.project_id)===Number(project.id));this.relations=relations;this.renderGraph();
        }
      }catch(error){if(this.current(turn)){if([401,403,404].includes(error.status)){this.clear();this.showError(error,true);}else this.showError(error,true);}}
      finally{this.refreshing=false;if(this.refreshPending)this.scheduleRefresh();}
    }
    renderActivity(){
      const f=this.filters;
      this.content.innerHTML=`<header><h1>Activity</h1><a href="/${encodeURIComponent(this.identifier)}/overview">Project overview</a></header>
        <form class="tc-analytics__filters" data-activity-filters><label>Search activity<input type="search" name="q" value="${escapeHtml(f.query)}"></label>
          <label>From date<input type="date" name="start" value="${escapeHtml(f.start)}"></label><label>To date<input type="date" name="end" value="${escapeHtml(f.end)}"></label>
          <button type="button" data-activity-clear>Clear filters</button></form>
        <div class="tc-activity__columns"><section aria-label="Project audit feed"><p data-activity-count></p><div data-activity-rows></div><button type="button" data-activity-more>Load more</button></section>
          <aside aria-label="Project actors"><h2>Actors · all time</h2><div data-activity-actors></div></aside></div>`;
      this.content.querySelector('[data-activity-filters]').addEventListener('submit',event=>event.preventDefault());
      this.content.querySelector('[data-activity-filters]').addEventListener('input',event=>{
        const field=event.target.name;if(!['q','start','end'].includes(field))return;this.filters[field==='q'?'query':field]=event.target.value;this.saveQuery({[field]:event.target.value});this.renderActivityRows();});
      this.content.querySelector('[data-activity-clear]').addEventListener('click',()=>{this.filters={actor:'all',query:'',start:'',end:''};this.saveQuery({actor:'',q:'',start:'',end:''});this.renderActivity();});
      this.content.querySelector('[data-activity-more]').addEventListener('click',()=>void this.loadMore());this.renderActors();this.renderActivityRows();
    }
    renderActors(){
      const root=this.content.querySelector('[data-activity-actors]');if(!root)return;
      const focused=root.contains(this.doc.activeElement)?this.doc.activeElement.dataset.actor:null;
      root.innerHTML=`<button type="button" data-actor="all" aria-pressed="${this.filters.actor==='all'}">Everyone</button>${this.actors.map(actor=>{
        const key=actor.actor_user_id==null?'system':String(actor.actor_user_id);return `<button type="button" data-actor="${key}" aria-pressed="${this.filters.actor===key}">${escapeHtml(model.actorName(actor))}${actor.is_bot?' · agent':''} · ${actor.actions} actions <small>${escapeHtml(actor.top_transport)} · ${escapeHtml(actor.last_ts)}</small></button>`;}).join('')}`;
      root.querySelectorAll('[data-actor]').forEach(button=>button.addEventListener('click',()=>{this.filters.actor=this.filters.actor===button.dataset.actor?'all':button.dataset.actor;this.saveQuery({actor:this.filters.actor==='all'?'':this.filters.actor});this.renderActors();this.renderActivityRows();this.content.querySelector(`[data-actor="${this.filters.actor}"]`)?.focus();}));
      if(focused)root.querySelector(`[data-actor="${focused}"]`)?.focus();
    }
    renderActivityRows(){
      const root=this.content.querySelector('[data-activity-rows]');if(!root)return;
      const expanded=[...root.querySelectorAll('details[open]')].map(node=>node.dataset.activityId),focus=this.doc.activeElement?.closest('[data-activity-id]')?.dataset.activityId;
      const rows=model.filterActivity(this.items,{...this.filters,projectId:this.project.id});
      const groups=new Map();for(const row of rows){const day=model.timestamp(row.ts).toLocaleDateString(undefined,{weekday:'long',month:'short',day:'numeric',year:'numeric'});if(!groups.has(day))groups.set(day,[]);groups.get(day).push(row);}
      root.innerHTML=rows.length?[...groups].map(([day,entries])=>`<section><h2>${escapeHtml(day)}</h2>${entries.map(row=>{
        const dest=model.activityHref(this.identifier,row),standing=this.actors.findIndex(actor=>actor.actor_user_id===row.actor_user_id);
        const diff=model.diffLines(row.old_value||'',row.new_value||''),multiline=(row.old_value||'').includes('\n')||(row.new_value||'').includes('\n');
        return `<details data-activity-id="${row.id}" ${expanded.includes(String(row.id))?'open':''}><summary>${escapeHtml(model.actorName(row))}${row.actor_is_bot?' · agent':''} ${escapeHtml(model.activityVerb(row))} ${escapeHtml(row.entity_label||`#${row.entity_id}`)} <time datetime="${model.timestamp(row.ts).toISOString()}">${escapeHtml(row.ts)}</time>
          ${row.action==='update'&&!multiline?` · ${escapeHtml(row.old_value||'(none)')} → ${escapeHtml(row.new_value||'(none)')}`:''}</summary>
          <dl><dt>When</dt><dd>${escapeHtml(model.timestamp(row.ts).toLocaleString())} · ${escapeHtml(row.ts)} UTC</dd><dt>Who</dt><dd>${escapeHtml(model.actorName(row))} ${escapeHtml(row.actor_username||'')} via ${escapeHtml(row.transport)}${standing>=0?` · ${this.actors[standing].actions} actions · rank ${standing+1}`:''}</dd><dt>Entity</dt><dd>${dest?`<a href="${escapeHtml(dest)}">${escapeHtml(row.entity_label||`#${row.entity_id}`)}</a>`:escapeHtml(row.entity_label||`#${row.entity_id}`)}</dd></dl>
          ${multiline&&diff!==null?`<pre class="tc-activity__diff" aria-label="Changed lines">${model.foldContext(diff).map(line=>line.kind==='fold'?`<span>… ${line.count} unchanged lines …</span>`:`<span class="tc-activity__${line.kind}">${line.kind==='added'?'+ ':line.kind==='removed'?'- ':'  '}${escapeHtml(line.text)}</span>`).join('')}</pre>`:`<div class="tc-activity__values"><div><h3>Before</h3><pre>${escapeHtml(row.old_value||'(none)')}</pre></div><div><h3>After</h3><pre>${escapeHtml(row.new_value||'(none)')}</pre></div></div>`}</details>`;}).join('')}</section>`).join(''):`<p>${this.items.length?'No matching activity in the loaded history.':'No activity yet.'}</p>`;
      if(focus)root.querySelector(`[data-activity-id="${focus}"] summary`)?.focus();
      const more=this.content.querySelector('[data-activity-more]');more.hidden=!this.hasMore;more.disabled=this.loadingMore;more.textContent=this.loadingMore?'Loading…':'Load more';
      this.content.querySelector('[data-activity-count]').textContent=`${rows.length} matching entries${this.hasMore?' · older history available':''}`;
    }
    async loadMore(){
      if(this.loadingMore||!this.project||!this.hasMore)return;const turn=this.generation;this.loadingMore=true;this.renderActivityRows();
      try{const feed=await this.request(`/projects/${this.project.id}/activity?limit=${ACTIVITY_PAGE}&offset=${this.items.length}`);if(!this.current(turn))return;
        const known=new Set(this.items.map(row=>row.id));this.items.push(...feed.items.filter(row=>!known.has(row.id)));this.hasMore=feed.has_more;this.error.hidden=true;
      }catch(error){if(this.current(turn))this.showError(error);}
      finally{if(this.current(turn)){this.loadingMore=false;this.renderActivityRows();if(this.refreshPending)this.scheduleRefresh();}}
    }
    renderInsights(){
      const data=this.data,series=model.insightSeries(data),any=data.status_counts.total>0;
      const distribution=(title,rows,overflow=0)=>`<section class="tc-analytics__card"><h2>${title}</h2>${rows.length?`<ul>${rows.map(row=>`<li><span>${escapeHtml(row.label)}</span><meter min="0" max="${Math.max(1,...rows.map(row=>row.count))}" value="${row.count}" aria-label="${escapeHtml(row.label)}">${row.count}</meter><strong>${row.count}</strong></li>`).join('')}</ul>`:'<p>No modules yet.</p>'}${overflow?`<p>+${overflow} more</p>`:''}</section>`;
      this.content.innerHTML=`<header><h1>Insights</h1><nav aria-label="Insight window">${[4,12,26,52].map(weeks=>`<button type="button" data-weeks="${weeks}" aria-pressed="${this.weeks===weeks}">${weeks}w</button>`).join('')}</nav></header>
        ${!any?'<p>Nothing to chart yet. Insights fills in once this project has issues to measure.</p>':`<section class="tc-analytics__card"><h2>Created vs. closed</h2><p>Last ${data.weeks} weeks · Created <span class="tc-trend-created">●</span> · Closed <span class="tc-trend-closed">●</span></p><div data-trend-chart></div>
          <details><summary>Weekly data</summary><table><caption>Issues created and closed each week</caption><thead><tr><th>Week beginning</th><th>Created</th><th>Closed</th></tr></thead><tbody>${series.map(row=>`<tr><th>${escapeHtml(row.week)}</th><td>${row.created??'Unavailable'}</td><td>${row.closed??'Unavailable'}</td></tr>`).join('')}</tbody></table></details></section>
          <div class="tc-insights__distributions">${distribution('Status',['backlog','todo','active','done','cancelled'].map(key=>({label:key,count:data.status_counts[key]})))}
            ${distribution('Priority',['urgent','high','medium','low','none'].map(key=>({label:key,count:data.priority_counts[key]})))}
            ${distribution('Module',data.module_counts.slice(0,6).map(row=>({label:row.name,count:row.count})),Math.max(0,data.module_counts.length-6))}</div>
          <section class="tc-analytics__card"><h2>Top actors · last ${data.weeks} weeks</h2>${data.top_actors.length?`<ol>${data.top_actors.map(actor=>`<li>${escapeHtml(model.actorName(actor))}${actor.is_bot?' · agent':''} · ${actor.actions} actions · ${escapeHtml(actor.top_transport)} <time>${escapeHtml(actor.last_ts)}</time></li>`).join('')}</ol>`:'<p>No activity in this window.</p>'}</section>`}`;
      this.content.querySelectorAll('[data-weeks]').forEach(button=>button.addEventListener('click',()=>void this.pickWeeks(Number(button.dataset.weeks))));if(any)this.renderTrend(series);
    }
    renderTrend(series){
      const root=this.content.querySelector('[data-trend-chart]');const ticks=model.niceTicks(Math.max(0,...series.flatMap(row=>[row.created??0,row.closed??0]))),max=ticks.at(-1)||1;
      const x=index=>series.length<=1?340:30+index*630/(series.length-1),y=count=>190-count/max*170;
      const segments=key=>{const parts=[];let current=[];for(let index=0;index<series.length;index++){const count=series[index][key];if(count===null){if(current.length)parts.push(current);current=[];}else current.push({x:x(index),y:y(count)});}if(current.length)parts.push(current);return parts;};
      root.innerHTML=`${!series.length?'<p>No history in this window.</p>':''}<svg viewBox="0 0 680 220" role="img" aria-label="Issues created vs closed per week; weekly values available in the data table">
        ${ticks.map(tick=>`<line x1="30" y1="${y(tick)}" x2="660" y2="${y(tick)}" stroke="var(--border)"/><text x="24" y="${y(tick)+3}" text-anchor="end">${tick}</text>`).join('')}
        ${['created','closed'].map(key=>segments(key).map(points=>`<path d="${model.smoothPath(points)}" class="tc-trend-${key}" fill="none" stroke="currentColor" stroke-width="2"/>${points.map(point=>`<circle cx="${point.x}" cy="${point.y}" r="3" class="tc-trend-${key}" fill="currentColor"/>`).join('')}`).join('')).join('')}
        ${series.map((row,index)=>`<g tabindex="0" role="img" aria-label="${escapeHtml(row.week)}: ${row.created??'unavailable'} created, ${row.closed??'unavailable'} closed"><title>${escapeHtml(row.week)}: ${row.created??'unavailable'} created, ${row.closed??'unavailable'} closed</title><rect x="${x(index)-8}" y="10" width="16" height="185" fill="transparent" stroke="transparent"/>${index===0||index===series.length-1?`<text x="${x(index)}" y="213" text-anchor="middle">${escapeHtml(row.week)}</text>`:''}</g>`).join('')}</svg>`;
      const observed=series.flatMap(row=>[row.created,row.closed]).filter(Number.isFinite);
      if(series.length&&series.every(row=>Number.isFinite(row.created)&&Number.isFinite(row.closed)&&row.created===0&&row.closed===0))root.insertAdjacentHTML('beforeend','<p>No issues created or closed in this window.</p>');
      else if(observed.length<series.length*2)root.insertAdjacentHTML('beforeend','<p>Some trend values are unavailable in this window.</p>');
    }
    async pickWeeks(weeks){
      if(weeks===this.weeks||this.busy)return;const turn=++this.generation;this.weeks=weeks;this.busy=true;this.root.setAttribute('aria-busy','true');this.status.textContent='Loading insights…';this.error.hidden=true;
      this.content.querySelectorAll('[data-weeks]').forEach(button=>button.disabled=true);
      try{const data=await this.request(`/projects/${this.project.id}/insights?weeks=${weeks}`);if(!this.current(turn))return;this.data=data;this.renderInsights();this.content.querySelector(`[data-weeks="${weeks}"]`).focus();}
      catch(error){if(this.current(turn)){this.content.replaceChildren();this.data=null;this.showError(error,true);}}
      finally{if(this.current(turn)){this.busy=false;this.root.setAttribute('aria-busy','false');this.status.textContent='';}}
    }
    renderGraph(){
      const active=this.doc.activeElement,focusedHref=this.content.contains(active)?active.getAttribute('href'):null;
      const partition=model.graphPartition(this.issues,this.relations,this.showClosed),edit=editable(this.role),rows=partition[this.view];
      const edges=partition.relations.map(row=>({source:row.source_id,target:row.target_id}));
      const layout=this.view==='linked'?model.layoutGraph(rows.map(row=>row.id),partition.relations.filter(row=>row.relation_type==='blocks').map(row=>({source:row.source_id,target:row.target_id})),geometry,edges):model.layoutGrid(rows.map(row=>row.id),{...geometry,gapX:24,gapY:16});
      this.positions=layout.positions;this.partition=partition;this.graphRows=rows;
      this.hidePreview();this.closeDialog();
      this.content.innerHTML=`<header><h1>Dependency graph</h1><div class="tc-analytics__filters"><button type="button" data-graph-view="linked" aria-pressed="${this.view==='linked'}">Linked (${partition.linked.length})</button><button type="button" data-graph-view="unlinked" aria-pressed="${this.view==='unlinked'}">Unlinked (${partition.unlinked.length})</button>
        <label><input type="checkbox" data-graph-closed ${this.showClosed?'checked':''}>Show closed</label>${edit?'<button type="button" data-graph-connect>Create relation</button>':''}</div></header>
        ${!this.issues.length?'<p>Nothing to graph yet. Create some issues first, then link them up.</p>':`<div class="tc-graph__controls" aria-label="Graph navigation"><button type="button" data-graph-zoom="in" aria-label="Zoom in">+</button><button type="button" data-graph-zoom="out" aria-label="Zoom out">−</button><button type="button" data-graph-fit>Fit graph</button><button type="button" data-graph-reset>Reset graph</button><output data-graph-scale></output></div>
          ${!rows.length?`<p>${this.view==='linked'?'Nothing linked in this view. Switch to Unlinked to connect issues.':'Everything in this view is linked.'}</p>`:''}
          <div class="tc-graph__viewport" data-graph-viewport tabindex="0" role="region" aria-label="Dependency graph canvas. Arrow keys pan; plus and minus zoom; Home fits graph."><div class="tc-graph__scene" data-graph-scene style="width:${Math.max(200,layout.width)}px;height:${Math.max(58,layout.height)}px">
            <svg class="tc-graph__edges" data-graph-edges width="${Math.max(200,layout.width)}" height="${Math.max(58,layout.height)}" aria-hidden="true"><defs><marker id="tc-graph-arrow" viewBox="0 0 10 10" refX="9" refY="5" markerWidth="7" markerHeight="7" orient="auto-start-reverse"><path d="M 0 0 L 10 5 L 0 10 z" fill="currentColor"/></marker></defs></svg>
            ${rows.map(issue=>{const point=layout.positions.get(issue.id);return `<article class="tc-graph__node" data-graph-node="${issue.id}" style="left:${point.x}px;top:${point.y}px">
              ${edit?`<button type="button" data-graph-target="${issue.id}" class="tc-graph__handle tc-graph__handle--target" aria-label="Connect to ${escapeHtml(issue.identifier)}">●</button>`:''}
              <a href="${href(this.identifier,issue)}" data-graph-issue="${issue.id}"><strong>${escapeHtml(issue.identifier)} · ${escapeHtml(issue.status)}</strong><span>${escapeHtml(issue.title)}</span></a>
              ${edit?`<button type="button" data-graph-source="${issue.id}" class="tc-graph__handle tc-graph__handle--source" aria-label="Connect from ${escapeHtml(issue.identifier)}">●</button>`:''}</article>`;}).join('')}</div></div>
          <section class="tc-analytics__card" aria-label="Graph text alternative"><h2>Issues and relations</h2><p>Every node and relation is available here for keyboard navigation.</p>
            <ul>${rows.map(issue=>`<li><a href="${href(this.identifier,issue)}">${escapeHtml(issue.identifier)} · ${escapeHtml(issue.title)}</a> · ${escapeHtml(issue.status)}${edit?` <button type="button" data-graph-connect-from="${issue.id}">Connect ${escapeHtml(issue.identifier)}</button>`:''}</li>`).join('')}</ul>
            ${this.view==='linked'?`<ul data-graph-relation-list>${partition.relations.map((relation,index)=>`<li><a href="/${encodeURIComponent(this.identifier)}/issues/${encodeURIComponent(relation.source_identifier)}">${escapeHtml(relation.source_identifier)}</a> ${relationName(relation.relation_type)} <a href="/${encodeURIComponent(this.identifier)}/issues/${encodeURIComponent(relation.target_identifier)}">${escapeHtml(relation.target_identifier)}</a>${edit?` <button type="button" data-graph-edge="${index}">Manage relation ${escapeHtml(relation.source_identifier)} ${relationName(relation.relation_type)} ${escapeHtml(relation.target_identifier)}</button>`:''}</li>`).join('')}</ul>`:''}</section>`}`;
      this.content.querySelectorAll('[data-graph-view]').forEach(button=>button.addEventListener('click',()=>{this.view=button.dataset.graphView;this.transform={x:24,y:24,scale:1};this.saveQuery({view:this.view==='linked'?'':this.view});this.renderGraph();this.content.querySelector(`[data-graph-view="${this.view}"]`).focus();}));
      this.content.querySelector('[data-graph-closed]').addEventListener('change',event=>{this.showClosed=event.target.checked;this.saveQuery({closed:this.showClosed?'1':''});this.renderGraph();this.content.querySelector('[data-graph-closed]').focus();});
      this.content.querySelectorAll('[data-graph-connect],[data-graph-connect-from],[data-graph-source]').forEach(button=>button.addEventListener('click',()=>this.connectDialog(Number(button.dataset.graphConnectFrom||button.dataset.graphSource)||null)));
      this.content.querySelectorAll('[data-graph-edge]').forEach(button=>button.addEventListener('click',()=>this.edgeDialog(partition.relations[Number(button.dataset.graphEdge)])));
      this.content.querySelectorAll('[data-graph-target]').forEach(button=>button.addEventListener('click',()=>this.connectDialog(null,Number(button.dataset.graphTarget))));
      this.content.querySelectorAll('[data-graph-zoom]').forEach(button=>button.addEventListener('click',()=>this.zoom(button.dataset.graphZoom==='in'?1.2:1/1.2)));
      this.content.querySelector('[data-graph-fit]')?.addEventListener('click',()=>this.fitGraph());
      this.content.querySelector('[data-graph-reset]')?.addEventListener('click',()=>{this.transform={x:24,y:24,scale:1};this.applyTransform();});
      this.drawEdges();this.applyTransform();this.bindCanvas();
      if(focusedHref)[...this.content.querySelectorAll('a[href]')].find(link=>link.getAttribute('href')===focusedHref)?.focus();
    }
    drawEdges(){
      const svg=this.content.querySelector('[data-graph-edges]');if(!svg)return;svg.querySelectorAll('[data-edge-path]').forEach(node=>node.remove());
      if(this.view!=='linked')return;
      this.partition.relations.forEach((relation,index)=>{
        const source=this.positions.get(relation.source_id),target=this.positions.get(relation.target_id);if(!source||!target)return;
        const start={x:source.x+200,y:source.y+29},end={x:target.x,y:target.y+29},bend=Math.max(40,Math.abs(end.x-start.x)/2);
        const path=this.doc.createElementNS('http://www.w3.org/2000/svg','path');path.dataset.edgePath=String(index);
        path.setAttribute('d',`M ${start.x} ${start.y} C ${start.x+bend} ${start.y}, ${end.x-bend} ${end.y}, ${end.x} ${end.y}`);path.setAttribute('fill','none');path.setAttribute('stroke','currentColor');path.setAttribute('stroke-width','2');
        if(relation.relation_type!=='relates_to')path.setAttribute('marker-end','url(#tc-graph-arrow)');
        if(relation.relation_type!=='blocks')path.setAttribute('stroke-dasharray',relation.relation_type==='duplicate'?'2 3':'5 4');
        const title=this.doc.createElementNS('http://www.w3.org/2000/svg','title');title.textContent=`${relation.source_identifier} ${relationName(relation.relation_type)} ${relation.target_identifier}`;path.append(title);
        if(editable(this.role)){path.style.cursor='pointer';path.addEventListener('click',event=>{event.stopPropagation();this.edgeDialog(relation);});}svg.append(path);
      });
    }
    bindCanvas(){
      const viewport=this.content.querySelector('[data-graph-viewport]');if(!viewport)return;
      let drag=null,connection=null,moved=false;
      viewport.addEventListener('keydown',event=>{if(event.target!==viewport)return;const offsets={ArrowLeft:[40,0],ArrowRight:[-40,0],ArrowUp:[0,40],ArrowDown:[0,-40]};
        if(offsets[event.key]){event.preventDefault();this.transform.x+=offsets[event.key][0];this.transform.y+=offsets[event.key][1];this.applyTransform();}
        else if(['+','=','-','Home'].includes(event.key)){event.preventDefault();if(event.key==='Home')this.fitGraph();else this.zoom(event.key==='-'?1/1.2:1.2);}});
      viewport.addEventListener('wheel',event=>{event.preventDefault();this.hidePreview();this.zoom(event.deltaY<0?1.1:1/1.1,{x:event.clientX-viewport.getBoundingClientRect().left,y:event.clientY-viewport.getBoundingClientRect().top});},{passive:false});
      viewport.addEventListener('pointerdown',event=>{
        if(event.button!==0)return;this.hidePreview();moved=false;
        const source=event.target.closest('[data-graph-source]');
        if(source&&editable(this.role)){connection={source:Number(source.dataset.graphSource),x:event.clientX,y:event.clientY};return;}
        if(event.target.closest('button,[data-edge-path]'))return;
        const node=event.target.closest('[data-graph-node]');
        drag={x:event.clientX,y:event.clientY,id:node?Number(node.dataset.graphNode):null,point:node?{...this.positions.get(Number(node.dataset.graphNode))}:{...this.transform}};
        viewport.setPointerCapture(event.pointerId);
      });
      viewport.addEventListener('pointermove',event=>{
        if(!drag)return;const dx=event.clientX-drag.x,dy=event.clientY-drag.y;if(Math.abs(dx)+Math.abs(dy)>5)moved=true;
        if(drag.id){const point=this.positions.get(drag.id);point.x=Math.max(0,drag.point.x+dx/this.transform.scale);point.y=Math.max(0,drag.point.y+dy/this.transform.scale);
          const node=this.content.querySelector(`[data-graph-node="${drag.id}"]`);node.style.left=`${point.x}px`;node.style.top=`${point.y}px`;this.drawEdges();
        }else {this.transform.x=drag.point.x+dx;this.transform.y=drag.point.y+dy;this.applyTransform();}
      });
      viewport.addEventListener('pointerup',event=>{
        if(connection){const target=this.doc.elementFromPoint(event.clientX,event.clientY)?.closest('[data-graph-node]');const source=connection.source;connection=null;
          if(target&&Number(target.dataset.graphNode)!==source){event.preventDefault();moved=true;this.connectDialog(source,Number(target.dataset.graphNode));}}
        drag=null;
      });
      viewport.addEventListener('pointercancel',()=>{drag=null;connection=null;});
      viewport.addEventListener('click',event=>{if(moved){event.preventDefault();event.stopPropagation();moved=false;}},true);
      for(const link of viewport.querySelectorAll('[data-graph-issue]')){
        link.addEventListener('pointerenter',event=>{if(event.pointerType==='mouse')this.schedulePreview(Number(link.dataset.graphIssue),link);});
        link.addEventListener('pointerleave',()=>{this.win.clearTimeout(this.previewShow);this.previewHide=this.win.setTimeout(()=>this.hidePreview(),200);});
        link.addEventListener('focus',()=>this.schedulePreview(Number(link.dataset.graphIssue),link));
        link.addEventListener('blur',()=>{this.previewHide=this.win.setTimeout(()=>this.hidePreview(),200);});
        let press;
        link.addEventListener('pointerdown',event=>{if(event.pointerType==='touch')press=this.win.setTimeout(()=>{moved=true;void this.showPreview(Number(link.dataset.graphIssue),link);},500);});
        for(const name of ['pointerup','pointercancel','pointermove'])link.addEventListener(name,()=>this.win.clearTimeout(press));
      }
    }
    applyTransform(){const scene=this.content.querySelector('[data-graph-scene]');if(scene)scene.style.transform=`translate(${this.transform.x}px,${this.transform.y}px) scale(${this.transform.scale})`;const scale=this.content.querySelector('[data-graph-scale]');if(scale)scale.textContent=`${Math.round(this.transform.scale*100)}%`;}
    zoom(factor,anchor){const viewport=this.content.querySelector('[data-graph-viewport]');if(!viewport)return;this.hidePreview();const pivot=anchor||{x:viewport.clientWidth/2,y:viewport.clientHeight/2},previous=this.transform.scale,next=Math.min(2.5,Math.max(0.2,previous*factor));
      this.transform.x=pivot.x-(pivot.x-this.transform.x)*next/previous;this.transform.y=pivot.y-(pivot.y-this.transform.y)*next/previous;this.transform.scale=next;this.applyTransform();}
    fitGraph(){const viewport=this.content.querySelector('[data-graph-viewport]');if(!viewport||!this.positions.size)return;
      const points=[...this.positions.values()],width=Math.max(...points.map(row=>row.x))+200,height=Math.max(...points.map(row=>row.y))+58;
      const scale=Math.min(1.5,Math.max(.2,Math.min((viewport.clientWidth-48)/width,(viewport.clientHeight-48)/height)));this.transform={x:(viewport.clientWidth-width*scale)/2,y:(viewport.clientHeight-height*scale)/2,scale};this.applyTransform();}
    openDialog(title,body){
      this.closeDialog();this.hidePreview();this.dialogFocus=this.doc.activeElement;
      const dialog=this.doc.createElement('dialog');dialog.className='tc-graph__dialog';dialog.setAttribute('aria-label',title);dialog.innerHTML=`<h2>${escapeHtml(title)}</h2>${body}<p data-dialog-error role="alert" hidden></p><button type="button" data-dialog-cancel>Cancel</button>`;
      this.root.append(dialog);this.dialog=dialog;dialog.addEventListener('cancel',event=>{if(this.busy)event.preventDefault();else this.closeDialog();});dialog.querySelector('[data-dialog-cancel]').addEventListener('click',()=>{if(!this.busy)this.closeDialog();});dialog.showModal();return dialog;
    }
    closeDialog(){if(this.dialog){this.dialog.remove();this.dialog=null;if(this.dialogFocus?.isConnected)this.dialogFocus.focus();this.dialogFocus=null;}if(this.refreshPending)this.scheduleRefresh();}
    connectDialog(sourceId=null,targetId=null){
      if(!editable(this.role)||this.busy)return;
      const visible=[...this.partition.linked,...this.partition.unlinked],options=id=>visible.map(issue=>`<option value="${issue.id}" ${issue.id===id?'selected':''}>${escapeHtml(issue.identifier)} · ${escapeHtml(issue.title)}</option>`).join('');
      const dialog=this.openDialog('Create relation',`<form data-relation-create><label>Source issue<select name="source">${options(sourceId)}</select></label><label>Target issue<select name="target">${options(targetId)}</select></label>
        <label>Relation type<select name="type"><option value="blocks">blocks</option><option value="relates_to">relates to</option><option value="duplicate">duplicates</option></select></label><button type="submit">Create relation</button></form>`);
      dialog.querySelector('form').addEventListener('submit',event=>{event.preventDefault();const form=new FormData(event.currentTarget);const source=visible.find(row=>row.id===Number(form.get('source'))),target=visible.find(row=>row.id===Number(form.get('target')));
        if(!source||!target||source.id===target.id){const error=dialog.querySelector('[data-dialog-error]');error.hidden=false;error.textContent='Choose two different issues.';return;}
        void this.mutate('/issues/link',{source:source.identifier,target:target.identifier,relation_type:form.get('type')});});
    }
    edgeDialog(relation){
      if(!editable(this.role)||this.busy)return;
      const dialog=this.openDialog('Manage relation',`<p><a href="/${encodeURIComponent(this.identifier)}/issues/${encodeURIComponent(relation.source_identifier)}">${escapeHtml(relation.source_identifier)}</a> ${relationName(relation.relation_type)} <a href="/${encodeURIComponent(this.identifier)}/issues/${encodeURIComponent(relation.target_identifier)}">${escapeHtml(relation.target_identifier)}</a></p>
        ${relation.relation_type!=='relates_to'?'<button type="button" data-relation-reverse>Reverse direction</button>':''}<button type="button" data-relation-remove>Remove relation</button>`);
      const payload={source:relation.source_identifier,target:relation.target_identifier};dialog.querySelector('[data-relation-reverse]')?.addEventListener('click',()=>void this.mutate('/issues/reverse',payload));dialog.querySelector('[data-relation-remove]').addEventListener('click',()=>void this.mutate('/issues/unlink',payload));
    }
    async mutate(path,body){
      if(this.busy||!editable(this.role)||!this.project)return;const turn=this.generation;let accepted=false;this.graphMutationEpoch++;this.busy=true;this.status.textContent='Saving relation…';this.error.hidden=true;
      const controls=[...this.root.querySelectorAll('button,input,select')].map(node=>[node,node.disabled]);for(const [node]of controls)node.disabled=true;
      try{await this.request(path,{method:'POST',body:JSON.stringify(body)});if(!this.current(turn))return;accepted=true;
        const [issues,relations]=await Promise.all([this.graphIssues(this.project,turn),this.request(`/projects/${this.project.id}/relations`)]);if(!this.current(turn))return;
        this.issues=issues.filter(row=>Number(row.project_id)===Number(this.project.id));this.relations=relations;this.closeDialog();this.renderGraph();this.status.textContent='Relation saved.';
      }catch(error){if(this.current(turn)){if(accepted){this.closeDialog();this.content.replaceChildren();this.showError(new Error(`Relation saved, but the graph could not be refreshed: ${error.message}`),true);this.status.textContent='';return;}const slot=this.dialog?.querySelector('[data-dialog-error]');if(slot){slot.hidden=false;slot.textContent=error.message;}else this.showError(error);this.status.textContent='';}}
      finally{if(this.current(turn)){this.busy=false;for(const [node,disabled]of controls)if(node.isConnected)node.disabled=disabled;if(this.refreshPending)this.scheduleRefresh();}}
    }
    schedulePreview(id,anchor){this.win.clearTimeout(this.previewShow);this.win.clearTimeout(this.previewHide);this.previewShow=this.win.setTimeout(()=>void this.showPreview(id,anchor),350);}
    hidePreview(){this.previewTurn++;this.win.clearTimeout(this.previewShow);this.win.clearTimeout(this.previewHide);this.preview?.remove();this.preview=null;}
    async showPreview(id,anchor){
      this.hidePreview();const turn=this.previewTurn,generation=this.generation,issue=this.issues.find(row=>row.id===id);if(!issue)return;
      const card=this.doc.createElement('aside');card.className='tc-graph__preview';card.setAttribute('aria-label',`Preview ${issue.identifier}`);card.innerHTML=`<h2>${escapeHtml(issue.identifier)}</h2><p>${escapeHtml(issue.title)}</p><p>${escapeHtml(issue.status)} · ${escapeHtml(issue.priority||'none')}</p><p data-preview-description>Loading preview…</p><a href="${href(this.identifier,issue)}">Open issue</a>`;
      this.root.append(card);this.preview=card;const rect=anchor.getBoundingClientRect();card.style.left=`${Math.max(8,Math.min(rect.left,this.win.innerWidth-330))}px`;card.style.top=`${Math.max(8,Math.min(rect.bottom+6,this.win.innerHeight-230))}px`;
      card.addEventListener('pointerenter',()=>this.win.clearTimeout(this.previewHide));card.addEventListener('pointerleave',()=>this.hidePreview());card.addEventListener('keydown',event=>{if(event.key==='Escape'){event.preventDefault();this.hidePreview();anchor.focus();}});
      try{const data=await this.request(`/issues/resolve/${encodeURIComponent(issue.identifier)}`);if(!this.current(generation)||turn!==this.previewTurn)return;card.querySelector('[data-preview-description]').textContent=data.description?.slice(0,1500)||'No description.';}
      catch(error){if(this.current(generation)&&turn===this.previewTurn)card.querySelector('[data-preview-description]').textContent=error.message;}
    }
    dispose(){this.disposed=true;this.generation++;for(const aborter of this.aborters)aborter.abort();this.aborters.clear();this.win.clearTimeout(this.refreshTimer);this.win.clearInterval(this.baseline);this.closeDialog();this.hidePreview();for(const remove of this.listeners)remove();this.listeners=[];}
  }
  const api={Controller,identity,editable};globalThis.LificTopcoatAnalytics=api;if(typeof module!=='undefined')module.exports=api;
  if(typeof document!=='undefined')document.querySelectorAll('[data-topcoat-analytics]').forEach(root=>{root._analytics=new Controller(root);});
})();

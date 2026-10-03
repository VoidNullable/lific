(() => {
  'use strict';
  const escapeHtml=value=>String(value??'').replace(/[&<>"']/g,char=>({'&':'&amp;','<':'&lt;','>':'&gt;','"':'&quot;',"'":'&#39;'}[char]));
  const editable=role=>!!role&&(role.is_admin||!role.enforced||['lead','maintainer'].includes(role.role));
  const metadataEditable=(role,project,user)=>!!role&&(role.is_admin||role.role==='lead'||role.enforced&&role.role==='maintainer'||!role.enforced&&project?.lead_user_id!=null&&Number(project.lead_user_id)===Number(user?.id));
  const STATUSES=['active','planned','paused','backlog','done','cancelled'];
  const TAB_STATUSES={active:['active','planned','paused'],backlog:['backlog'],archive:['done','cancelled']};
  function visibleModules(rows,tab){return tab==='all'?rows:rows.filter(row=>(TAB_STATUSES[tab]||[]).includes(row.status));}
  function moduleProgress(rows,id){const mine=rows.filter(row=>Number(row.module_id)===Number(id)),done=mine.filter(row=>row.status==='done').length;return {done,total:mine.length,fraction:mine.length?done/mine.length:0};}
  function issueState(id,blocked,workable){
    const blockers=blocked.get(id)?.blocked_by;
    return [blocked.has(id)?blockers?.length?`Blocked by ${blockers.join(', ')}`:'Blocked':'',workable.has(id)?'Workable':''].filter(Boolean).join(' · ');
  }
  const readTab=project=>{try{return localStorage.getItem(`lific:subtab:modules:${project}`);}catch{return null;}};
  const saveTab=(project,tab)=>{try{localStorage.setItem(`lific:subtab:modules:${project}`,tab);}catch{}};

  class Controller {
    constructor(root,{session=globalThis.lificSession,sync=globalThis.lificSync,navigate=path=>location.assign(path)}={}){
      this.root=root;this.session=session;this.sync=sync;this.navigate=navigate;this.mode=root.dataset.topcoatModules;
      this.identifier=root.dataset.projectIdentifier;this.id=Number(root.dataset.moduleId);
      this.content=root.querySelector('[data-modules-content]');this.status=root.querySelector('[data-modules-status]');this.error=root.querySelector('[data-modules-error]');
      this.project=null;this.module=null;this.modules=[];this.issues=[];this.role=null;this.busy=false;this.disposed=false;this.generation=0;this.editRevision=0;this.picker=null;this.listeners=[];this.timer=null;
      this.blocked=new Map();this.workable=new Set();this.tab='active';this.query='';this.scope=this.identity();
      const listen=(node,event,callback)=>{node.addEventListener(event,callback);this.listeners.push(()=>node.removeEventListener(event,callback));};
      for(const event of ['input','change'])listen(root,event,()=>{this.editRevision++;});
      const audience=()=>{if(!this.session?.state?.loading){const next=this.identity();if(next!==this.scope){this.scope=next;void this.load();}else this.scheduleRefresh();}};
      for(const event of ['lific:account-change','lific:session-change','lific:scope-change'])listen(globalThis,event,audience);
      listen(globalThis,'lific:realtime',event=>{const detail=event.detail||{};if(detail.type==='resync.required'||Number(detail.project_id)===Number(this.project?.id))this.scheduleRefresh();});
      for(const event of ['focus','online'])listen(globalThis,event,()=>this.scheduleRefresh());
      listen(document,'visibilitychange',()=>{if(!document.hidden)this.scheduleRefresh();});
      listen(globalThis,'pagehide',event=>{if(!event.persisted)this.dispose();});
      listen(globalThis,'pageshow',event=>{if(event.persisted)audience();});
      this.unsubscribe=this.sync?.subscribe?.(()=>{
        if(this.mode!=='list'||!this.project||this.disposed||this.scope!==this.identity())return;
        const model=this.sync.peekProject(this.project.id);if(model?.status==='ready'){this.issues=model.issues;if(!this.localWork())this.renderList();}
      });
      if(!session?.state?.loading)void this.load();
    }
    identity(){let token='';try{token=localStorage.getItem('lific_token')||'';}catch{}return `${this.session?.state?.user?.id??''}:${this.session?.state?.publicProject??''}:${token}`;}
    current(turn){return !this.disposed&&turn===this.generation&&this.scope===this.identity();}
    async request(path,options){const result=await this.session.request(path,options);if(!result?.ok){const error=new Error(result?.error||'Could not load modules.');error.status=result?.status;throw error;}return result.data;}
    metadataEditable(){return metadataEditable(this.role,this.project,this.session?.state?.user);}
    async issueRows(project){
      const path=`/issues?${new URLSearchParams({project_id:String(project.id),module_id:String(this.id),limit:'500'})}`;
      const turn=this.generation;
      const read=async filter=>{
        const rows=[];
        for(let offset=0;;offset+=500){
          const page=await this.request(`${path}${filter}${offset?`&offset=${offset}`:''}`);
          if(!this.current(turn))return [];
          rows.push(...page);
          if(page.length<500)return rows;
        }
      };
      const [issues,blocked,workable]=await Promise.all([read(''),read('&blocked=true'),read('&workable=true')]);
      return {issues,blocked:new Map(blocked.map(issue=>[issue.id,issue])),workable:new Set(workable.map(issue=>issue.id))};
    }
    localWork(){return this.busy||this.picker||this.root.querySelector('dialog[open]')||this.content.querySelector('[data-module-description-form]')||[...this.content.querySelectorAll('[data-module-create] input')].some(input=>input.value.trim())||this.content.contains(document.activeElement)&&document.activeElement.matches('input,textarea,select');}
    scheduleRefresh(){clearTimeout(this.timer);this.timer=setTimeout(()=>{this.timer=null;if(this.root.getAttribute('aria-busy')==='true'||this.localWork())this.scheduleRefresh();else void this.load({refresh:true});},250);}
    showError(reason){this.error.hidden=false;this.error.replaceChildren(document.createTextNode(reason?.message||'Could not load modules.'));}
    async load({refresh=false}={}){
      if(refresh&&this.localWork()){this.scheduleRefresh();return;}
      clearTimeout(this.timer);this.timer=null;
      const revision=this.editRevision;
      const turn=++this.generation;this.busy=false;this.picker?.dispose();this.picker=null;
      if(!refresh){this.role=null;this.content.replaceChildren();this.project=null;this.module=null;this.issues=[];this.blocked=new Map();this.workable=new Set();}
      this.status.textContent='Loading modules…';this.error.hidden=true;this.root.setAttribute('aria-busy','true');
      try {
        if(this.session?.state?.publicProject||location.pathname.startsWith('/public/'))throw new Error('Modules are not available in public projects.');
        if(this.mode==='detail'&&(!Number.isSafeInteger(this.id)||this.id<=0))throw new Error('Invalid module ID.');
        const projects=await this.request('/projects');if(!this.current(turn))return;
        const project=projects.find(item=>item.identifier.toLowerCase()===this.identifier.toLowerCase());
        if(!project)throw new Error(`Project ${this.identifier} not found.`);
        const role=await this.request(`/projects/${project.id}/my-role`);if(!this.current(turn))return;
        let modules=[],module=null,issues=[],blocked=new Map(),workable=new Set();
        if(this.mode==='list'){
          modules=await this.request(`/modules?project_id=${project.id}`);if(!this.current(turn))return;
          this.sync?.setActiveProject?.(project.id);
          if(!this.sync?.ensureProject)throw new Error('Issue counts are unavailable. Try again.');
          const model=await this.sync.ensureProject(project.id);if(!this.current(turn))return;
          if(!model||model.status!=='ready')throw new Error(model?.error||'Could not load issue counts.');
          issues=model.issues;
        }else {
          module=await this.request(`/modules/${this.id}`);if(!this.current(turn))return;
          if(Number(module.project_id)!==Number(project.id))throw new Error('This module does not belong to the selected project.');
          ({issues,blocked,workable}=await this.issueRows(project));if(!this.current(turn))return;
          this.sync?.setActiveProject?.(project.id);
        }
        if(refresh&&(revision!==this.editRevision||this.localWork())){
          this.root.setAttribute('aria-busy','false');if(this.status.textContent==='Loading modules…')this.status.textContent='';this.scheduleRefresh();return;
        }
        this.project=project;this.role=role;this.module=module;this.modules=modules;this.issues=issues;this.blocked=blocked;this.workable=workable;
        const saved=readTab(this.identifier);this.tab=['active','backlog','archive','all'].includes(saved)?saved:'active';
        this.render();this.status.textContent='';this.root.setAttribute('aria-busy','false');
      }catch(reason){if(this.current(turn)){this.root.setAttribute('aria-busy','false');this.status.textContent='';this.showError(reason);
        const retry=document.createElement('button');retry.type='button';retry.textContent='Retry';retry.addEventListener('click',()=>void this.load({refresh}),{once:true});this.error.append(' ',retry);}}
    }
    async mutate(path,method,body,{remove=false,assignment=false}={}){
      if(this.busy||!(assignment?editable(this.role):this.metadataEditable())||this.disposed)return false;
      this.editRevision++;
      const turn=this.generation;this.busy=true;this.error.hidden=true;this.status.textContent='Saving…';
      const controls=[...this.content.querySelectorAll('input,textarea,select,button')].map(node=>[node,node.disabled]);for(const [node] of controls)node.disabled=true;
      try {
        const result=await this.request(path,{method,...body===undefined?{}:{body:JSON.stringify(body)}});if(!this.current(turn))return false;
        if(remove){this.navigate(`/${encodeURIComponent(this.identifier)}/modules`);return true;}
        if(this.mode==='list'){this.navigate(`/${encodeURIComponent(this.identifier)}/modules/${result.id}`);return true;}
        if(assignment){const rows=await this.issueRows(this.project);if(!this.current(turn))return false;Object.assign(this,rows);}
        else this.module=result;
        this.renderDetail({preserveDescription:!Object.hasOwn(body||{},'description')});this.status.textContent='Saved.';return true;
      }catch(reason){if(this.current(turn)){this.showError(reason);this.status.textContent='';for(const field of ['name','status','emoji']){const node=this.content.querySelector(`[data-module-${field}]`);if(node)node.value=this.module?.[field]||'';}}return false;}
      finally{if(this.current(turn)){this.busy=false;for(const [node,disabled] of controls)if(node.isConnected)node.disabled=disabled;}}
    }
    render(){if(this.mode==='list')this.renderList();else this.renderDetail();}
    renderList(){
      const edit=this.metadataEditable(),rows=visibleModules(this.modules,this.tab);
      const assigned=this.issues.filter(issue=>issue.module_id!=null),done=assigned.filter(issue=>issue.status==='done').length;
      this.content.innerHTML=`<header><h1>Modules</h1></header><nav aria-label="Module views">${['active','backlog','archive','all'].map(tab=>`<button type="button" data-module-tab="${tab}" aria-current="${tab===this.tab?'page':'false'}">${tab[0].toUpperCase()+tab.slice(1)} (${visibleModules(this.modules,tab).length})</button>`).join('')}</nav>
        <section aria-label="Module portfolio"><span>${this.modules.filter(module=>module.status==='active').length} active modules · ${this.modules.length} modules</span>
          <progress max="${assigned.length||1}" value="${done}"></progress><span>${done}/${assigned.length} assigned issues done</span></section>
        ${edit?'<form data-module-create><label>Module name<input name="name" required maxlength="200"></label><label>Icon or emoji<input name="emoji" placeholder="Emoji or lucide:Layers"></label><button type="submit">Create module</button></form>':''}
        ${rows.length?STATUSES.concat(['other']).map(status=>{const group=rows.filter(row=>status==='other'?!STATUSES.includes(row.status):row.status===status).sort((a,b)=>a.name.localeCompare(b.name));
          return group.length?`<section><h2>${escapeHtml(status)}</h2><ul>${group.map(module=>{const counts=moduleProgress(this.issues,module.id);return `<li><a href="/${encodeURIComponent(this.identifier)}/modules/${module.id}">${escapeHtml(module.emoji||'')} ${escapeHtml(module.name)}</a>
            <p>${escapeHtml(module.description?.split('\n').find(line=>line.trim()&&!line.startsWith('#'))||'')}</p><progress max="${counts.total||1}" value="${counts.done}"></progress><span>${counts.done}/${counts.total} issues done</span></li>`;}).join('')}</ul></section>`:'';}).join(''):
          `<p>${this.modules.length?'No modules in this view.':'No modules yet.'}</p>`}`;
      this.content.querySelectorAll('[data-module-tab]').forEach(button=>button.addEventListener('click',()=>{this.tab=button.dataset.moduleTab;saveTab(this.identifier,this.tab);this.renderList();}));
      this.content.querySelector('[data-module-create]')?.addEventListener('submit',event=>{event.preventDefault();const data=new FormData(event.currentTarget),name=data.get('name').trim(),emoji=data.get('emoji').trim();if(name)void this.mutate('/modules','POST',{project_id:this.project.id,name,status:'active',...emoji?{emoji}:{}});});
    }
    renderDetail({preserveDescription=true}={}){
      const descriptionForm=preserveDescription?this.content.querySelector('[data-module-description-form]'):null;
      const focused=descriptionForm?.contains(document.activeElement)?document.activeElement:null;
      descriptionForm?.remove();
      const module=this.module,edit=this.metadataEditable(),editIssues=editable(this.role),counts=moduleProgress(this.issues,this.id);
      this.content.innerHTML=`<nav aria-label="Breadcrumb"><a href="/${encodeURIComponent(this.identifier)}/modules">Modules</a> / ${escapeHtml(module.name)}</nav>
        <header class="tc-module-heading">${edit?`<label>Module name<input data-module-name value="${escapeHtml(module.name)}"></label>`:`<h1>${escapeHtml(module.name)}</h1>`}
          <label>Status<select aria-label="Module status" data-module-status ${edit?'':'disabled'}>${STATUSES.map(status=>`<option ${module.status===status?'selected':''}>${status}</option>`).join('')}</select></label>
          ${edit?`<label>Icon or emoji<input data-module-emoji value="${escapeHtml(module.emoji||'')}" placeholder="Emoji or lucide:Layers"></label><button type="button" data-module-delete>Delete module</button>`:`<span>${escapeHtml(module.emoji||'')}</span>`}</header>
        <section aria-label="Module progress"><progress max="${counts.total||1}" value="${counts.done}"></progress><span data-module-progress>${counts.done}/${counts.total} issues done</span></section>
        <section><h2>Description</h2><article data-module-description></article>${edit?'<button type="button" data-module-edit-description>Edit description</button>':''}</section>
        <section><header class="tc-module-heading"><h2>Issues (${this.issues.length})</h2>${editIssues?`<a href="/${encodeURIComponent(this.identifier)}/issues/new?module=${this.id}">New issue in module</a><button type="button" data-module-assign>Assign issue</button>`:''}</header>
          <label>Search module issues<input type="search" data-module-search value="${escapeHtml(this.query)}"></label><div data-module-issue-list></div></section>
        <p>Created ${escapeHtml(module.created_at)} · Updated ${escapeHtml(module.updated_at)}</p>`;
      const description=this.content.querySelector('[data-module-description]');
      if(globalThis.lificIssueEditor?.renderMarkdown)globalThis.lificIssueEditor.renderMarkdown(description,module.description||'');else description.textContent=module.description||'';
      const bind=(selector,field)=>{const node=this.content.querySelector(selector);node?.addEventListener(field==='status'?'change':'blur',()=>{
        const value=field==='emoji'?node.value.trim()||null:node.value.trim();if(field==='name'&&!value){node.value=module.name;return;}
        if(value!==(module[field]??null))void this.mutate(`/modules/${this.id}`,'PUT',{[field]:value});});
        if(field==='name')node?.addEventListener('keydown',event=>{if(event.key==='Enter'){event.preventDefault();node.blur();}else if(event.key==='Escape'){node.value=module.name;node.blur();}});};
      bind('[data-module-name]','name');bind('[data-module-status]','status');bind('[data-module-emoji]','emoji');
      this.content.querySelector('[data-module-delete]')?.addEventListener('click',()=>{if(confirm('Delete this module? Its issues will remain in the project.'))void this.mutate(`/modules/${this.id}`,'DELETE',undefined,{remove:true});});
      this.content.querySelector('[data-module-edit-description]')?.addEventListener('click',()=>this.editDescription());
      this.content.querySelector('[data-module-assign]')?.addEventListener('click',()=>this.assignIssue());
      this.content.querySelector('[data-module-search]').addEventListener('input',event=>{this.query=event.currentTarget.value;this.renderIssues();});
      this.renderIssues();
      if(descriptionForm){description.hidden=true;this.content.querySelector('[data-module-edit-description]').hidden=true;description.after(descriptionForm);focused?.focus();}
    }
    renderIssues(){
      const list=this.content.querySelector('[data-module-issue-list]'),query=this.query.toLowerCase().trim(),edit=editable(this.role);
      const rows=this.issues.filter(issue=>`${issue.identifier} ${issue.title}`.toLowerCase().includes(query));
      list.innerHTML=rows.length?`<ul>${rows.map(issue=>`<li><a href="/${encodeURIComponent(this.identifier)}/issues/${encodeURIComponent(issue.identifier)}">${escapeHtml(issue.identifier)} · ${escapeHtml(issue.title)}</a>
        <span>${escapeHtml(issue.status)} · ${escapeHtml(issue.priority||'none')}</span><small>${escapeHtml(issueState(issue.id,this.blocked,this.workable))}</small>
        ${edit?`<button type="button" data-module-detach="${issue.id}">Remove from module</button>`:''}</li>`).join('')}</ul>`:`<p>${this.issues.length?'No matching issues.':'No issues in this module.'}</p>`;
      list.querySelectorAll('[data-module-detach]').forEach(button=>button.addEventListener('click',()=>void this.mutate(`/issues/${button.dataset.moduleDetach}`,'PUT',{module_id:null},{assignment:true})));
    }
    editDescription(){
      const article=this.content.querySelector('[data-module-description]'),button=this.content.querySelector('[data-module-edit-description]');article.hidden=true;button.hidden=true;
      const form=document.createElement('form');form.dataset.moduleDescriptionForm='';form.innerHTML=`<label>Description<textarea aria-label="Description" name="description">${escapeHtml(this.module.description)}</textarea></label><button type="submit">Save description</button><button type="button" data-description-cancel>Cancel</button>`;
      article.after(form);form.querySelector('textarea').focus();
      form.addEventListener('submit',event=>{event.preventDefault();void this.mutate(`/modules/${this.id}`,'PUT',{description:new FormData(form).get('description')});});
      form.querySelector('[data-description-cancel]').addEventListener('click',()=>{form.remove();this.content.querySelector('[data-module-description]').hidden=false;const currentButton=this.content.querySelector('[data-module-edit-description]');currentButton.hidden=false;currentButton.focus();});
      form.querySelector('textarea').addEventListener('keydown',event=>{if((event.ctrlKey||event.metaKey)&&event.key.toLowerCase()==='s'){event.preventDefault();form.requestSubmit();}else if(event.key==='Escape'){event.preventDefault();form.querySelector('[data-description-cancel]').click();}});
    }
    assignIssue(){this.picker?.dispose();this.picker=globalThis.LificTopcoatIssuePicker.mount(this.root,{request:path=>this.request(path),project:this.project,projectOnly:true,title:'Assign an issue to this module',
      onClose:()=>{this.picker=null;},onSelect:async issue=>{this.picker=null;await this.mutate(`/issues/${issue.id}`,'PUT',{module_id:this.id},{assignment:true});}});}
    dispose(){this.disposed=true;this.generation++;clearTimeout(this.timer);this.picker?.dispose();this.unsubscribe?.();for(const remove of this.listeners)remove();this.listeners=[];}
  }
  const api={Controller,moduleProgress,visibleModules,issueState,metadataEditable,editable};globalThis.LificTopcoatModules=api;if(typeof module!=='undefined')module.exports=api;
  if(typeof document!=='undefined')document.querySelectorAll('[data-topcoat-modules]').forEach(root=>{root._modules=new Controller(root);});
})();

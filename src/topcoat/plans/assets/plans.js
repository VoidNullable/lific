(() => {
  'use strict';
  const escapeHtml=value=>String(value??'').replace(/[&<>"']/g,char=>({'&':'&amp;','<':'&lt;','>':'&gt;','"':'&quot;',"'":'&#39;'}[char]));
  const editable=role=>!!role&&(role.is_admin||!role.enforced||['lead','maintainer'].includes(role.role));
  const STATUSES=['active','done','archived'];
  function flattenSteps(steps,depth=0){return (steps||[]).flatMap(step=>[{step,depth},...flattenSteps(step.children,depth+1)]);}
  const movePatch=(parent,position)=>({...parent==null?{move_to_root:true}:{move_parent_step_id:Number(parent)},move_position:Number(position)});
  const progress=plan=>({done:Number(plan.done_count||0),total:Number(plan.step_count||0),fraction:plan.step_count>0?plan.done_count/plan.step_count:0});
  function provenance(step){
    if(!step.issue_identifier)return '';
    if(step.done&&step.issue_status==='done')return `via ${step.issue_identifier}`;
    if(!step.done&&step.reopened_via_issue_at)return `${step.issue_identifier} reopened`;
    return `${step.issue_identifier}: ${step.issue_status||'unknown'}`;
  }
  const readTab=(project)=>{try{return localStorage.getItem(`lific:subtab:plans:${project}`);}catch{return null;}};
  const saveTab=(project,tab)=>{try{localStorage.setItem(`lific:subtab:plans:${project}`,tab);}catch{}};

  class Controller {
    constructor(root,{session=globalThis.lificSession,sync=globalThis.lificSync,navigate=path=>location.assign(path)}={}){
      this.root=root;this.session=session;this.sync=sync;this.navigate=navigate;
      this.mode=root.dataset.topcoatPlans;this.identifier=root.dataset.projectIdentifier;this.id=Number(root.dataset.planId);
      this.content=root.querySelector('[data-plans-content]');this.status=root.querySelector('[data-plans-status]');this.error=root.querySelector('[data-plans-error]');
      this.project=null;this.plan=null;this.plans=[];this.activity=[];this.role=null;this.busy=false;this.disposed=false;this.generation=0;this.editRevision=0;
      this.collapsed=new Set();this.picker=null;this.peek=null;this.tab='active';this.timer=null;this.listeners=[];this.scope=this.identity();
      const listen=(node,event,callback)=>{node.addEventListener(event,callback);this.listeners.push(()=>node.removeEventListener(event,callback));};
      for(const event of ['input','change'])listen(root,event,()=>{this.editRevision++;});
      this.onAudience=()=>{if(!this.session?.state?.loading){const next=this.identity();if(next!==this.scope){this.scope=next;void this.load();}else this.scheduleRefresh();}};
      for(const event of ['lific:account-change','lific:session-change','lific:scope-change'])listen(globalThis,event,this.onAudience);
      listen(globalThis,'lific:realtime',event=>{const detail=event.detail||{};if(detail.type==='resync.required'||Number(detail.project_id)===Number(this.project?.id)||detail.issue_id!=null&&[this.plan?.issue_id,...flattenSteps(this.plan?.steps).map(({step})=>step.issue_id)].some(id=>id!=null&&Number(id)===Number(detail.issue_id)))this.scheduleRefresh();});
      for(const event of ['focus','online'])listen(globalThis,event,()=>this.scheduleRefresh());
      listen(document,'visibilitychange',()=>{if(!document.hidden)this.scheduleRefresh();});
      listen(globalThis,'pagehide',event=>{if(!event.persisted)this.dispose();});
      listen(globalThis,'pageshow',event=>{if(event.persisted)this.onAudience();});
      if(!session?.state?.loading)void this.load();
    }
    identity(){let token='';try{token=localStorage.getItem('lific_token')||'';}catch{}return `${this.session?.state?.user?.id??''}:${this.session?.state?.publicProject??''}:${token}`;}
    current(turn){return !this.disposed&&turn===this.generation&&this.scope===this.identity();}
    async request(path,options){const result=await this.session.request(path,options);if(!result?.ok){const error=new Error(result?.error||'Could not load plans.');error.status=result?.status;throw error;}return result.data;}
    showError(reason){this.error.hidden=false;this.error.replaceChildren(document.createTextNode(reason?.message||'Could not load plans.'));}
    localWork(){return this.busy||this.picker||this.root.querySelector('dialog[open]')||this.content.querySelector('[data-plan-create] input')?.value.trim()||this.content.contains(document.activeElement)&&document.activeElement.matches('input,textarea,select');}
    scheduleRefresh(){clearTimeout(this.timer);this.timer=setTimeout(()=>{this.timer=null;if(this.root.getAttribute('aria-busy')==='true'||this.localWork())this.scheduleRefresh();else void this.load({refresh:true});},250);}
    async load({refresh=false}={}){
      if(refresh&&this.localWork()){this.scheduleRefresh();return;}
      clearTimeout(this.timer);this.timer=null;
      const revision=this.editRevision;
      const turn=++this.generation;this.picker?.dispose();this.picker=null;
      this.peek?.dispose();this.peek=null;
      this.busy=false;this.root.querySelectorAll('.tc-plan-dialog').forEach(node=>{node.close();node.remove();});
      if(!refresh){this.role=null;this.content.replaceChildren();this.plan=null;this.project=null;}
      this.error.hidden=true;this.status.textContent='Loading plans…';this.root.setAttribute('aria-busy','true');
      try {
        if(this.session?.state?.publicProject||location.pathname.startsWith('/public/'))throw new Error('Plans are not available in public projects.');
        if(this.mode==='detail'&&(!Number.isSafeInteger(this.id)||this.id<=0))throw new Error('Invalid plan ID.');
        const projects=await this.request('/projects');if(!this.current(turn))return;
        const project=projects.find(item=>item.identifier.toLowerCase()===this.identifier.toLowerCase());
        if(!project)throw new Error(`Project ${this.identifier} not found.`);
        const role=await this.request(`/projects/${project.id}/my-role`);if(!this.current(turn))return;
        let plan=null,plans=[],activity=[];
        if(this.mode==='detail'){
          plan=await this.request(`/plans/${this.id}`);if(!this.current(turn))return;
          if(Number(plan.project_id)!==Number(project.id))throw new Error('This plan does not belong to the selected project.');
          const feed=await this.request(`/plans/${this.id}/activity?limit=100`);if(!this.current(turn))return;
          activity=feed.items||[];
        } else {
          const seen=new Set();let before=null;
          while(true){
            const params=new URLSearchParams({project_id:String(project.id),limit:'200',order_by:'id'});if(before!==null)params.set('before_id',String(before));
            const page=await this.request(`/plans?${params}`);if(!this.current(turn))return;
            for(const item of page)if(!seen.has(item.id)){seen.add(item.id);plans.push(item);}
            if(page.length<200)break;
            const last=Number(page.at(-1).id);if(before!==null&&last>=before)throw new Error('Could not continue loading plans.');before=last;
          }
          plans.sort((a,b)=>String(b.updated_at).localeCompare(String(a.updated_at))||b.id-a.id);
        }
        if(!this.current(turn))return;
        if(refresh&&(revision!==this.editRevision||this.localWork())){
          this.root.setAttribute('aria-busy','false');if(this.status.textContent==='Loading plans…')this.status.textContent='';this.scheduleRefresh();return;
        }
        this.project=project;this.role=role;this.plan=plan;this.plans=plans;this.activity=activity;this.sync?.setActiveProject?.(project.id);
        if(this.mode==='list'){
          const tab=readTab(project.id);this.tab=['active','done','archived','all'].includes(tab)?tab:plans.length&&!plans.some(item=>item.status==='active')?'all':'active';
        } else this.recordRecent();
        this.render();this.status.textContent='';this.root.setAttribute('aria-busy','false');
      }catch(reason){if(this.current(turn)){this.root.setAttribute('aria-busy','false');this.status.textContent='';this.showError(reason);
        const retry=document.createElement('button');retry.type='button';retry.textContent='Retry';retry.addEventListener('click',()=>void this.load({refresh}),{once:true});this.error.append(' ',retry);}}
    }
    recordRecent(){try{const rows=JSON.parse(localStorage.getItem('lific_recents')||'[]');localStorage.setItem('lific_recents',JSON.stringify([
      {type:'plan',routeId:String(this.plan.id),identifier:this.plan.identifier,title:this.plan.title,project:this.identifier,ts:Date.now()},
      ...rows.filter(row=>!(row.type==='plan'&&row.routeId===String(this.plan.id)))].slice(0,15)));}catch{}}
    async mutate(path,method,body,{step=false,remove=false}={}){
      if(this.busy||!editable(this.role)||this.disposed)return false;
      this.editRevision++;
      const turn=this.generation;this.busy=true;this.error.hidden=true;this.status.textContent='Saving…';
      const controls=[...this.content.querySelectorAll('input,textarea,select,button')].map(node=>[node,node.disabled]);
      for(const [node] of controls)node.disabled=true;
      try {
        const result=await this.request(path,{method,...body===undefined?{}:{body:JSON.stringify(body)}});if(!this.current(turn))return false;
        if(remove){this.navigate(`/${encodeURIComponent(this.identifier)}/plans`);return true;}
        if(this.mode==='list'){this.navigate(`/${encodeURIComponent(this.identifier)}/plans/${result.id}`);return true;}
        this.plan=step?result.plan:result;
        if(step&&result.effect?.issue_status_changed)this.status.textContent=`${result.effect.issue_identifier} marked ${result.effect.issue_new_status||'done'}.`;
        else this.status.textContent='Saved.';
        try{const feed=await this.request(`/plans/${this.id}/activity?limit=100`);if(this.current(turn))this.activity=feed.items||[];}catch{if(this.current(turn))this.showError(new Error('Saved, but activity could not be refreshed.'));}
        if(!this.current(turn))return false;
        this.render();return true;
      }catch(reason){if(this.current(turn)){this.render();this.showError(reason);this.status.textContent='';}return false;}
      finally{if(this.current(turn)){this.busy=false;for(const [node,disabled] of controls)if(node.isConnected)node.disabled=disabled;}}
    }
    render(){if(this.mode==='list')this.renderList();else this.renderDetail();}
    renderList(){
      const edit=editable(this.role),rows=this.plans.filter(plan=>this.tab==='all'||plan.status===this.tab);
      this.content.innerHTML=`<header class="tc-plans-heading"><h1>Plans</h1></header><nav aria-label="Plan views">${['active','done','archived','all'].map(tab=>
        `<button type="button" data-plan-tab="${tab}" aria-current="${this.tab===tab?'page':'false'}">${tab[0].toUpperCase()+tab.slice(1)} (${this.plans.filter(plan=>tab==='all'||plan.status===tab).length})</button>`).join('')}</nav>
        ${edit&&['active','all'].includes(this.tab)?'<form data-plan-create><label>Plan title<input name="title" required maxlength="200"></label><button type="submit" data-edit-control>Create plan</button></form>':''}
        ${rows.length?STATUSES.concat(['other']).map(status=>{const group=rows.filter(plan=>status==='other'?!STATUSES.includes(plan.status):plan.status===status);return group.length?
          `<section><h2>${escapeHtml(status)}</h2><ul>${group.map(plan=>{const counts=progress(plan);return `<li><a href="/${encodeURIComponent(this.identifier)}/plans/${plan.id}">${escapeHtml(plan.identifier)} · ${escapeHtml(plan.title)}</a>
            <progress max="${counts.total||1}" value="${counts.done}"></progress><span>${counts.done}/${counts.total} steps</span></li>`;}).join('')}</ul></section>`:'';}).join(''):
          `<p>${this.plans.length?'No plans in this view.':'No plans yet.'}</p>`}`;
      this.content.querySelectorAll('[data-plan-tab]').forEach(button=>button.addEventListener('click',()=>{this.tab=button.dataset.planTab;saveTab(this.project.id,this.tab);this.renderList();}));
      this.content.querySelector('[data-plan-create]')?.addEventListener('submit',event=>{event.preventDefault();const title=new FormData(event.currentTarget).get('title').trim();if(title)void this.mutate('/plans','POST',{project_id:this.project.id,title});});
    }
    stepRows(steps){
      const edit=editable(this.role);
      return `<ol class="tc-plan-tree">${(steps||[]).map((step,index)=>{
        const expanded=!this.collapsed.has(step.id);
        return `<li data-plan-step="${step.id}" id="step-${step.id}"><div class="tc-plan-step-row">
          <button type="button" data-step-collapse="${step.id}" aria-expanded="${expanded}" aria-label="${expanded?'Collapse':'Expand'} ${escapeHtml(step.title)}">${expanded?'▾':'▸'}</button>
          <label><input type="checkbox" data-step-done="${step.id}" data-edit-control ${step.done?'checked':''} ${edit?'':'disabled'} aria-label="Complete ${escapeHtml(step.title)}"></label>
          ${edit?`<input data-step-title="${step.id}" data-edit-control aria-label="Step title ${step.id}" value="${escapeHtml(step.title)}">`:`<span>${escapeHtml(step.title)}</span>`}
          ${step.issue_identifier?`<a href="/${encodeURIComponent(step.issue_identifier.split('-')[0])}/issues/${encodeURIComponent(step.issue_identifier)}">${escapeHtml(provenance(step))}</a>`:''}
          ${edit?`<div class="tc-plan-step-actions"><button type="button" data-step-child="${step.id}" data-edit-control>Add child</button>
            <button type="button" data-step-link="${step.id}" data-edit-control>Link issue</button>${step.issue_id!=null?`<button type="button" data-step-detach="${step.id}" data-edit-control>Detach issue</button>`:''}
            <button type="button" data-step-move="${step.id}" data-edit-control>Move step</button>
            <button type="button" data-step-order="${step.id}" data-delta="-1" data-edit-control ${index===0?'disabled':''}>Move up</button>
            <button type="button" data-step-order="${step.id}" data-delta="1" data-edit-control ${index===steps.length-1?'disabled':''}>Move down</button>
            <button type="button" data-step-delete="${step.id}" data-edit-control>Delete step</button></div>`:''}</div>
          <div data-step-body="${step.id}" ${expanded?'':'hidden'}><article data-step-description="${step.id}"></article>
            ${edit?`<button type="button" data-step-edit-description="${step.id}" data-edit-control>Edit description</button>`:''}
            ${this.stepRows(step.children)}</div></li>`;
      }).join('')}</ol>`;
    }
    renderDetail(){
      const plan=this.plan,edit=editable(this.role),counts=progress(plan);
      this.content.innerHTML=`<nav aria-label="Breadcrumb"><a href="/${encodeURIComponent(this.identifier)}/plans">Plans</a> / ${escapeHtml(plan.identifier)}</nav>
        <header class="tc-plans-heading">${edit?`<label>Plan title<input data-plan-title data-edit-control value="${escapeHtml(plan.title)}"></label>`:`<h1>${escapeHtml(plan.title)}</h1>`}
          <label>Status<select aria-label="Plan status" data-plan-status data-edit-control ${edit?'':'disabled'}>${STATUSES.map(status=>`<option ${plan.status===status?'selected':''}>${status}</option>`).join('')}</select></label>
          ${edit?'<button type="button" data-plan-delete data-edit-control>Delete plan</button>':''}</header>
        <section aria-label="Plan progress"><progress max="${counts.total||1}" value="${counts.done}"></progress><span data-plan-progress>${counts.done}/${counts.total} steps</span></section>
        <section class="tc-plan-anchor"><h2>Anchor issue</h2>${plan.anchor_identifier?`<a href="/${encodeURIComponent(plan.anchor_identifier.split('-')[0])}/issues/${encodeURIComponent(plan.anchor_identifier)}">${escapeHtml(plan.anchor_identifier)}</a>`:'<p>No anchor issue.</p>'}
          ${edit?'<button type="button" data-plan-anchor data-edit-control>Set anchor issue</button>':''}</section>
        <section><h2>Steps</h2>${edit?'<button type="button" data-plan-add-root data-edit-control>Add step</button>':''}
          ${plan.steps?.length?this.stepRows(plan.steps):'<p>No steps yet.</p>'}</section>
        <section><h2>Activity</h2><ol data-plan-activity>${this.activity.map(item=>`<li>${escapeHtml(item.actor_display_name||item.actor_username||'System')} ${escapeHtml(item.action||'')} ${escapeHtml(item.field||'')} ${escapeHtml(item.old_value||'')} → ${escapeHtml(item.new_value||'')} <time>${escapeHtml(item.ts||'')}</time></li>`).join('')||'<li>No activity yet.</li>'}</ol></section>
        <p>Created ${escapeHtml(plan.created_at)} · Updated ${escapeHtml(plan.updated_at)}</p>`;
      const all=flattenSteps(plan.steps),find=id=>all.find(row=>row.step.id===Number(id))?.step;
      for(const {step} of all){const node=this.content.querySelector(`[data-step-description="${step.id}"]`);if(globalThis.lificIssueEditor?.renderMarkdown)globalThis.lificIssueEditor.renderMarkdown(node,step.description||'');else node.textContent=step.description||'';}
      const scalar=(selector,field)=>{const input=this.content.querySelector(selector);input?.addEventListener(field==='title'?'blur':'change',()=>{
        const value=field==='title'?input.value.trim():input.value;if(!value){input.value=plan[field];return;}if(value!==plan[field])void this.mutate(`/plans/${this.id}`,'PUT',{[field]:value});});
        if(field==='title')input?.addEventListener('keydown',event=>{if(event.key==='Enter'){event.preventDefault();input.blur();}else if(event.key==='Escape'){input.value=plan.title;input.blur();}});};
      scalar('[data-plan-title]','title');scalar('[data-plan-status]','status');
      this.content.querySelector('[data-plan-delete]')?.addEventListener('click',()=>{if(confirm('Delete this plan and all of its steps?'))void this.mutate(`/plans/${this.id}`,'DELETE',undefined,{remove:true});});
      this.content.querySelector('[data-plan-anchor]')?.addEventListener('click',()=>this.openPicker(null));
      this.content.querySelector('[data-plan-add-root]')?.addEventListener('click',()=>this.addStep(null));
      this.content.querySelectorAll('[data-step-collapse]').forEach(button=>button.addEventListener('click',()=>{const id=Number(button.dataset.stepCollapse);if(this.collapsed.has(id))this.collapsed.delete(id);else this.collapsed.add(id);this.renderDetail();}));
      this.content.querySelectorAll('[data-step-done]').forEach(input=>input.addEventListener('change',()=>void this.mutate(`/plans/${this.id}/steps/${input.dataset.stepDone}`,'PUT',{done:input.checked},{step:true})));
      this.content.querySelectorAll('[data-step-title]').forEach(input=>{
        const step=find(input.dataset.stepTitle);
        input.addEventListener('blur',()=>{const title=input.value.trim();if(!title){input.value=step.title;return;}if(title!==step.title)void this.mutate(`/plans/${this.id}/steps/${step.id}`,'PUT',{title},{step:true});});
        input.addEventListener('keydown',event=>{if(event.key==='Enter'){event.preventDefault();input.blur();}else if(event.key==='Escape'){input.value=step.title;input.blur();}});
      });
      this.content.querySelectorAll('[data-step-child]').forEach(button=>button.addEventListener('click',()=>this.addStep(Number(button.dataset.stepChild))));
      this.content.querySelectorAll('[data-step-link]').forEach(button=>button.addEventListener('click',()=>this.openPicker(Number(button.dataset.stepLink))));
      this.content.querySelectorAll('[data-step-detach]').forEach(button=>button.addEventListener('click',()=>void this.mutate(`/plans/${this.id}/steps/${button.dataset.stepDetach}`,'PUT',{issue_id:null},{step:true})));
      this.content.querySelectorAll('[data-step-delete]').forEach(button=>button.addEventListener('click',()=>{if(confirm('Delete this step and its children?'))void this.mutate(`/plans/${this.id}/steps/${button.dataset.stepDelete}`,'DELETE',undefined).then(()=>{});}));
      this.content.querySelectorAll('[data-step-move]').forEach(button=>button.addEventListener('click',()=>this.moveStep(find(button.dataset.stepMove))));
      this.content.querySelectorAll('[data-step-order]').forEach(button=>button.addEventListener('click',()=>{
        const step=find(button.dataset.stepOrder),parent=step.parent_step_id==null?null:find(step.parent_step_id),siblings=parent?parent.children:plan.steps;
        const index=siblings.findIndex(item=>item.id===step.id),next=index+Number(button.dataset.delta);
        if(next>=0&&next<siblings.length)void this.mutate(`/plans/${this.id}/steps/${step.id}`,'PUT',movePatch(step.parent_step_id,next),{step:true});
      }));
      this.content.querySelectorAll('[data-step-edit-description]').forEach(button=>button.addEventListener('click',()=>this.editDescription(find(button.dataset.stepEditDescription))));
      this.content.querySelectorAll('a[href*="/issues/"]').forEach(link=>link.addEventListener('click',event=>{
        if(!event.shiftKey)return;event.preventDefault();this.peek?.dispose();
        const identifier=decodeURIComponent(link.getAttribute('href').split('/').at(-1));
        this.peek=globalThis.LificTopcoatIssuePicker.peek(this.root,{request:path=>this.request(path),identifier,onClose:()=>{this.peek=null;}});
      }));
      const anchor=location.hash.match(/^#step-(\d+)$/)?.[1];if(anchor)this.content.querySelector(`[data-plan-step="${Number(anchor)}"]`)?.scrollIntoView({block:'center'});
    }
    dialog(title,markup,onSubmit){
      const node=document.createElement('dialog');node.className='tc-plan-dialog';node.innerHTML=`<h2>${escapeHtml(title)}</h2><form>${markup}<div><button type="submit">Save</button><button type="button" data-dialog-cancel>Cancel</button></div><p data-dialog-error role="alert"></p></form>`;
      this.root.append(node);const previous=document.activeElement;
      const close=()=>{node.close();node.remove();if(previous?.isConnected)previous.focus();};
      node.querySelector('[data-dialog-cancel]').addEventListener('click',close);node.addEventListener('cancel',event=>{event.preventDefault();close();});
      node.querySelector('textarea')?.addEventListener('keydown',event=>{if((event.ctrlKey||event.metaKey)&&event.key.toLowerCase()==='s'){event.preventDefault();node.querySelector('form').requestSubmit();}else if(event.key==='Escape'){event.preventDefault();close();}});
      node.querySelector('form').addEventListener('submit',async event=>{event.preventDefault();const form=event.currentTarget,data=new FormData(form);form.querySelectorAll('button,input,textarea,select').forEach(input=>{input.disabled=true;});
        try{const done=await onSubmit(data);if(done)close();else{form.querySelector('[data-dialog-error]').textContent=this.error.textContent;form.querySelectorAll('button,input,textarea,select').forEach(input=>{input.disabled=false;});}}
        catch(reason){form.querySelector('[data-dialog-error]').textContent=reason.message;form.querySelectorAll('button,input,textarea,select').forEach(input=>{input.disabled=false;});}});
      node.showModal();node.querySelector('input,textarea,select')?.focus();return node;
    }
    addStep(parent){this.dialog(parent===null?'Add step':'Add child step','<label>Step title<input name="title" required></label>',data=>{
      const title=data.get('title').trim();return title?this.mutate(`/plans/${this.id}/steps`,'POST',{title,...parent===null?{}:{parent_step_id:parent}}):false;});}
    editDescription(step){this.dialog('Edit step description',`<label>Description<textarea aria-label="Description" name="description">${escapeHtml(step.description)}</textarea></label>`,data=>this.mutate(`/plans/${this.id}/steps/${step.id}`,'PUT',{description:data.get('description')},{step:true}));}
    moveStep(step){
      const forbidden=new Set([step.id,...flattenSteps(step.children).map(row=>row.step.id)]);
      const choices=flattenSteps(this.plan.steps).filter(row=>!forbidden.has(row.step.id));
      this.dialog('Move step',`<label>Parent<select name="parent"><option value="">Root</option>${choices.map(row=>`<option value="${row.step.id}" ${step.parent_step_id===row.step.id?'selected':''}>${escapeHtml('  '.repeat(row.depth)+row.step.title)}</option>`).join('')}</select></label><label>Position<input name="position" type="number" min="0" value="${step.position||0}" required></label>`,data=>this.mutate(`/plans/${this.id}/steps/${step.id}`,'PUT',movePatch(data.get('parent')===''?null:Number(data.get('parent')),Number(data.get('position'))),{step:true}));
    }
    openPicker(stepId){this.picker?.dispose();this.picker=globalThis.LificTopcoatIssuePicker.mount(this.root,{request:(path)=>this.request(path),project:this.project,
      title:stepId===null?'Set anchor issue':'Link an issue to this step',onClose:()=>{this.picker=null;},
      onSelect:async issue=>{this.picker=null;await this.mutate(stepId===null?`/plans/${this.id}`:`/plans/${this.id}/steps/${stepId}`,'PUT',{issue_id:issue.id},{step:stepId!==null});},
      ...stepId===null?{onClear:async()=>{this.picker=null;await this.mutate(`/plans/${this.id}`,'PUT',{issue_id:null});}}:{}});}
    dispose(){this.disposed=true;this.generation++;clearTimeout(this.timer);this.picker?.dispose();this.peek?.dispose();for(const remove of this.listeners)remove();this.listeners=[];this.root.querySelectorAll('dialog').forEach(node=>node.remove());}
  }
  const api={Controller,flattenSteps,movePatch,provenance,progress};
  globalThis.LificTopcoatPlans=api;if(typeof module!=='undefined')module.exports=api;
  if(typeof document!=='undefined')document.querySelectorAll('[data-topcoat-plans]').forEach(root=>{root._plans=new Controller(root);});
})();

(() => {
  'use strict';
  const escapeHtml=value=>String(value??'').replace(/[&<>"']/g,char=>({'&':'&amp;','<':'&lt;','>':'&gt;','"':'&quot;',"'":'&#39;'}[char]));
  function identifierShape(query,project) {
    const compact=String(query).trim();
    if(/^\d+$/.test(compact))return `${project}-${Number(compact)}`;
    const match=compact.match(/^([a-z][a-z0-9_]*?)[\s-]*(\d+)$/i);
    return match?`${match[1].toUpperCase()}-${Number(match[2])}`:null;
  }
  async function findIssues(request,query,project,{projectOnly=false}={}) {
    const text=query.trim();
    if(!text)return [];
    const identifier=identifierShape(text,project.identifier);
    const [resolved,search]=await Promise.allSettled([
      identifier?request(`/issues/resolve/${encodeURIComponent(identifier)}`):Promise.resolve(null),
      request(`/search?${new URLSearchParams({query:text,project_id:String(project.id)})}`),
    ]);
    if(search.status==='rejected')throw search.reason;
    if(resolved.status==='rejected'&&resolved.reason?.status!==404)throw resolved.reason;
    const rows=[],seen=new Set();
    const append=issue=>{if(!seen.has(issue.identifier)){seen.add(issue.identifier);rows.push(issue);}};
    if(resolved.status==='fulfilled'&&resolved.value&&(!projectOnly||Number(resolved.value.project_id)===Number(project.id)))append(resolved.value);
    for(const hit of search.value||[])if(hit.result_type==='issue'&&hit.identifier&&
      (hit.project_id==null||Number(hit.project_id)===Number(project.id)))append(hit);
    return rows;
  }
  function mount(root,{request,project,projectOnly=false,onSelect,onClear,onClose,title='Link an issue'}={}) {
    const dialog=document.createElement('dialog');dialog.className='tc-issue-picker';
    dialog.innerHTML=`<h2>${escapeHtml(title)}</h2><label>Search issues<input type="search" data-picker-query placeholder="Title, identifier, or number" autocomplete="off"></label>
      <p data-picker-status role="status">Search by title or identifier.</p><p data-picker-error role="alert" hidden></p>
      <div data-picker-results role="listbox" aria-label="Issue search results"></div>
      <div class="tc-picker-actions">${onClear?'<button type="button" data-picker-clear>Clear issue</button>':''}<button type="button" data-picker-close>Cancel</button></div>`;
    root.append(dialog);
    const input=dialog.querySelector('[data-picker-query]'),results=dialog.querySelector('[data-picker-results]');
    const status=dialog.querySelector('[data-picker-status]'),error=dialog.querySelector('[data-picker-error]');
    let alive=true,epoch=0,timer=null,hits=[],selected=0,returnFocus=document.activeElement;
    const listeners=[];
    const listen=(node,event,callback)=>{node.addEventListener(event,callback);listeners.push(()=>node.removeEventListener(event,callback));};
    function dispose(){if(!alive)return;alive=false;epoch++;clearTimeout(timer);for(const remove of listeners)remove();dialog.close();dialog.remove();if(returnFocus?.isConnected)returnFocus.focus();onClose?.();}
    function render(){
      results.innerHTML=hits.map((hit,index)=>`<button type="button" role="option" aria-selected="${index===selected}" data-picker-index="${index}">
        <span>${escapeHtml(hit.identifier)}</span> ${escapeHtml(hit.title)} ${hit.status?`<small>${escapeHtml(hit.status)}</small>`:''}</button>`).join('');
      input.setAttribute('aria-activedescendant',hits.length?`tc-picked-${selected}`:'');
      results.querySelectorAll('[data-picker-index]').forEach((node,index)=>{node.id=`tc-picked-${index}`;});
    }
    async function search(){
      const turn=++epoch;error.hidden=true;status.textContent='Searching…';
      try {
        const next=await findIssues(request,input.value,project,{projectOnly});
        if(!alive||turn!==epoch)return;
        hits=next;selected=0;render();status.textContent=input.value.trim()?(hits.length?`${hits.length} issues`:'No matching issues.'):'Search by title or identifier.';
      } catch(reason){if(alive&&turn===epoch){hits=[];render();status.textContent='';error.hidden=false;error.textContent=reason.message||'Could not search issues. Try again.';}}
    }
    async function pick(index){
      const hit=hits[index];if(!hit)return;
      const turn=++epoch;error.hidden=true;input.disabled=true;results.querySelectorAll('button').forEach(button=>{button.disabled=true;});
      try {
        const issue=await request(`/issues/resolve/${encodeURIComponent(hit.identifier)}`);
        if(!alive||turn!==epoch)return;
        if(projectOnly&&Number(issue.project_id)!==Number(project.id))throw new Error('Choose an issue from this project.');
        dispose();await onSelect(issue);
      } catch(reason){if(alive&&turn===epoch){error.hidden=false;error.textContent=reason.message||'Could not select this issue.';}}
      finally{if(alive){input.disabled=false;results.querySelectorAll('button').forEach(button=>{button.disabled=false;});}}
    }
    listen(input,'input',()=>{epoch++;hits=[];render();clearTimeout(timer);timer=setTimeout(()=>void search(),120);});
    listen(input,'keydown',event=>{
      if(event.key==='ArrowDown'||event.key==='ArrowUp'){event.preventDefault();selected=Math.max(0,Math.min(hits.length-1,selected+(event.key==='ArrowDown'?1:-1)));render();results.querySelector('[aria-selected=true]')?.scrollIntoView({block:'nearest'});}
      else if(event.key==='Enter'){event.preventDefault();clearTimeout(timer);void pick(selected);}
      else if(event.key==='Escape'){event.preventDefault();dispose();}
    });
    listen(results,'click',event=>{const button=event.target.closest('[data-picker-index]');if(button)void pick(Number(button.dataset.pickerIndex));});
    listen(dialog,'cancel',event=>{event.preventDefault();dispose();});
    listen(dialog.querySelector('[data-picker-close]'),'click',dispose);
    if(onClear)listen(dialog.querySelector('[data-picker-clear]'),'click',()=>{dispose();void onClear();});
    dialog.showModal();input.focus();
    return {dispose,search};
  }
  function peek(root,{request,identifier,onClose}={}) {
    const dialog=document.createElement('dialog');dialog.className='tc-issue-picker';
    dialog.innerHTML='<p data-peek-status role="status">Loading issue…</p><div data-peek-content></div><button type="button" data-peek-close>Close peek</button>';
    root.append(dialog);const focused=document.activeElement;let alive=true,epoch=0;
    function dispose(){if(!alive)return;alive=false;epoch++;dialog.close();dialog.remove();if(focused?.isConnected)focused.focus();onClose?.();}
    async function load(){
      const turn=++epoch,status=dialog.querySelector('[data-peek-status]'),content=dialog.querySelector('[data-peek-content]');status.textContent='Loading issue…';content.replaceChildren();
      try {
        const issue=await request(`/issues/resolve/${encodeURIComponent(identifier)}`);if(!alive||turn!==epoch)return;
        content.innerHTML=`<h2>${escapeHtml(issue.identifier)} · ${escapeHtml(issue.title)}</h2><p>${escapeHtml(issue.status)} · ${escapeHtml(issue.priority||'none')}</p><article data-peek-markdown></article>
          <a href="/${encodeURIComponent(issue.identifier.replace(/-\d+$/,''))}/issues/${encodeURIComponent(issue.identifier)}">Open issue</a>`;
        const description=content.querySelector('[data-peek-markdown]');
        if(globalThis.lificIssueEditor?.renderMarkdown)globalThis.lificIssueEditor.renderMarkdown(description,issue.description||'');else description.textContent=issue.description||'';
        status.textContent='';
      }catch(reason){if(alive&&turn===epoch){status.textContent='';const error=document.createElement('p');error.setAttribute('role','alert');error.textContent=reason.message||'Could not load issue.';
        const retry=document.createElement('button');retry.type='button';retry.textContent='Retry';retry.addEventListener('click',()=>void load(),{once:true});content.replaceChildren(error,retry);}}
    }
    dialog.querySelector('[data-peek-close]').addEventListener('click',dispose);dialog.addEventListener('cancel',event=>{event.preventDefault();dispose();});
    dialog.showModal();void load();return {dispose};
  }
  const api={escapeHtml,identifierShape,findIssues,mount,peek};
  globalThis.LificTopcoatIssuePicker=api;
  if(typeof module!=='undefined')module.exports=api;
})();

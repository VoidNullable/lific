(() => {
  'use strict';

  const escape = value => String(value ?? '').replace(/[&<>"']/g, char => ({'&':'&amp;','<':'&lt;','>':'&gt;','"':'&quot;',"'":'&#39;'}[char]));
  const date = value => { const parsed = new Date(value); return Number.isNaN(parsed.valueOf()) ? value : parsed.toLocaleString(); };
  const api = () => globalThis.lificSession;
  const request = (path, options) => api().request(path, options);
  const read = async path => { const result = await request(path); if (!result.ok) throw new Error(result.error); return result.data; };
  const send = (root, action) => root.dispatchEvent(new CustomEvent('lific:issue-detail-intent', {bubbles:true, detail:{route:JSON.parse(root.dataset.route), action}}));

  function commentMarkup(comment, canEdit) {
    const owner = canEdit && comment.user_id === api().state.user?.id;
    return `<li class="tc-comment" data-comment-id="${comment.id}"><header><strong>${escape(comment.author_display_name || comment.author)}</strong><time datetime="${escape(comment.created_at)}">${escape(date(comment.created_at))}</time>${comment.kind === 'verification' ? '<span class="tc-comment__badge">Verification</span>' : ''}</header><p data-comment-content="">${escape(comment.content).replace(/\n/g,'<br>')}</p>${owner ? `<div class="tc-comment__actions"><button type="button" data-comment-edit="${comment.id}">Edit</button><button type="button" data-comment-delete="${comment.id}">Delete</button></div>` : ''}</li>`;
  }

  function issueHref(root, identifier) {
    const project=root.dataset.projectIdentifier;
    if(!project)return '#';
    const local=`/${encodeURIComponent(project)}/issues/${encodeURIComponent(identifier)}`;
    if(root.dataset.publicScope!=='true'&&api().state.publicProject===null)return local;
    return api().scopedRoute?.(local) || `/public/${encodeURIComponent(project)}/issues/${encodeURIComponent(identifier)}`;
  }

  function renderRelations(root) {
    const list = root.querySelector('[data-relation-list]');
    const rows = [['blocked-by',root.dataset.blockedBy],['blocks',root.dataset.blocks],['relates-to',root.dataset.relatesTo],['duplicates',root.dataset.duplicates],['duplicated-by',root.dataset.duplicatedBy]];
    list.innerHTML = rows.flatMap(([kind, values]) => values ? values.split(',').filter(Boolean).map(identifier => {
      const reverseSource = ['blocks','duplicates'].includes(kind) ? root.dataset.identifier : identifier;
      const reverseTarget = ['blocks','duplicates'].includes(kind) ? identifier : root.dataset.identifier;
      const reversible = ['blocks','blocked-by','duplicates','duplicated-by'].includes(kind);
      return `<li data-relation-kind="${kind}" data-relation-target="${escape(identifier)}"><a href="${escape(issueHref(root,identifier))}">${escape(kind.replace('-', ' '))}: ${escape(identifier)}</a>${root.dataset.editEnabled === 'true' ? `<button type="button" data-relation-remove="${escape(identifier)}" aria-label="Remove relation to ${escape(identifier)}">Remove</button>${reversible ? `<button type="button" data-relation-reverse="${escape(identifier)}" data-relation-reverse-source="${escape(reverseSource)}" data-relation-reverse-target="${escape(reverseTarget)}" aria-label="Reverse relation to ${escape(identifier)}">Reverse</button>` : ''}` : ''}</li>`;
    }) : []).join('');
    if (!list.children.length) list.innerHTML = '<li class="tc-collab__muted">No relations</li>';
  }

  function renderWaits(root, waits) {
    const list = root.querySelector('[data-wait-list]');
    list.innerHTML = (waits || []).map(wait => {
      const label = wait.kind === 'user' ? `Waiting for ${wait.display_name || wait.username || 'a user'}` : `Waiting ${wait.earliest}${wait.latest && wait.latest !== wait.earliest ? ` through ${wait.latest}` : ''}`;
      return `<li data-wait-id="${wait.id}"><span>${escape(label)} · ${escape(wait.state)}${wait.note ? ` · ${escape(wait.note)}` : ''}</span>${root.dataset.editEnabled === 'true' ? `<button type="button" data-wait-clear="${wait.id}" aria-label="Clear wait">Clear</button>` : ''}</li>`;
    }).join('') || '<li class="tc-collab__muted">No active waits</li>';
  }

  function renderActivity(root, items) {
    const list = root.querySelector('[data-issue-history]');
    list.innerHTML = (items || []).map(item => `<li><span>${escape(item.actor_display_name || item.actor_username || 'System')}</span>${item.actor_is_bot ? ' <span class="tc-comment__badge">agent</span>' : ''} ${escape(item.action)}${item.field ? ` ${escape(item.field)}` : ''}${item.old_value || item.new_value ? `: ${item.old_value ? `${escape(item.old_value)} → ` : ''}${escape(item.new_value || '')}` : ''}<time datetime="${escape(item.ts)}">${escape(date(item.ts))}</time></li>`).join('') || '<li class="tc-collab__muted">No history yet</li>';
  }

  function renderAttachments(root, items) {
    const target = root.querySelector('[data-issue-attachments]');
    const privateScope = api().state.publicProject === null;
    target.innerHTML = `<ul class="tc-attachments">${(items || []).map(item => `<li class="tc-attachment" data-attachment-id="${item.id}" data-attachment-kind="${globalThis.LificTopcoatAttachments?.viewerKind(item) || 'file'}"><a href="${escape(api().resolve(`/attachments/${item.id}`).url)}" download="${escape(item.filename)}">${escape(item.filename)}</a><span>${Number(item.size_bytes)} bytes</span>${item.mime.startsWith('image/') ? `<img data-attachment-image="${item.id}" alt="${escape(item.alt_text || item.filename)}" loading="lazy">` : ''}${item.uploader_id === api().state.user?.id || api().affordances().manage ? `<button type="button" data-attachment-delete="${item.id}">Delete</button>` : ''}<p data-attachment-message="" role="status"></p></li>`).join('')}</ul>${privateScope ? `<form class="tc-attachment-upload" data-attachment-upload="" data-attachment-entity="issue" data-attachment-entity-id="${root.dataset.issueId}"><label>Attach files <input type="file" multiple data-attachment-files=""></label><button type="submit">Upload</button><button type="button" data-attachment-cancel hidden>Cancel</button><progress data-attachment-progress max="1" value="0" hidden aria-label="Upload progress"></progress><p data-attachment-status role="status"></p></form>` : ''}`;
    const helper = globalThis.LificTopcoatAttachments;
    if (helper) root._attachmentMount?.dispose?.(), root._attachmentMount = helper.attach(target, {
      client:helper.createClient({session:api()}),
      target:{entity_type:'issue',entity_id:Number(root.dataset.issueId)},
      onUploaded:async()=>refreshAttachments(root),
      onDeleted:async()=>refreshAttachments(root),
    });
  }

  async function refreshAttachments(root, current = null) {
    const rows = await read(`/attachments?entity_type=issue&entity_id=${root.dataset.issueId}`);
    if (root.isConnected && (current === null || current === root._collabGeneration)) renderAttachments(root, rows);
  }

  async function refreshComments(root, current = null, before = null) {
    const params=new URLSearchParams({order:'desc',limit:'51'});
    if(before){params.set('before_created_at',before.created_at);params.set('before_id',String(before.id));}
    const result=await request(`/issues/${root.dataset.issueId}/comments?${params}`);
    if(!result.ok)throw new Error(result.error);
    const rows=result.data;
    if (!root.isConnected || (current !== null && current !== root._collabGeneration)) return;
    const overFetched=Array.isArray(rows)&&rows.length>50, page=(Array.isArray(rows)?rows.slice(0,50):rows.items||[]).reverse();
    const comments=before?page.concat(root._comments||[]):page;
    const older=overFetched||result.headers?.get('x-comment-has-more')==='true';
    root._nextCommentCursor=page.length?{created_at:page[0].created_at,id:page[0].id}:before;
    const olderButton=root.querySelector('[data-comments-older]');olderButton.hidden=!older;olderButton.disabled=false;
    root.querySelector('[data-comment-count]').textContent = String(comments.length);
    root.querySelector('[data-comment-thread]').innerHTML = comments.length ? `<ol>${comments.map(item => commentMarkup(item, root.dataset.commentEnabled === 'true')).join('')}</ol>` : '<p class="tc-collab__muted">No comments yet</p>';
    root._comments = comments;
  }

  async function refresh(root) {
    const current = root._collabGeneration;
    const results = await Promise.allSettled([
      refreshComments(root,current), read(`/issues/${root.dataset.issueId}/activity?limit=100`),
      refreshAttachments(root,current),
    ]);
    if (!root.isConnected || current !== root._collabGeneration) return;
    const activity = results[1];
    if (activity.status === 'fulfilled') renderActivity(root, activity.value.items || []);
    const failed = results.find(result => result.status === 'rejected');
    if (failed) root.querySelector('[data-collab-status]').textContent = failed.reason.message;
  }

  function mount(root, props = null) {
    root._issueCollaboration?.dispose?.();
    const detailRoot=root.closest('[data-topcoat-issue-detail]');
    if(!root.dataset.projectIdentifier)root.dataset.projectIdentifier=detailRoot?.dataset.projectIdentifier||props?.project_identifier||props?.projectIdentifier||'';
    if(root.dataset.publicScope===undefined)root.dataset.publicScope=String(detailRoot?.dataset.issueScope==='public'||api().state.publicProject!==null);
    if (props) {
      const issue = props.issue || {};
      const route = props.route || {issue_id:issue.id,generation:0};
      root.dataset.issueId = String(issue.id || route.issue_id || '');
      root.dataset.projectId = String(issue.project_id || '');
      root.dataset.identifier = String(issue.identifier || '');
      root.dataset.projectIdentifier=String(props.project_identifier||props.projectIdentifier||detailRoot?.dataset.projectIdentifier||'');
      const publicScope=props.public_scope??props.publicScope??(detailRoot?.dataset.issueScope==='public'||api().state.publicProject!==null);
      root.dataset.publicScope=String(publicScope);
      root.dataset.route = JSON.stringify(route);
      root.dataset.commentEnabled = String(props.capabilities?.comment === true);
      root.dataset.editEnabled = String(props.capabilities?.edit === true);
      for (const [field,key] of [['blocks','blocks'],['blockedBy','blocked_by'],['relatesTo','relates_to'],['duplicates','duplicates'],['duplicatedBy','duplicated_by']]) root.dataset[field]=(issue[key] || []).join(',');
      root.dataset.waits = JSON.stringify(issue.waits || []);
    }
    if (!Number(root.dataset.issueId)) return {dispose(){}};
    const commentAllowed = props?.capabilities ? props.capabilities.comment === true : root.dataset.commentEnabled === 'true';
    const editAllowed = props?.capabilities ? props.capabilities.edit === true : root.dataset.editEnabled === 'true';
    root.dataset.commentEnabled=String(commentAllowed);root.dataset.editEnabled=String(editAllowed);
    const composer=root.querySelector('[data-comment-compose]'),relationForm=root.querySelector('[data-relation-create]'),waitForm=root.querySelector('[data-wait-create]'),deleteButton=root.querySelector('[data-issue-delete]');
    if(composer)composer.hidden=!commentAllowed;if(relationForm)relationForm.hidden=!editAllowed;if(waitForm)waitForm.hidden=!editAllowed;if(deleteButton)deleteButton.hidden=!editAllowed;
    let disposed = false;
    let generation = 0;
    root._collabGeneration = (root._collabGeneration || 0) + 1;
    const initialRoute = root.dataset.route;
    const say = message => { root.querySelector('[data-collab-status]').textContent = message; };
    const pendingActions=[];
    const snapshotForm=form=>Object.fromEntries(new FormData(form).entries());
    const trackAction=(action,form=null,draft=null)=>pendingActions.push({action,form,values:form?snapshotForm(form):null,draft});
    const sameAction=(left,right)=>!right||Object.entries(right).every(([key,value])=>JSON.stringify(left[key])===JSON.stringify(value));
    function takeAction(detail) {
      const index=pendingActions.findIndex(item=>item.action.panel===detail.panel&&(!detail.operation||item.action.operation===detail.operation)&&sameAction(item.action,detail.action));
      return index<0?null:pendingActions.splice(index,1)[0];
    }
    function commitDraft(detail) {
      const pending=takeAction(detail);if(!pending)return;
      if(pending.action.operation==='create_comment'){
        const input=root.querySelector('[data-comment-draft]');if(input&&input.value===pending.draft)input.value='';
      } else if(pending.action.operation==='link_relation'&&pending.form){
        const values=snapshotForm(pending.form);if(values.target===pending.values.target&&values.kind===pending.values.kind)pending.form.reset();
      } else if(pending.action.operation==='add_wait'&&pending.form){
        const values=snapshotForm(pending.form);if(Object.keys(pending.values).every(key=>values[key]===pending.values[key])){pending.form.reset();onWaitKind({target:pending.form.querySelector('select[name="kind"]')});}
      }
    }
    function publishIssuePanels(issue) {
      if(!issue)return;
      for(const [field,dataset] of [['blocks','blocks'],['blocked_by','blockedBy'],['relates_to','relatesTo'],['duplicates','duplicates'],['duplicated_by','duplicatedBy']])root.dataset[dataset]=(issue[field]||[]).join(',');
      renderRelations(root);renderWaits(root,issue.waits||[]);
    }
    renderRelations(root);
    let waits = [];
    try { waits = JSON.parse(root.dataset.waits || '[]'); } catch { /* Missing optional wait snapshot. */ }
    renderWaits(root, waits);
    const keepFocus = (node, offset) => { node.focus(); if (typeof offset === 'number') node.setSelectionRange(offset, offset); };
    let candidates = [];
    let mentionMatches = [], mentionIndex = 0;
    const candidateGeneration=generation;
    void request(`/projects/${root.dataset.projectId}/mention-candidates`).then(result => { if (!disposed && generation===candidateGeneration && result.ok) candidates = result.data; });

    async function onSubmit(event) {
      const form = event.target;
      if (form.matches('[data-comment-compose]')) {
        event.preventDefault();
        const input = form.querySelector('[data-comment-draft]'), content = input.value.trim();
        if (!content) return;
        const action={type:'mutate_panel',panel:'comments',operation:'create_comment',content};trackAction(action,form,input.value);
        send(root,action);keepFocus(input,input.value.length);
      } else if (form.matches('[data-relation-create]')) {
        event.preventDefault();
        const data = new FormData(form), target = String(data.get('target') || '').trim(), kind = String(data.get('kind'));
        if (!/^[A-Za-z][A-Za-z0-9_-]*-[1-9][0-9]*$/.test(target)) { say('Enter a valid issue identifier.'); return; }
        const action={type:'mutate_panel',panel:'relations',operation:'link_relation',source:root.dataset.identifier,target,kind};trackAction(action,form);send(root,action);
      } else if (form.matches('[data-wait-create]')) {
        event.preventDefault();
        const data = new FormData(form), kind = String(data.get('kind'));
        const input = kind === 'user' ? {user:String(data.get('user') || '').trim(),note:String(data.get('note') || '')} : {from:String(data.get('from') || ''),until:String(data.get('until') || '') || undefined,note:String(data.get('note') || '')};
        if (kind === 'user' ? !input.user : !input.from) { say(kind === 'user' ? 'Enter a username.' : 'Choose a start date.'); return; }
        const action={type:'mutate_panel',panel:'waits',operation:'add_wait',input};trackAction(action,form);send(root,action);
      }
    }
    async function onClick(event) {
      const older=event.target.closest('[data-comments-older]');
      if(older){older.disabled=true;const cursor=root._nextCommentCursor;try{if(cursor)await refreshComments(root,root._collabGeneration,cursor);}catch(error){say(error.message);}root.querySelector('[data-comments-older]').focus();return;}
      const edit = event.target.closest('[data-comment-edit]');
      const remove = event.target.closest('[data-comment-delete]');
      const unlink = event.target.closest('[data-relation-remove]');
      const reverse = event.target.closest('[data-relation-reverse]');
      const clear = event.target.closest('[data-wait-clear]');
      if (edit) {
        const id = Number(edit.dataset.commentEdit), row = edit.closest('[data-comment-id]'), comment = root._comments.find(item => item.id === id);
        if (!comment || row.querySelector('textarea')) return;
        const field = document.createElement('textarea'); field.value = comment.content; field.setAttribute('aria-label','Edit comment');
        const save = document.createElement('button'); save.type='button'; save.textContent='Save comment'; save.dataset.commentSave=String(id);
        const cancel = document.createElement('button'); cancel.type='button'; cancel.textContent='Cancel'; cancel.dataset.commentCancel='';
        const holder = document.createElement('div'); holder.dataset.commentEditor=''; holder.append(field,save,cancel);
        row.querySelector('[data-comment-content]').replaceWith(holder); field.focus();
      }
      const save = event.target.closest('[data-comment-save]');
      if (save) {
        const row = save.closest('[data-comment-id]'), field = row.querySelector('textarea'), content = field.value.trim();
        if (content) {const action={type:'mutate_panel',panel:'comments',operation:'edit_comment',comment_id:Number(save.dataset.commentSave),content};trackAction(action);send(root,action);}
      }
      if (event.target.closest('[data-comment-cancel]')) void refreshComments(root);
      if (remove) {
        const id = Number(remove.dataset.commentDelete);
        if (globalThis.confirm('Delete this comment?')) {const action={type:'mutate_panel',panel:'comments',operation:'delete_comment',comment_id:id};trackAction(action);send(root,action);}
      }
      if (unlink) {const action={type:'mutate_panel',panel:'relations',operation:'unlink_relation',source:root.dataset.identifier,target:unlink.dataset.relationRemove};trackAction(action);send(root,action);}
      if (reverse) {const action={type:'mutate_panel',panel:'relations',operation:'reverse_relation',source:reverse.dataset.relationReverseSource,target:reverse.dataset.relationReverseTarget};trackAction(action);send(root,action);}
      if (clear) {const action={type:'mutate_panel',panel:'waits',operation:'clear_wait',wait_id:Number(clear.dataset.waitClear)};trackAction(action);send(root,action);}
      if (event.target.closest('[data-issue-delete]') && globalThis.confirm('Delete this issue?')) send(root,{type:'delete'});
      if (event.target.closest('[data-issue-restore]')) send(root,{type:'restore'});
    }
    function onInput(event) {
      if (!event.target.matches('[data-comment-draft]')) return;
      const node = event.target, caret = node.selectionStart, match = node.value.slice(0,caret).match(/(?:^|\s)@([\w-]*)$/), list = root.querySelector('[data-mention-list]');
      if (!match) { list.hidden = true; mentionMatches=[]; return; }
      const query = match[1].toLowerCase(); mentionMatches = candidates.filter(user => `${user.username} ${user.display_name}`.toLowerCase().includes(query)).slice(0,8); mentionIndex=0;
      list.innerHTML = mentionMatches.map((user,index) => `<button type="button" role="option" aria-selected="${index===mentionIndex}" data-mention-index="${index}" data-mention-user="${escape(user.username)}"><strong>@${escape(user.username)}</strong> ${escape(user.display_name)}</button>`).join(''); list.hidden = !mentionMatches.length;
    }
    function chooseMention(index) {
      const user=mentionMatches[index], input=root.querySelector('[data-comment-draft]');if(!user||!input)return;
      const before=input.value.slice(0,input.selectionStart),at=before.lastIndexOf('@');
      input.setRangeText(`@${user.username} `,at,input.selectionStart,'end');root.querySelector('[data-mention-list]').hidden=true;mentionMatches=[];input.focus();
    }
    function onKeydown(event) {
      if(!event.target.matches('[data-comment-draft]'))return;
      const list=root.querySelector('[data-mention-list]');if(list.hidden||!mentionMatches.length)return;
      if(event.key==='ArrowDown'||event.key==='ArrowUp'){event.preventDefault();mentionIndex=(mentionIndex+(event.key==='ArrowDown'?1:-1)+mentionMatches.length)%mentionMatches.length;for(const option of list.querySelectorAll('[role=option]'))option.setAttribute('aria-selected',String(Number(option.dataset.mentionIndex)===mentionIndex));}
      else if(event.key==='Enter'||event.key==='Tab'){event.preventDefault();chooseMention(mentionIndex);}
      else if(event.key==='Escape'){event.preventDefault();list.hidden=true;}
    }
    function onMention(event) {
      const choice = event.target.closest('[data-mention-user]'); if (!choice) return;
      chooseMention(Number(choice.dataset.mentionIndex));
    }
    function onApplied(event) {
      if (event.detail?.route?.issue_id !== Number(root.dataset.issueId) || event.detail?.route?.generation !== JSON.parse(initialRoute).generation) return;
      if(event.detail.panel)commitDraft(event.detail);
      if (event.detail.panel === 'comments') void refreshComments(root,root._collabGeneration);
      publishIssuePanels(event.detail.issue);
      if (event.detail.panel === 'comments' || event.detail.panel === 'relations' || event.detail.panel === 'waits') void read(`/issues/${root.dataset.issueId}/activity?limit=100`).then(data=>{if(root.isConnected)renderActivity(root,data.items || []);});
      if (event.detail.kind === 'deleted' || event.detail.deleted) { root.querySelector('[data-issue-delete]')?.setAttribute('hidden',''); root.querySelector('[data-issue-restore]')?.removeAttribute('hidden'); }
      if (event.detail.kind === 'restored' || event.detail.restored) { root.querySelector('[data-issue-delete]')?.removeAttribute('hidden'); root.querySelector('[data-issue-restore]')?.setAttribute('hidden',''); }
      say('Saved.');
    }
    function onConflict(event) {
      if (event.detail?.route?.issue_id !== Number(root.dataset.issueId) || event.detail?.route?.generation !== JSON.parse(initialRoute).generation) return;
      if(!['comments','relations','waits'].includes(event.detail.panel))return;
      takeAction(event.detail);publishIssuePanels(event.detail.current);
      say(event.detail.error||'This issue changed elsewhere. Your draft is still here; review the latest values and retry.');
    }
    function onError(event) {
      if(event.detail?.route?.issue_id!==Number(root.dataset.issueId)||event.detail?.route?.generation!==JSON.parse(initialRoute).generation)return;
      if(!['comments','relations','waits'].includes(event.detail.panel))return;
      takeAction(event.detail);say(event.detail.error||'Could not save. Your draft is still here; try again.');
    }
    function onScope() {
      generation++; root._collabGeneration++;
      root.dataset.publicScope=String(api().state.publicProject!==null);
      root.querySelector('[data-comment-thread]').innerHTML=''; root.querySelector('[data-comments-older]').hidden=true;root.querySelector('[data-issue-history]').innerHTML=''; root.querySelector('[data-issue-attachments]').replaceChildren(); root._comments=[]; candidates=[];
      const affordances=api().affordances(), comment=affordances.comment===true, edit=affordances.edit===true;
      root.dataset.commentEnabled=String(comment);root.dataset.editEnabled=String(edit);
      const compose=root.querySelector('[data-comment-compose]'), relation=root.querySelector('[data-relation-create]'), wait=root.querySelector('[data-wait-create]'), del=root.querySelector('[data-issue-delete]');
      if(compose)compose.hidden=!comment;if(relation)relation.hidden=!edit;if(wait)wait.hidden=!edit;if(del)del.hidden=!edit;
      renderRelations(root);renderWaits(root,[]);
      if(root.dataset.projectId) {const candidateGeneration=generation;void request(`/projects/${root.dataset.projectId}/mention-candidates`).then(result=>{if(!disposed&&generation===candidateGeneration&&result.ok)candidates=result.data;});}
      queueMicrotask(()=>{if(!disposed)void refresh(root);});
    }
    function onWaitKind(event) {
      if (!event.target.matches('[data-wait-create] select[name="kind"]')) return;
      const user = root.querySelector('[data-wait-user-field]'), from = root.querySelector('[data-wait-date-field]'), until = root.querySelector('[data-wait-until-field]'), dateMode = event.target.value === 'date';
      user.hidden=dateMode; from.hidden=!dateMode; until.hidden=!dateMode;
    }
    root.addEventListener('submit',onSubmit); root.addEventListener('click',onClick); root.addEventListener('input',onInput); root.addEventListener('click',onMention);
    root.addEventListener('change',onWaitKind);root.addEventListener('keydown',onKeydown); window.addEventListener('lific:issue-detail-applied',onApplied); window.addEventListener('lific:issue-detail-conflict',onConflict); window.addEventListener('lific:account-change',onScope); window.addEventListener('lific:scope-change',onScope);
    window.addEventListener('lific:issue-detail-error',onError);
    root._issueCollaboration={
      refresh:()=>refresh(root),
      update(issue,capabilities) {
        if(!issue)return;
        root._collabGeneration++;
        root.dataset.issueId=String(issue.id);root.dataset.projectId=String(issue.project_id);root.dataset.identifier=String(issue.identifier||'');
        root.dataset.waits=JSON.stringify(issue.waits||[]);
        if(capabilities){root.dataset.commentEnabled=String(capabilities.comment===true);root.dataset.editEnabled=String(capabilities.edit===true);composer.hidden=!capabilities.comment;relationForm.hidden=!capabilities.edit;waitForm.hidden=!capabilities.edit;deleteButton.hidden=!capabilities.edit;}
        publishIssuePanels(issue);
      },
      dispose(){disposed=true;generation++;root._collabGeneration++;root.removeEventListener('submit',onSubmit);root.removeEventListener('click',onClick);root.removeEventListener('input',onInput);root.removeEventListener('change',onWaitKind);root.removeEventListener('keydown',onKeydown);window.removeEventListener('lific:issue-detail-applied',onApplied);window.removeEventListener('lific:issue-detail-conflict',onConflict);window.removeEventListener('lific:issue-detail-error',onError);window.removeEventListener('lific:account-change',onScope);window.removeEventListener('lific:scope-change',onScope);root._attachmentMount?.dispose?.();}
    };
    void refresh(root);
    return root._issueCollaboration;
  }
  globalThis.LificTopcoatIssueCollaboration={mount,commentMarkup,renderRelations,renderWaits,renderActivity};
  if (typeof document !== 'undefined') for (const root of document.querySelectorAll('[data-topcoat-collaboration]')) if (Number(root.dataset.issueId)) mount(root);
})();

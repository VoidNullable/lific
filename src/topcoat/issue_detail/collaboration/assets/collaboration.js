(() => {
  'use strict';

  const escape = value => String(value ?? '').replace(/[&<>"']/g, char => ({'&':'&amp;','<':'&lt;','>':'&gt;','"':'&quot;',"'":'&#39;'}[char]));
  const date = value => { const parsed = new Date(value); return Number.isNaN(parsed.valueOf()) ? value : parsed.toLocaleString(); };
  const api = () => globalThis.lificSession;
  const request = (path, options) => api().request(path, options);
  const read = async path => { const result = await request(path); if (!result.ok) throw new Error(result.error); return result.data; };
  const send = (root, action) => root.dispatchEvent(new CustomEvent('lific:issue-detail-intent', {bubbles:true, detail:{route:JSON.parse(root.dataset.route), action}}));

  function referenceSummary(content) {
    const issues = new Set(String(content).match(/\b[A-Z][A-Z0-9_-]*-[1-9][0-9]*\b/g) || []);
    const attachments = new Set(String(content).match(/\/api\/attachments\/(\d+)/g) || []);
    const mentions = new Set(String(content).match(/(?:^|\s)@[\w-]+/g) || []);
    return {issues:issues.size,attachments:attachments.size,mentions:mentions.size};
  }

  function commentMarkup(comment, canEdit, draft = null) {
    const owner = canEdit && comment.user_id === api().state.user?.id;
    const body = draft ? `<div data-comment-editor=""><textarea data-comment-edit-draft="${comment.id}" aria-label="Edit comment" ${canEdit ? '' : 'readonly'}>${escape(draft.content)}</textarea><div class="tc-collab__upload-tools"><label>Attach files <input type="file" multiple data-comment-files="${comment.id}" ${canEdit ? '' : 'disabled'}></label><span data-comment-upload-status="${comment.id}" role="status"></span></div><button type="button" data-comment-save="${comment.id}" ${!canEdit||draft.uploading ? 'disabled' : ''}>Save comment</button><button type="button" data-comment-cancel="${comment.id}">Cancel</button></div>` : `<div class="tc-comment__body" data-comment-content="">${escape(comment.content)}</div>`;
    const refs=referenceSummary(comment.content);
    return `<li class="tc-comment" id="comment-${comment.id}" data-comment-id="${comment.id}"><header><strong>${escape(comment.author_display_name || comment.author)}</strong><time datetime="${escape(comment.created_at)}">${escape(date(comment.created_at))}</time><a class="tc-comment__permalink" href="#comment-${comment.id}" aria-label="Link to this comment">#</a>${comment.kind === 'verification' ? '<span class="tc-comment__badge">Verification</span>' : ''}</header>${body}${refs.issues||refs.attachments||refs.mentions?`<small class="tc-comment__references">References: ${refs.issues?`${refs.issues} issue${refs.issues===1?'':'s'}`:''}${refs.attachments?`${refs.issues?', ':''}${refs.attachments} attachment${refs.attachments===1?'':'s'}`:''}${refs.mentions?`${refs.issues||refs.attachments?', ':''}${refs.mentions} mention${refs.mentions===1?'':'s'}`:''}</small>`:''}${owner ? `<div class="tc-comment__actions"><button type="button" data-comment-edit="${comment.id}">Edit</button><button type="button" data-comment-delete="${comment.id}">Delete</button></div>` : ''}</li>`;
  }

  function decorateCommentReferences(node) {
    const walker=node.ownerDocument.createTreeWalker(node,NodeFilter.SHOW_TEXT);
    const textNodes=[];while(walker.nextNode())textNodes.push(walker.currentNode);
    for(const textNode of textNodes){
      if(textNode.parentElement?.closest('code,pre,a,script,style,.tc-comment__mention'))continue;
      const text=textNode.nodeValue, pattern=/(@[\w-]+|\b[A-Z][A-Z0-9_-]*-[1-9][0-9]*\b)/g;
      if(!pattern.test(text))continue;pattern.lastIndex=0;const fragment=node.ownerDocument.createDocumentFragment();let offset=0,match;
      while((match=pattern.exec(text))){fragment.append(text.slice(offset,match.index));const token=match[0];
        if(token.startsWith('@')){const span=node.ownerDocument.createElement('span');span.className='tc-comment__mention';span.textContent=token;fragment.append(span);}
        else {const a=node.ownerDocument.createElement('a');a.href=issueHref(node.closest('[data-topcoat-collaboration]'),token);a.textContent=token;fragment.append(a);}
        offset=pattern.lastIndex;
      }
      fragment.append(text.slice(offset));textNode.replaceWith(fragment);
    }
  }

  function renderCommentBodies(root) {
    for(const body of root.querySelectorAll('[data-comment-content]')){
      const source=body.textContent||'';
      if(globalThis.lificIssueEditor?.renderMarkdown)globalThis.lificIssueEditor.renderMarkdown(body,source);
      else body.innerHTML=escape(source).replace(/\n/g,'<br>');
      decorateCommentReferences(body);
    }
  }

  function renderCommentList(root, comments, canEdit = root.dataset.commentEnabled === 'true') {
    const active=root.ownerDocument.activeElement,activeField=active?.matches?.('[data-comment-edit-draft]')&&root.contains(active)?active:null;
    let focusedId=null;
    for(const state of root._commentEdits?.values?.()||[])state.focused=false;
    if(activeField){focusedId=Number(activeField.dataset.commentEditDraft);const state=root._commentEdits.get(focusedId)||{};Object.assign(state,{content:activeField.value,selectionStart:activeField.selectionStart,selectionEnd:activeField.selectionEnd,focused:true});root._commentEdits.set(focusedId,state);}
    const editors=new Map([...root.querySelectorAll('[data-comment-id] [data-comment-editor]')].map(editor=>[Number(editor.closest('[data-comment-id]').dataset.commentId),editor]));
    root.querySelector('[data-comment-count]').textContent=String(comments.length);
    root.querySelector('[data-comment-thread]').innerHTML=comments.length?`<ol>${comments.map(item=>commentMarkup(item,canEdit,root._commentEdits?.get(Number(item.id)))).join('')}</ol>`:'<p class="tc-collab__muted">No comments yet</p>';
    for(const [id,editor] of editors){const replacement=root.querySelector(`[data-comment-id="${id}"] [data-comment-editor]`);if(replacement){editor.querySelector('textarea').readOnly=!canEdit;editor.querySelector('[data-comment-files]').disabled=!canEdit;editor.querySelector('[data-comment-save]').disabled=!canEdit||Boolean(root._commentEdits.get(id)?.uploading);replacement.replaceWith(editor);}}
    root._syncCommentComposers?.();
    renderCommentBodies(root);
    if(focusedId!==null){const state=root._commentEdits.get(focusedId),field=root.querySelector(`[data-comment-edit-draft="${focusedId}"]`);if(field){field.focus();field.setSelectionRange(state.selectionStart??field.value.length,state.selectionEnd??state.selectionStart??field.value.length);}}
  }

  function issueHref(root, identifier) {
    const project=String(identifier).match(/^([A-Za-z][A-Za-z0-9_-]*)-[1-9][0-9]*$/)?.[1];
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
    if (!target.querySelector('[data-issue-attachment-list]')) target.innerHTML=`<ul class="tc-attachments" data-issue-attachment-list=""></ul>${privateScope ? `<form class="tc-attachment-upload" data-attachment-upload=""><label>Attach files <input type="file" multiple data-attachment-files=""></label><button type="submit">Upload</button><button type="button" data-attachment-cancel hidden>Cancel</button><progress data-attachment-progress max="1" value="0" hidden aria-label="Upload progress"></progress><p data-attachment-status role="status"></p></form>` : ''}`;
    const list=target.querySelector('[data-issue-attachment-list]'),rows=items||[],wanted=new Set(rows.map(item=>String(item.id)));
    for(const card of list.querySelectorAll('[data-attachment-id]'))if(!wanted.has(card.dataset.attachmentId))card.remove();
    for(const item of rows){
      if(list.querySelector(`[data-attachment-id="${Number(item.id)}"]`))continue;
      const kind=globalThis.LificTopcoatAttachments?.viewerKind(item)||'file',url=api().resolve(`/attachments/${item.id}`).url,card=document.createElement('li');
      card.className='tc-attachment';card.dataset.attachmentId=String(item.id);card.dataset.attachmentKind=kind;
      const link=document.createElement('a');link.href=url;link.download=item.filename;link.textContent=item.filename;card.append(link);
      const size=document.createElement('span');size.textContent=`${Number(item.size_bytes)} bytes`;card.append(size);
      if(kind==='image'){const image=document.createElement('img');image.dataset.attachmentImage=String(item.id);image.alt=item.alt_text||item.filename;image.loading='lazy';card.append(image);}
      if(kind==='video'||kind==='audio'){const media=document.createElement(kind);media.controls=true;media.preload='none';media.src=url;card.append(media);}
      const preview=document.createElement('button');preview.type='button';preview.setAttribute(kind==='image'?'data-attachment-original':'data-attachment-preview','');preview.setAttribute('aria-label',`${kind==='image'?'Original':'Preview'} ${item.filename}`);preview.textContent=kind==='image'?'Original':'Preview';card.append(preview);
      const content=document.createElement('pre');content.dataset.attachmentContent='';content.hidden=true;card.append(content);
      if(item.uploader_id===api().state.user?.id||api().affordances().manage){const remove=document.createElement('button');remove.type='button';remove.dataset.attachmentDelete=String(item.id);remove.textContent='Delete';card.append(remove);}
      const message=document.createElement('p');message.dataset.attachmentMessage='';message.setAttribute('role','status');card.append(message);list.append(card);
      if(kind==='image'&&root._attachmentClient){void root._attachmentClient.thumbnail(Number(item.id)).then(result=>{if(card.isConnected&&result.ok){const src=URL.createObjectURL(result.blob);root._attachmentObjectUrls||=new Set();root._attachmentObjectUrls.add(src);card.querySelector('img').src=src;}else if(card.isConnected&&result.status===404)card.querySelector('img').src=url;});}
    }
    const helper = globalThis.LificTopcoatAttachments;
    if (helper && !root._attachmentMount) root._attachmentMount = helper.attach(target, {
      client:root._attachmentClient ||= helper.createClient({session:api()}),
      target:{entity_type:'issue',entity_id:Number(root.dataset.issueId)},
      onUploaded:async()=>refreshAttachments(root),
      onDeleted:async()=>refreshAttachments(root),
    });
  }

  async function refreshAttachments(root, current = null) {
    const rows = await read(`/attachments?entity_type=issue&entity_id=${root.dataset.issueId}`);
    if (root.isConnected && (current === null || current === root._collabGeneration)) renderAttachments(root, rows);
  }

  async function fetchCommentPage(root, current, before) {
    const params=new URLSearchParams({order:'desc',limit:'51'});
    if(before){params.set('before_created_at',before.created_at);params.set('before_id',String(before.id));}
    const result=await request(`/issues/${root.dataset.issueId}/comments?${params}`);
    if(!result.ok)throw new Error(result.error);
    const rows=Array.isArray(result.data)?result.data:(result.data?.items||[]);
    return {page:rows.slice(0,50).reverse(),hasOlder:rows.length>50||result.headers?.get('x-comment-has-more')==='true'};
  }

  async function refreshComments(root, current = null, before = null) {
    const existing=root._comments||[];
    const first=await fetchCommentPage(root,current,before);
    if (!root.isConnected || (current !== null && current !== root._collabGeneration)) return;
    let page=first.page,older=first.hasOlder;
    let comments;
    let cursor=page.length?{created_at:page[0].created_at,id:page[0].id}:before;
    if(before){
      const byId=new Map(existing.map(item=>[Number(item.id),item]));for(const item of page)byId.set(Number(item.id),item);
      comments=[...byId.values()].sort((a,b)=>String(a.created_at).localeCompare(String(b.created_at))||Number(a.id)-Number(b.id));
      root._commentsWindowExpanded=true;
    } else if(root._commentsWindowExpanded&&existing.length){
      const priorIds=new Set(existing.map(item=>Number(item.id)));
      const newRows=page.filter(item=>!priorIds.has(Number(item.id))).length;
      const desired=existing.length+newRows, seen=new Map(page.map(item=>[Number(item.id),item]));
      comments=page;
      while(comments.length<desired&&older){
        if(!cursor)break;
        const next=await fetchCommentPage(root,current,cursor);
        if(!root.isConnected||(current!==null&&current!==root._collabGeneration))return;
        page=next.page;older=next.hasOlder;
        for(const item of page)seen.set(Number(item.id),item);
        comments=[...seen.values()].sort((a,b)=>String(a.created_at).localeCompare(String(b.created_at))||Number(a.id)-Number(b.id));
        if(!page.length)break;
        const nextCursor={created_at:page[0].created_at,id:page[0].id};
        if(cursor.created_at===nextCursor.created_at&&Number(cursor.id)===Number(nextCursor.id))break;
        cursor=nextCursor;
      }
    } else comments=first.page;
    root._commentsHasOlder=older;
    root._nextCommentCursor=cursor;
    const olderButton=root.querySelector('[data-comments-older]');olderButton.hidden=!older;olderButton.disabled=false;
    root._comments = comments;
    renderCommentList(root,comments);
    if(!before)await resolveCommentHash(root,current);
  }

  async function resolveCommentHash(root,current=null) {
    const match=globalThis.location?.hash?.match(/^#comment-([1-9][0-9]*)$/)||globalThis.location?.hash?.match(/[?&]comment=([1-9][0-9]*)/);
    if(!match)return false;
    const targetId=match[1];if(root._commentTargetDone===targetId)return false;if(root._commentTargetPromise&&root._commentTargetId===targetId)return root._commentTargetPromise;
    const id=Number(targetId),promise=(async()=>{let budget=10;
      while(root.isConnected&&root._comments?.every(item=>Number(item.id)!==id)&&!root.querySelector('[data-comments-older]').hidden&&budget-->0){const cursor=root._nextCommentCursor;if(!cursor)break;await refreshComments(root,current,cursor);}
      const target=root.querySelector(`#comment-${id}`);
      if(target){target.classList.add('tc-comment--target');target.scrollIntoView?.({block:'center'});return true;}
      return false;
    })();
    root._commentTargetId=targetId;root._commentTargetPromise=promise;
    try{return await promise;}finally{root._commentTargetDone=targetId;if(root._commentTargetPromise===promise){root._commentTargetPromise=null;root._commentTargetId=null;}}
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
    let audienceGeneration = 0;
    root._collabGeneration = (root._collabGeneration || 0) + 1;
    const initialRoute = root.dataset.route;
    root._commentEdits ||= new Map();
    root._commentUploads ||= new Map();
    root._composerClient ||= globalThis.LificTopcoatAttachments?.createClient({session:api()});
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
      } else if(pending.action.operation==='edit_comment') {
        const id=Number(pending.action.comment_id),draft=root._commentEdits.get(id);
        if(draft?.content===pending.draft)root._commentEdits.delete(id);
      } else if(pending.action.operation==='delete_comment') {
        root._commentEdits.delete(Number(pending.action.comment_id));
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
    function commentTextarea(id='new') { return id==='new'?root.querySelector('[data-comment-draft]'):root.querySelector(`[data-comment-edit-draft="${Number(id)}"]`); }
    function updateUploadGate(id, uploading) {
      const form=id==='new'?root.querySelector('[data-comment-compose]'):root.querySelector(`[data-comment-id="${Number(id)}"] [data-comment-editor]`);
      const submit=id==='new'?form?.querySelector('[type="submit"]'):form?.querySelector(`[data-comment-save="${Number(id)}"]`);
      if(submit)submit.disabled=uploading;
      const status=root.querySelector(`[data-comment-upload-status="${id}"]`);if(status)status.textContent=uploading?'Uploading attachments…':'';
    }
    function cancelCommentUploads(id) {
      const queue=root._commentUploads.get(String(id));if(!queue)return;
      root._commentUploads.delete(String(id));queue.dispose();
      const state=id==='new'?null:root._commentEdits.get(Number(id));if(state)state.uploading=false;
      updateUploadGate(id,false);
    }
    function commentComposer(id='new') {
      id=String(id);if(root._commentUploads.has(id))return root._commentUploads.get(id);
      const field=commentTextarea(id),parent=id==='new'?composer:field?.closest('[data-comment-editor]');
      if(!field||!parent||!root._composerClient||root.dataset.commentEnabled!=='true')return null;
      const host=document.createElement('div');host.dataset.commentUploadQueue=id;parent.append(host);
      host.addEventListener('lific:attachment-busy',event=>{const state=id==='new'?null:root._commentEdits.get(Number(id));if(state)state.uploading=event.detail.busy;updateUploadGate(id,event.detail.busy);});
      const write=value=>{const current=commentTextarea(id);if(current){current.value=value;current.dispatchEvent(new Event('input',{bubbles:true}));}};
      const controller=globalThis.LificTopcoatAttachments.createComposer({root:host,client:root._composerClient,target:null,concurrency:1,textarea:field,text:{read:()=>commentTextarea(id)?.value||'',write},
        onStatus:message=>{const status=root.querySelector(`[data-comment-upload-status="${id}"]`);if(status)status.textContent=message;},
        onUploaded:(_attachment,markdown)=>{
          const current=commentTextarea(id);if(!current)return;const text=current.value,start=current.selectionStart??text.length,end=current.selectionEnd??start,before=text.slice(0,start),after=text.slice(end),prefix=before&&!/\s$/.test(before)?' ':'',suffix=after&&!/^\s/.test(after)?' ':'';
          current.value=`${before}${prefix}${markdown}${suffix}${after}`;const caret=start+prefix.length+markdown.length;current.setSelectionRange(caret,caret);current.dispatchEvent(new Event('input',{bubbles:true}));
        }});
      const queue={get pending(){return controller.pending;},enqueue:(files,options)=>controller.enqueue(files,options),dispose(){controller.dispose();host.remove();}};
      root._commentUploads.set(id,queue);return queue;
    }
    root._syncCommentComposers=()=>{
      for(const id of [...root._commentUploads.keys()])if(id!=='new'&&(!root._commentEdits.has(Number(id))||!commentTextarea(id)))cancelCommentUploads(id);
      if(root.dataset.commentEnabled==='true'){commentComposer('new');for(const id of root._commentEdits.keys())commentComposer(String(id));}
    };
    async function uploadCommentFiles(id, files) {await commentComposer(id)?.enqueue(Array.from(files||[]));}
    let candidates = [];
    let mentionMatches = [], mentionIndex = 0;
    const candidateGeneration=generation;
    void request(`/projects/${root.dataset.projectId}/mention-candidates`).then(result => { if (!disposed && generation===candidateGeneration && result.ok) candidates = result.data; });

    async function onSubmit(event) {
      const form = event.target;
      if (form.matches('[data-comment-compose]')) {
        event.preventDefault();
        const input = form.querySelector('[data-comment-draft]'), content = input.value.trim();
        if(root._commentUploads.get('new')?.pending){say('Wait for comment attachments to finish uploading.');return;}
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
      const original=event.target.closest('[data-attachment-original]');
      if(original){const card=original.closest('[data-attachment-id]'),image=card?.querySelector('[data-attachment-image]');if(image&&root._composerClient)image.src=root._composerClient.url(Number(card.dataset.attachmentId),'original');return;}
      const older=event.target.closest('[data-comments-older]');
      if(older){older.disabled=true;root._commentTargetDone=null;const cursor=root._nextCommentCursor;try{if(cursor)await refreshComments(root,root._collabGeneration,cursor);}catch(error){say(error.message);}root.querySelector('[data-comments-older]').focus();return;}
      const edit = event.target.closest('[data-comment-edit]');
      const remove = event.target.closest('[data-comment-delete]');
      const unlink = event.target.closest('[data-relation-remove]');
      const reverse = event.target.closest('[data-relation-reverse]');
      const clear = event.target.closest('[data-wait-clear]');
      if (edit) {
        const id = Number(edit.dataset.commentEdit), row = edit.closest('[data-comment-id]'), comment = root._comments.find(item => item.id === id);
        if (!comment || row.querySelector('textarea')) return;
        root._commentEdits.set(id,{content:comment.content,selectionStart:comment.content.length,selectionEnd:comment.content.length,uploading:false});
        await refreshComments(root,root._collabGeneration);const field=commentTextarea(id);field?.focus();field?.setSelectionRange(field.value.length,field.value.length);
      }
      const save = event.target.closest('[data-comment-save]');
      if (save) {
        const row = save.closest('[data-comment-id]'), field = row.querySelector('textarea'), content = field.value.trim();
        if (content&&!root._commentUploads.get(String(save.dataset.commentSave))?.pending) {const action={type:'mutate_panel',panel:'comments',operation:'edit_comment',comment_id:Number(save.dataset.commentSave),content};trackAction(action,null,field.value);send(root,action);}
      }
      const cancelEdit=event.target.closest('[data-comment-cancel]');if(cancelEdit){const id=String(cancelEdit.dataset.commentCancel);cancelCommentUploads(id);root._commentEdits.delete(Number(id));void refreshComments(root);}
      if (remove) {
        const id = Number(remove.dataset.commentDelete);
        const comment=root._comments.find(item=>Number(item.id)===id), refs=referenceSummary(comment?.content||''), impact=[];if(refs.issues)impact.push(`${refs.issues} issue reference${refs.issues===1?'':'s'}`);if(refs.attachments)impact.push(`${refs.attachments} attachment reference${refs.attachments===1?'':'s'}`);if(refs.mentions)impact.push(`${refs.mentions} mention${refs.mentions===1?'':'s'}`);
        const warning=impact.length?` This comment contains ${impact.join(', ')}.`:'';
        if (globalThis.confirm(`Delete this comment?${warning} Its references will no longer be visible.`)) {const action={type:'mutate_panel',panel:'comments',operation:'delete_comment',comment_id:id};trackAction(action);send(root,action);}
      }
      if (unlink) {const action={type:'mutate_panel',panel:'relations',operation:'unlink_relation',source:root.dataset.identifier,target:unlink.dataset.relationRemove};trackAction(action);send(root,action);}
      if (reverse) {const action={type:'mutate_panel',panel:'relations',operation:'reverse_relation',source:reverse.dataset.relationReverseSource,target:reverse.dataset.relationReverseTarget};trackAction(action);send(root,action);}
      if (clear) {const action={type:'mutate_panel',panel:'waits',operation:'clear_wait',wait_id:Number(clear.dataset.waitClear)};trackAction(action);send(root,action);}
      if (event.target.closest('[data-issue-delete]') && globalThis.confirm('Delete this issue?')) send(root,{type:'delete'});
      if (event.target.closest('[data-issue-restore]')) send(root,{type:'restore'});
    }
    function onInput(event) {
      const editField=event.target.closest('[data-comment-edit-draft]');if(editField){const id=Number(editField.dataset.commentEditDraft),state=root._commentEdits.get(id)||{};Object.assign(state,{content:editField.value,selectionStart:editField.selectionStart,selectionEnd:editField.selectionEnd,focused:document.activeElement===editField});root._commentEdits.set(id,state);return;}
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
      if (event.detail.panel === 'comments') void refreshComments(root,root._collabGeneration).catch(error=>{if(root.isConnected)say(`Saved, but comments could not refresh: ${error.message}`);});
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
      generation++;audienceGeneration++;root._collabGeneration++;
      for(const id of [...root._commentUploads.keys()])cancelCommentUploads(id);
      for(const state of root._commentEdits.values())state.uploading=false;
      root._attachmentMount?.dispose?.();root._attachmentMount=null;
      for(const src of root._attachmentObjectUrls||[])URL.revokeObjectURL(src);root._attachmentObjectUrls?.clear?.();
      root.dataset.publicScope=String(api().state.publicProject!==null);
      root.querySelector('[data-comment-thread]').innerHTML=''; root.querySelector('[data-comments-older]').hidden=true;root.querySelector('[data-issue-history]').innerHTML=''; root.querySelector('[data-issue-attachments]').replaceChildren(); root._comments=[];root._commentsWindowExpanded=false; candidates=[];
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
    function onFileChange(event){const input=event.target.closest('[data-comment-files]');if(input&&input.files?.length){void uploadCommentFiles(input.dataset.commentFiles,input.files);input.value='';}}
    function onHashChange(){const hash=globalThis.location?.hash||'';if(root._commentLastHash!==undefined&&root._commentLastHash!==hash)root._commentTargetDone=null;root._commentLastHash=hash;void resolveCommentHash(root,root._collabGeneration);}
    root.addEventListener('submit',onSubmit); root.addEventListener('click',onClick); root.addEventListener('input',onInput); root.addEventListener('click',onMention);
    root.addEventListener('change',onWaitKind);root.addEventListener('change',onFileChange);root.addEventListener('keydown',onKeydown); window.addEventListener('lific:issue-detail-applied',onApplied); window.addEventListener('lific:issue-detail-conflict',onConflict); window.addEventListener('lific:account-change',onScope); window.addEventListener('lific:scope-change',onScope);
    window.addEventListener('lific:issue-detail-error',onError);window.addEventListener('hashchange',onHashChange);
    function setCapabilities(capabilities={}) {
      const comment=capabilities.comment===true,edit=capabilities.edit===true;
      if(!comment&&root.dataset.commentEnabled==='true'){
        for(const id of [...root._commentUploads.keys()])cancelCommentUploads(id);
        root._commentUploads.clear();
      }
      root.dataset.commentEnabled=String(comment);root.dataset.editEnabled=String(edit);
      if(composer)composer.hidden=!comment;if(relationForm)relationForm.hidden=!edit;if(waitForm)waitForm.hidden=!edit;if(deleteButton)deleteButton.hidden=!edit;
      renderRelations(root);renderWaits(root,JSON.parse(root.dataset.waits||'[]'));
      renderCommentList(root,root._comments||[],comment);root._syncCommentComposers();
    }
    root._issueCollaboration={
      refresh:()=>refresh(root),
      update(issue,capabilities) {
        if(!issue)return;
        root._collabGeneration++;
        root.dataset.issueId=String(issue.id);root.dataset.projectId=String(issue.project_id);root.dataset.identifier=String(issue.identifier||'');
        root.dataset.waits=JSON.stringify(issue.waits||[]);
        if(capabilities)setCapabilities(capabilities);
        publishIssuePanels(issue);
      },
      setCapabilities,
      dispose(){disposed=true;generation++;audienceGeneration++;root._collabGeneration++;for(const id of [...root._commentUploads.keys()])cancelCommentUploads(id);root._commentUploads.clear();for(const src of root._attachmentObjectUrls||[])URL.revokeObjectURL(src);root._attachmentObjectUrls?.clear?.();root.removeEventListener('submit',onSubmit);root.removeEventListener('click',onClick);root.removeEventListener('input',onInput);root.removeEventListener('change',onWaitKind);root.removeEventListener('change',onFileChange);root.removeEventListener('keydown',onKeydown);window.removeEventListener('lific:issue-detail-applied',onApplied);window.removeEventListener('lific:issue-detail-conflict',onConflict);window.removeEventListener('lific:issue-detail-error',onError);window.removeEventListener('hashchange',onHashChange);window.removeEventListener('lific:account-change',onScope);window.removeEventListener('lific:scope-change',onScope);root._attachmentMount?.dispose?.();root._attachmentMount=null;root.querySelector('[data-issue-attachments]')?.replaceChildren();}
    };
    root._syncCommentComposers();
    void refresh(root);
    return root._issueCollaboration;
  }
  globalThis.LificTopcoatIssueCollaboration={mount,commentMarkup,decorateCommentReferences,renderRelations,renderWaits,renderActivity};
  if (typeof document !== 'undefined') for (const root of document.querySelectorAll('[data-topcoat-collaboration]')) if (Number(root.dataset.issueId)) mount(root);
})();

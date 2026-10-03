/* Browser route coordinator for one issue-detail activation. */
(() => {
  'use strict';

  const api = () => globalThis.lificSession;
  const ok = (result, label) => {
    if (!result?.ok) {const error = new Error(result?.error || `Could not ${label}.`); error.status = result?.status; error.current = result?.current; throw error;}
    return result.data;
  };
  const editable = role => !!role && (role.is_admin || !role.enforced || role.role === 'lead' || role.role === 'maintainer');
  const routeMatches = (left, right) => !!left && !!right && Number(left.issue_id) === Number(right.issue_id) && Number(left.generation) === Number(right.generation);
  const read = (storage, key) => {try {return storage?.getItem(key) || '';} catch {return '';}};
  const localStorage = () => {try {return globalThis.localStorage;} catch {return null;}};

  function scalarPatch(action) {
    switch (action?.field) {
      case 'title': return {title:String(action.value ?? '').trim()};
      case 'status': return {status:String(action.value)};
      case 'priority': return {priority:String(action.value)};
      case 'module_id': return {module_id:action.value == null ? null : Number(action.value)};
      case 'labels': return {labels:Array.isArray(action.value) ? action.value : []};
      default: return null;
    }
  }

  class IssueDetailController {
    constructor(root, env = {}) {
      this.root = root; this.env = env; this.session = env.session || api();
      this.projectIdentifier = root.dataset.projectIdentifier || '';
      this.identifier = root.dataset.issueIdentifier || '';
      this.publicScope = root.dataset.issueScope === 'public';
      this.generation = 0; this.issue = null; this.route = null; this.fields = null;
      this.editor = null; this.collaboration = null; this.saveChain = Promise.resolve();
      this.refreshTimer = null; this.refreshing = false; this.refreshAgain = false;
      this.writesPending = 0;
      this.capabilities = {edit:false, comment:!this.publicScope}; this.disposed = false;
      this.loading = root.querySelector('[data-detail-loading]');
      this.errorNode = root.querySelector('[data-detail-error]');
      this.content = root.querySelector('[data-detail-content]');
      this.onIntent = event => {void this.accept(event.detail).catch(()=>{});};
      this.audienceIdentity = this.identity();
      this.onAccount = () => {
        if (this.session?.state?.loading) return;
        const identity = this.identity();
        if (identity !== this.audienceIdentity) {this.audienceIdentity = identity; void this.load();}
        else this.scheduleRefresh(true);
      };
      this.onScope = () => {if (this.session?.state?.publicProject !== (this.publicScope ? this.projectIdentifier : null)) void this.load();};
      this.onRealtime = event => this.handleRealtime(event.detail);
      this.onFocus = () => this.scheduleRefresh();
      this.onVisibility = () => {if (!document.hidden) this.scheduleRefresh();};
      globalThis.addEventListener('lific:issue-detail-intent', this.onIntent);
      globalThis.addEventListener('lific:account-change', this.onAccount);
      globalThis.addEventListener('lific:scope-change', this.onScope);
      globalThis.addEventListener('lific:session-change', this.onAccount);
      globalThis.addEventListener('lific:realtime', this.onRealtime);
      globalThis.addEventListener('focus', this.onFocus);
      if (typeof document !== 'undefined') document.addEventListener('visibilitychange', this.onVisibility);
      if (!this.session?.state?.loading) void this.load();
    }
    current(generation) {return !this.disposed && generation === this.generation;}
    identity() {
      const state = this.session?.state || {};
      return `${state.publicProject ?? ''}:${state.user?.id ?? ''}:${read(localStorage(),'lific_token')}`;
    }
    async request(path, options) {return ok(await this.session.request(path, options), path);}
    handleRealtime(event) {
      if (!this.issue || !event || typeof event.type !== 'string') return;
      if (event.type === 'resync.required') {this.scheduleRefresh(true); return;}
      if (event.type === 'sync_required' && Number(event.project_id) === Number(this.issue.project_id)) {this.scheduleRefresh(true); return;}
      if (event.issue_id != null && Number(event.issue_id) === Number(this.issue.id)) {this.scheduleRefresh(); return;}
      if (/^project\./.test(event.type) && Number(event.project_id) === Number(this.issue.project_id)) this.scheduleRefresh();
    }
    scheduleRefresh(immediate = false) {
      if (!this.issue || this.disposed) return;
      if (this.refreshTimer !== null) clearTimeout(this.refreshTimer);
      this.refreshTimer = setTimeout(() => {this.refreshTimer = null; void this.refreshIssue();}, immediate ? 0 : 50);
    }
    hasLocalWork() {
      const active = typeof document !== 'undefined' ? document.activeElement : null;
      const editing = active && this.root.contains(active) && active.matches('input,textarea,select,[contenteditable="true"]');
      return this.writesPending > 0 || this.editor?.queue?.state?.().dirty || editing;
    }
    async refreshIssue() {
      if (!this.issue || this.disposed) return;
      if (this.refreshing) {this.refreshAgain = true; return;}
      this.refreshing = true; this.refreshAgain = false;
      const generation = this.generation;
      try {
        const role = this.publicScope ? null : await this.request(`/projects/${this.issue.project_id}/my-role`);
        if (!this.current(generation)) return;
        Object.assign(this.capabilities,{edit:!this.publicScope && editable(role),comment:!this.publicScope});
        this.fields?.setCapabilities?.(this.capabilities);
        this.editor?.update?.({route:this.route,capabilities:this.capabilities});
        this.collaboration?.setCapabilities?.(this.capabilities);
        if (this.hasLocalWork()) {
          this.refreshAgain = true;
          this.refreshTimer = setTimeout(() => {this.refreshTimer = null; void this.refreshIssue();},2000);
          return;
        }
        const issue = await this.request(`/issues/resolve/${encodeURIComponent(this.identifier)}`);
        if (!this.current(generation) || Number(issue?.id) !== Number(this.issue?.id)) return;
        if (this.hasLocalWork()) {
          this.refreshAgain = true;
          this.refreshTimer = setTimeout(() => {this.refreshTimer = null; void this.refreshIssue();},2000);
          return;
        }
        this.publishIssue(issue);
        this.collaboration?.refresh?.();
      } catch (_) {
        if (_.status === 401 || _.status === 403) {
          Object.assign(this.capabilities,{edit:false,comment:false});
          this.fields?.setCapabilities?.(this.capabilities);
          this.editor?.update?.({route:this.route,capabilities:this.capabilities});
          this.collaboration?.setCapabilities?.(this.capabilities);
        }
        // Keep the current issue and draft visible while the connection recovers.
      } finally {
        this.refreshing = false;
        if (this.refreshAgain && this.refreshTimer === null) {this.refreshAgain = false; this.scheduleRefresh(true);}
      }
    }
    async load() {
      const generation = ++this.generation;
      this.disposeChildren(); this.issue = null; this.route = null; this.content.hidden = true;
      this.errorNode.hidden = true; this.loading.hidden = false; this.root.setAttribute('aria-busy', 'true');
      try {
        const issue = await this.request(`/issues/resolve/${encodeURIComponent(this.identifier)}`);
        if (!this.current(generation)) return;
        if (!issue || !Number.isSafeInteger(Number(issue.id)) || !Number.isSafeInteger(Number(issue.project_id))) throw new Error('The issue response is incomplete.');
        if (String(issue.identifier || '').split('-')[0].toLowerCase() !== this.projectIdentifier.toLowerCase()) throw new Error('This issue does not belong to the selected project.');
        this.issue = issue;
        this.route = {issue_id:Number(issue.id),generation};
        const roleTask = this.publicScope
          ? Promise.resolve({role:null,enforced:true,is_admin:false})
          : this.request(`/projects/${issue.project_id}/my-role`);
        const [role, modules, labels] = await Promise.all([
          roleTask,
          this.publicScope ? Promise.resolve([]) : this.request(`/modules?project_id=${issue.project_id}`),
          this.publicScope ? Promise.resolve([]) : this.request(`/labels?project_id=${issue.project_id}`),
        ]);
        if (!this.current(generation)) return;
        this.capabilities = {edit:!this.publicScope && editable(role),comment:!this.publicScope};
        this.render(issue, modules, labels);
      } catch (error) {
        if (!this.current(generation)) return;
        this.loading.hidden = true; this.content.hidden = true; this.root.setAttribute('aria-busy', 'false');
        this.errorNode.textContent = error?.message || 'Could not load this issue.'; this.errorNode.hidden = false;
      }
    }
    render(issue, modules, labels) {
      const identifier = this.root.querySelector('[data-detail-identifier]');
      const title = this.root.querySelector('[data-detail-title]');
      const back = this.root.querySelector('[data-detail-back]');
      if (identifier) identifier.textContent = issue.identifier || this.identifier;
      if (title) title.textContent = issue.title || '';
      if (back) {
        const layout = read(localStorage(), `lific:list:layout:${this.projectIdentifier}`);
        back.href = layout === 'board' ? `/${encodeURIComponent(this.projectIdentifier)}/board` : `/${encodeURIComponent(this.projectIdentifier)}/issues`;
        if (this.publicScope) back.href = back.href.replace(/^\//, '/public/');
      }
      const fieldsRoot = this.root.querySelector('[data-issue-fields]');
      if (fieldsRoot && globalThis.LificTopcoatIssueFields) {
        this.fields = globalThis.LificTopcoatIssueFields.mount(fieldsRoot,{issue,modules,labels,capabilities:this.capabilities,
          onIntent:action => this.accept({route:this.route,action:{...action,type:'set_scalar'}})});
      }
      const editorRoot = this.root.querySelector('[data-topcoat-issue-editor]');
      if (editorRoot && globalThis.lificIssueEditor) {
        this.editor = globalThis.lificIssueEditor.mount(editorRoot,{route:this.route,text:issue.description || '',saved_description:issue.description || '',
          dirty:false,expected_seq:Number(issue.seq || 0),capabilities:this.capabilities});
      }
      const collabRoot = this.root.querySelector('[data-topcoat-collaboration]');
      if (collabRoot && globalThis.LificTopcoatIssueCollaboration) {
        this.collaboration = globalThis.LificTopcoatIssueCollaboration.mount(collabRoot,{route:this.route,issue,capabilities:this.capabilities});
      }
      this.loading.hidden = true; this.errorNode.hidden = true; this.content.hidden = false; this.root.setAttribute('aria-busy', 'false');
      this.root.dispatchEvent(new CustomEvent('lific:issue-detail-loaded',{bubbles:true,detail:{route:this.route,issue,capabilities:this.capabilities}}));
    }
    disposeChildren() {
      this.fields?.dispose?.(); this.editor?.dispose?.(); this.collaboration?.dispose?.();
      this.fields = this.editor = this.collaboration = null;
    }
    async accept(intent) {
      if (!intent || !routeMatches(intent.route,this.route) || this.disposed) return {status:'stale'};
      const action = intent.action || {};
      if (action.type === 'edit_description') {
        if (this.capabilities.edit) this.editor?.update({route:this.route,text:String(action.description ?? ''),saved_description:this.issue.description,
          expected_seq:this.issue.seq,capabilities:this.capabilities});
        return {status:'applied'};
      }
      if (action.type === 'mutate_panel') return this.queueWrite(intent,async () => {
        const id = this.issue.id, operation = action.operation;
        const send = (path, method, body) => this.request(path,{method,body:body === undefined ? undefined : JSON.stringify(body)});
        if (operation === 'create_comment') await send(`/issues/${id}/comments`,'POST',{content:action.content});
        else if (operation === 'edit_comment') await send(`/comments/${action.comment_id}`,'PUT',{content:action.content});
        else if (operation === 'delete_comment') await send(`/comments/${action.comment_id}`,'DELETE');
        else if (operation === 'link_relation') await send('/issues/link','POST',{source:action.source,target:action.target,relation_type:action.kind});
        else if (operation === 'unlink_relation') await send('/issues/unlink','POST',{source:action.source,target:action.target});
        else if (operation === 'reverse_relation') await send('/issues/reverse','POST',{source:action.source,target:action.target});
        else if (operation === 'add_wait') {
          const input = action.input || {};
          await send(`/issues/${id}/waits`,'POST',Object.hasOwn(input,'user')
            ? {user:input.user,note:input.note || ''} : {from:input.from,until:input.until,note:input.note || ''});
        } else if (operation === 'clear_wait') await send(`/issues/${id}/waits/${action.wait_id}`,'DELETE');
        else throw new Error('Unsupported issue panel action.');
        return this.request(`/issues/${id}`);
      },action.panel,action);
      if (action.type === 'delete') return this.deleteIssue(intent);
      if (action.type === 'restore') return this.restoreIssue(intent);
      const patch = action.type === 'set_scalar' ? scalarPatch(action) : action.type === 'save_description' ? {description:String(action.description ?? '')} : null;
      if (!patch) return {status:'denied'};
      if (!this.capabilities.edit) {
        if (action.type === 'save_description') this.emit('lific:issue-detail-error',{route:this.route,error:'You no longer have permission to edit this issue.',edit_revision:action.edit_revision,action});
        return {status:'denied'};
      }
      if (Object.keys(patch).every(key => JSON.stringify(this.issue[key] ?? null) === JSON.stringify(patch[key] ?? null))) return {status:'unchanged',issue:this.issue};
      return this.queueWrite(intent,async () => this.request(`/issues/${this.issue.id}`,{method:'PUT',body:JSON.stringify({...patch,expected_seq:Number(this.issue.seq || 0)})}),null,action);
    }
    async queueWrite(intent, operation, panel = null, action = null) {
      if (!this.capabilities.edit && panel !== 'comments') return {status:'denied'};
      const dispatchGeneration = this.generation;
      const {route} = intent;
      const descriptionAtEnqueue = this.issue?.description;
      const write = async () => {
        if (!this.current(dispatchGeneration) || !routeMatches(route,this.route)) return {status:'stale'};
        const editRevision = action?.edit_revision;
        if (action?.type === 'save_description' && descriptionAtEnqueue !== this.issue?.description
          && String(action.description ?? '') !== String(this.issue?.description ?? '')) {
          this.emit('lific:issue-detail-conflict',{route:this.route,current:this.issue,current_description:this.issue.description,
            expected_seq:this.issue.seq,edit_revision:editRevision,action});
          return {status:'conflict',issue:this.issue};
        }
        try {
          let updated = await operation();
          if (panel) updated = await this.request(`/issues/${this.issue.id}`);
          if (!this.current(dispatchGeneration) || !routeMatches(route,this.route)) return {status:'stale'};
          this.publishIssue(updated);
          if (panel) {
            this.emit('lific:issue-detail-applied',{route:this.route,panel,issue:updated});
          } else if (action?.type === 'save_description') {
            this.emit('lific:issue-detail-applied',{route:this.route,kind:'editor',issue:updated,description:updated.description,
              expected_seq:updated.seq,edit_revision:editRevision});
          } else {
            this.emit('lific:issue-detail-applied',{route:this.route,kind:'scalar',issue:updated});
          }
          return {status:'applied',issue:updated};
        } catch (error) {
          if (!this.current(dispatchGeneration)) return {status:'stale'};
          const current = error.current;
          if (error.status === 409 && current && typeof current === 'object') {
            this.publishIssue(current);
            this.emit('lific:issue-detail-conflict',{route:this.route,current,current_description:current.description,
              expected_seq:current.seq,edit_revision:editRevision,panel,operation:action?.operation,action});
            return {status:'conflict',issue:current};
          }
          this.emit('lific:issue-detail-error',{route:this.route,error:error.message,edit_revision:editRevision,panel,operation:action?.operation,action});
          throw error;
        }
      };
      const result = this.saveChain.then(write,write);
      this.writesPending++;
      result.finally(() => {this.writesPending = Math.max(0,this.writesPending - 1);}).catch(()=>{});
      this.saveChain = result.then(()=>undefined,()=>undefined);
      return result;
    }
    async deleteIssue(intent) {
      if (!this.capabilities.edit) return {status:'denied'};
      const current = this.issue;
      const route = this.route, dispatchGeneration = this.generation;
      const queuedWork = this.saveChain.then(async () => {
        if (!this.current(dispatchGeneration) || !routeMatches(intent.route,this.route)) return {status:'stale'};
        const destination = this.root.querySelector('[data-detail-back]')?.href || `/${this.projectIdentifier}/issues`;
        const list = globalThis.LificTopcoatIssueList;
        if (list?.queueDeletion) {
          const sessionStorage = (() => {try {return globalThis.sessionStorage;} catch {return null;}})();
          const identity = () => {
            try {return `${this.session.state.publicProject ?? ''}:${globalThis.localStorage.getItem('lific_token') ?? ''}`;}
            catch {return `${this.session.state.publicProject ?? ''}:`;}
          };
          const queued = await list.queueDeletion([{id:current.id,project_id:current.project_id,identifier:current.identifier}],{sessionStorage,identity});
          if (queued.queued) {globalThis.location.assign(destination);return {status:'queued'};}
        }
        await this.request(`/issues/${current.id}`,{method:'DELETE'});
        if (!this.current(dispatchGeneration) || !routeMatches(intent.route,this.route)) return {status:'stale'};
        this.emit('lific:issue-detail-applied',{route,kind:'deleted',deleted:true});
        globalThis.location.assign(destination);
        return {status:'applied'};
      });
      this.saveChain = queuedWork.then(()=>undefined,()=>undefined);
      return queuedWork;
    }
    async restoreIssue(intent) {
      if (!this.capabilities.edit) return {status:'denied'};
      const route = this.route, dispatchGeneration = this.generation;
      const queuedWork = this.saveChain.then(async () => {
        if (!this.current(dispatchGeneration) || !routeMatches(intent.route,this.route)) return {status:'stale'};
        const issue = await this.request(`/issues/${this.issue.id}/restore`,{method:'POST',body:'{}'});
        if (!this.current(dispatchGeneration) || !routeMatches(intent.route,this.route)) return {status:'stale'};
        this.publishIssue(issue);
        this.emit('lific:issue-detail-applied',{route,kind:'restored',restored:true,issue});
        return {status:'applied',issue};
      });
      this.saveChain = queuedWork.then(()=>undefined,()=>undefined);
      return queuedWork;
    }
    publishIssue(issue) {
      if (Number(issue?.seq ?? 0) < Number(this.issue?.seq ?? 0)) return;
      this.issue = issue;
      const title = this.root.querySelector('[data-detail-title]'); if (title) title.textContent = issue.title || '';
        this.fields?.update?.(issue,this.capabilities);
      const editorState = this.editor?.queue?.state?.();
      this.editor?.update?.({route:this.route,text:editorState?.dirty ? editorState.text : issue.description,
        saved_description:issue.description,expected_seq:Number(issue.seq || 0),capabilities:this.capabilities});
      this.collaboration?.update?.(issue,this.capabilities);
    }
    emit(name, detail) {globalThis.dispatchEvent(new CustomEvent(name,{detail}));}
    dispose() {
      this.disposed = true; this.generation++; this.disposeChildren();
      if (this.refreshTimer !== null) clearTimeout(this.refreshTimer);
      globalThis.removeEventListener('lific:issue-detail-intent',this.onIntent);
      globalThis.removeEventListener('lific:account-change',this.onAccount);
      globalThis.removeEventListener('lific:scope-change',this.onScope);
      globalThis.removeEventListener('lific:session-change',this.onAccount);
      globalThis.removeEventListener('lific:realtime',this.onRealtime);
      globalThis.removeEventListener('focus',this.onFocus);
      if (typeof document !== 'undefined') document.removeEventListener('visibilitychange',this.onVisibility);
    }
  }

  function mount(root, env) {return root ? new IssueDetailController(root,env) : null;}
  const apiForTests = {IssueDetailController,scalarPatch,editable,routeMatches,mount};
  if (typeof module !== 'undefined') module.exports = apiForTests;
  globalThis.LificTopcoatIssueDetail = apiForTests;
  if (typeof document !== 'undefined') {
    const root = document.querySelector('[data-topcoat-issue-detail]');
    if (root) globalThis.lificIssueDetail = mount(root);
  }
})();

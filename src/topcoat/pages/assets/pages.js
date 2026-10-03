(() => {
  'use strict';

  const STATUS = new Set(['draft', 'active', 'complete', 'archived']);
  const el = (doc, tag, text, className) => {
    const node = doc.createElement(tag);
    if (text !== undefined) node.textContent = text;
    if (className) node.className = className;
    return node;
  };
  const escapeHtml = value => String(value ?? '').replace(/[&<>"']/g, char => ({'&':'&amp;','<':'&lt;','>':'&gt;','"':'&quot;',"'":'&#39;'}[char]));
  const inline = text => {
    const mentions = []; const codes = [];
    let protectedText = String(text).replace(/`([^`]+)`/g, (_match, code) => {
      const marker = `\u0000PAGE_CODE_${codes.length}\u0000`; codes.push(code); return marker;
    });
    let value = escapeHtml(protectedText).replace(/(^|[\s([{])@([\p{L}\p{N}_.-]+)/gu, (_match, prefix, username) => {
      const marker = `\u0000PAGE_MENTION_${mentions.length}\u0000`; mentions.push(username); return `${prefix}${marker}`;
    });
    value = value.replace(/!\[([^\]]*)\]\((\/api\/attachments\/(\d+)|\/attachments\/(\d+))\)/g,
      (_m, alt, _url, privateId, scopedId) => `<img data-page-attachment="${privateId || scopedId}" alt="${alt}" loading="lazy">`)
      .replace(/\*\*([^*]+)\*\*/g, '<strong>$1</strong>')
      .replace(/\*([^*]+)\*/g, '<em>$1</em>')
      .replace(/\[([^\]]+)\]\((https?:\/\/[^\s)]+|\/[^\s)]*)\)/g, (_m, label, url) => {
        const attachment = url.match(/^\/(?:api\/)?attachments\/(\d+)(?:\/(?:preview|thumbnail))?$/);
        if (attachment) return `<a href="#attachment-${attachment[1]}" data-page-attachment-link="${attachment[1]}">${label}</a>`;
        const safe = url.startsWith('/') || /^https?:\/\//i.test(url);
        return safe ? `<a href="${url}" rel="nofollow noopener">${label}</a>` : label;
      })
      ;
    mentions.forEach((username, index) => {
      value = value.replace(`\u0000PAGE_MENTION_${index}\u0000`, `<span class="tc-page-mention" data-page-mention="${escapeHtml(username)}">@${escapeHtml(username)}</span>`);
    });
    codes.forEach((code, index) => {value = value.replace(`\u0000PAGE_CODE_${index}\u0000`, `<code>${escapeHtml(code)}</code>`);});
    return value;
  };
  function markdown(source) {
    const normalized = String(source ?? '').replace(/\r\n?/g, '\n');
    const blocks = [];
    let block = [];
    let fence = null;
    for (const line of normalized.split('\n')) {
      const marker = line.match(/^\s*(```+|~~~+)/)?.[1];
      if (marker && (!fence || marker[0] === fence[0])) fence = fence ? null : marker;
      if (!fence && !marker && !line.trim()) {
        if (block.length) { blocks.push(block); block = []; }
      } else block.push(line);
    }
    if (block.length) blocks.push(block);
    return blocks.map(lines => {
      const block = lines.join('\n');
      if (/^\s*(```|~~~)/.test(block)) {
        const code = block.replace(/^\s*(?:```|~~~)[^\n]*\n?/, '').replace(/\n?\s*(?:```|~~~)\s*$/, '');
        return `<pre><code>${escapeHtml(code)}</code></pre>`;
      }
      const heading = block.match(/^(#{1,6})\s+(.+)$/);
      if (heading) return `<h${heading[1].length}>${inline(heading[2])}</h${heading[1].length}>`;
      const rows = block.split('\n');
      if (rows.length > 1 && /^\|?\s*:?-{3,}/.test(rows[1])) {
        const cells = row => row.trim().replace(/^\||\|$/g, '').split('|').map(cell => cell.trim());
        const headers = cells(rows[0]);
        if (headers.length && cells(rows[1]).length === headers.length) {
          const body = rows.slice(2).filter(Boolean).map(row => cells(row));
          return `<table><thead><tr>${headers.map(cell => `<th>${inline(cell)}</th>`).join('')}</tr></thead><tbody>${body.map(row => `<tr>${headers.map((_cell, index) => `<td>${inline(row[index] || '')}</td>`).join('')}</tr>`).join('')}</tbody></table>`;
        }
      }
      const renderItems = (items, type) => `<${type}>${items.map(item => `<li>${item.text}${item.children.map(group => renderItems(group.items, group.type)).join('')}</li>`).join('')}</${type}>`;
      const output = [];
      let list = null;
      const flush = () => { if (list) { output.push(renderItems(list.items, list.type)); list = null; } };
      for (const line of lines) {
        const match = line.match(/^(\s*)(?:([-*+])|(\d+)\.)\s+(.*)$/);
        if (!match) { flush(); if (line.trim()) output.push(`<p>${inline(line)}</p>`); continue; }
        const depth = Math.floor(match[1].length / 2);
        const type = match[3] ? 'ol' : 'ul';
        const task = match[4].match(/^\[([ xX])\]\s+(.*)$/);
        const text = task ? `<input type="checkbox" disabled${task[1].toLowerCase() === 'x' ? ' checked' : ''} aria-label="${task[1].toLowerCase() === 'x' ? 'Complete' : 'Incomplete'}"> ${inline(task[2])}` : inline(match[4]);
        if (!list) list = {type, items:[]};
        if (depth === 0 && list.type !== type) { flush(); list = {type, items:[]}; }
        let level = list;
        for (let i = 0; i < depth; i++) {
          const parent = level.items.at(-1);
          if (!parent) break;
          let child = parent.children.at(-1);
          if (!child || child.type !== type) { child = {type, items:[]}; parent.children.push(child); }
          level = child;
        }
        level.items.push({text,children:[]});
      }
      flush();
      return output.join('');
    }).join('');
  }
  const byId = (root, selector) => root.querySelector(selector);
  const requestData = async (session, path, options) => {
    const response = await session.request(path, options);
    if (!response?.ok) throw new Error(response?.error || 'Request failed');
    return response.data;
  };
  const identity = session => session.state.publicProject === null && session.state.user
    ? `private:${session.state.user.id}` : session.state.publicProject ? `public:${session.state.publicProject}` : null;

  class PagesController {
    constructor(root, {window: win = globalThis.window, session = win.lificSession} = {}) {
      this.root = root; this.win = win; this.doc = root.ownerDocument; this.session = session;
      this.generation = 0; this.disposed = false; this.timer = null; this.page = null;
      this.commentDraftVersion = 0;
      this.commentEditorGeneration = 0; this.activeCommentEditor = null;
      this.editing = false; this.commentEditing = false; this.comments = []; this.commentCursor = null; this.hasOlderComments = false;
      this.peekGeneration = 0; this.previewGeneration = 0; this.commentPaginationError = null;
      this.folders = []; this.pages = []; this.activity = []; this.mentionCandidates = []; this.activeMentionIndex = 0;
      this.activeTab = 'browse';
      this.audience = identity(session); this.canEdit = false; this.canComment = false;
      this.objectUrls = new Set(); this.uploads = new Set(); this.composers = new Map(); this.attachments = []; this.commentAttachments = new Map();
      this.attachmentClient = win.LificTopcoatAttachments?.createClient({session, win});
      this.public = root.dataset.pageScope === 'public';
      this.projectName = root.dataset.projectIdentifier;
      this.projectId = null;
      this.listeners = [];
      const listen = (target, type, fn) => { target.addEventListener(type, fn); this.listeners.push(() => target.removeEventListener(type, fn)); };
      if (root.dataset.topcoatPages === 'list') this.bindList(listen);
      else this.bindDetail(listen);
      for (const name of ['lific:session-change', 'lific:account-change', 'lific:scope-change']) listen(win, name, () => this.transition());
      listen(win, 'lific:realtime', event => this.realtime(event.detail));
      listen(win, 'focus', () => { if (!this.editing && !this.commentEditing) void this.load(); });
      listen(win, 'pagehide', () => this.dispose());
      void this.load();
    }
    current(generation) { return !this.disposed && generation === this.generation && this.audience === identity(this.session); }
    transition() {
      const next = identity(this.session);
      if (next === this.audience) return;
      this.composers.forEach(composer=>composer.dispose()); this.composers.clear();
      this.audience = next; this.generation++; this.win.clearTimeout(this.timer); this.page = null; this.pages = [];
      this.root.querySelectorAll('[data-page-mention-menu]').forEach(menu => menu.remove());
      this.root.setAttribute('aria-busy', 'true');
      if (this.root.dataset.topcoatPages === 'detail') {
        byId(this.root, '[data-page-content]').hidden = true;
        byId(this.root, '[data-page-error]').hidden = true;
        byId(this.root, '[data-page-status-message]').textContent = 'Loading page…';
      } else {
        byId(this.root, '[data-pages-content]').replaceChildren();
        const dialog=byId(this.root,'[data-pages-peek-dialog]');
        if(dialog){
          if(dialog.open)dialog.close();
          byId(this.root,'[data-pages-peek-title]').textContent='';
          byId(this.root,'[data-pages-peek-content]').replaceChildren();
          byId(this.root,'[data-pages-peek-open]').removeAttribute('href');
        }
      }
      void this.load();
    }
    async resolveProject(generation) {
      const projects = await requestData(this.session, '/projects');
      if (!this.current(generation)) return null;
      return projects.find(item => item.identifier.toLowerCase() === this.projectName.toLowerCase()) || null;
    }
    async resolvePageProject(projectId, generation) {
      const projects=await requestData(this.session,'/projects');
      if(!this.current(generation))return null;
      return projects.find(project=>project.id===projectId)||null;
    }
    async role(project) {
      if (this.public) return {edit:false, comment:false};
      if (!project) {
        const me = this.session.state.user || await requestData(this.session, '/auth/me');
        const enforced = me.enforced ?? this.session.state.role?.enforced ?? false;
        const admin = Boolean(me.is_admin);
        return {edit:admin || !enforced, comment:admin || !enforced};
      }
      const result = await requestData(this.session, `/projects/${project.id}/my-role`);
      const name = result.role;
      return {edit:Boolean(!result.enforced || result.is_admin || ['maintainer','lead','admin'].includes(name)), comment:Boolean(!result.enforced || result.is_admin || ['viewer','maintainer','lead','admin'].includes(name))};
    }
    async load() {
      if (this.disposed) return;
      const generation = ++this.generation;
      if (this.root.dataset.topcoatPages === 'list') return this.loadList(generation);
      return this.loadDetail(generation);
    }
    async loadList(generation) {
      const status = byId(this.root, '[data-pages-status]');
      const content = byId(this.root, '[data-pages-content]');
      try {
        if (this.audience === null) { status.textContent = 'Sign in to view pages.'; return; }
        status.textContent = 'Loading pages…'; this.root.setAttribute('aria-busy', 'true');
        const project = await this.resolveProject(generation);
        if (!this.current(generation)) return;
        if (!project) throw new Error(`Project ${this.projectName} not found`);
        this.projectId = project.id;
        const access = await this.role(project);
        const [pages, folders] = await Promise.all([
          this.public ? requestData(this.session, `/projects/${project.id}/index`).then(index => index.pages || []) : this.loadAllPages(project.id),
          requestData(this.session, `/folders?project_id=${project.id}`),
        ]);
        if (!this.current(generation)) return;
        this.canEdit = access.edit; this.pages = pages; this.folders = folders;
        this.renderList();
        byId(this.root, '[data-page-create]').hidden = !this.canEdit || this.public;
        byId(this.root,'[data-page-create-presets]').hidden=!this.canEdit||this.public;
        byId(this.root, '[data-pages-folder-create]').hidden = !this.canEdit || this.public;
        this.root.setAttribute('aria-busy', 'false'); status.textContent = pages.length ? `${pages.length} pages` : 'No pages yet.';
        byId(this.root, '[data-pages-error]').hidden = true;
      } catch (error) {
        if (!this.current(generation)) return;
        this.root.setAttribute('aria-busy', 'false'); status.textContent = '';
        const node = byId(this.root, '[data-pages-error]'); node.hidden = false;
        node.replaceChildren(el(this.doc, 'p', error.message), this.retryButton(() => this.load()));
      }
    }
    async loadAllPages(projectId) {
      const limit = 200;
      const pages = [];
      for (let offset = 0; ; offset += limit) {
        const batch = await requestData(this.session, `/pages?project_id=${projectId}&limit=${limit}&offset=${offset}`);
        pages.push(...batch);
        if (batch.length < limit) return pages;
      }
    }
    filteredPages() {
      const q = byId(this.root, '[data-pages-search]').value.trim().toLocaleLowerCase();
      const selectedStatus = byId(this.root, '[data-pages-status-filter]').value;
      const defaultActiveOnly = selectedStatus === '__active' && this.activeTab !== 'archived';
      const status = selectedStatus === '__active' ? '' : selectedStatus;
      const folder = byId(this.root, '[data-pages-folder]').value;
      const label = byId(this.root, '[data-pages-label-filter]')?.value || '';
      const fuzzy = value => {
        let cursor = 0;
        for (const char of q) { cursor = value.indexOf(char, cursor); if (cursor < 0) return false; cursor++; }
        return true;
      };
      let result = this.pages.filter(page => (!q || `${page.title || ''} ${page.identifier || ''} ${page.preview || ''} ${page.content || ''} ${(page.labels || []).join(' ')}`.toLocaleLowerCase().includes(q)
          || fuzzy(`${page.title || ''} ${page.identifier || ''} ${page.preview || ''} ${(page.labels || []).join(' ')}`.toLocaleLowerCase()))
        && (!defaultActiveOnly || page.status !== 'archived')
        && (!status || page.status === status)
        && (!label || (page.labels || []).some(item => (typeof item === 'string' ? item : item.name) === label))
        && (folder === '' || String(page.folder_id ?? '') === folder)
        && (this.activeTab !== 'drafts' || page.status === 'draft')
        && (this.activeTab !== 'archived' || page.status === 'archived')
        && (this.activeTab !== 'recent' || page.status !== 'archived'));
      if(this.activeTab==='recent') result=result.sort((a,b)=>String(b.updated_at||'').localeCompare(String(a.updated_at||''))).slice(0,20);
      if(q) {
        const score=page=>{const title=(page.title||'').toLowerCase();const id=(page.identifier||'').toLowerCase();const preview=(page.preview||'').toLowerCase();const labels=(page.labels||[]).map(item=>typeof item==='string'?item:item.name).join(' ').toLowerCase();
          let best=title.includes(q)?1:id.includes(q)?0.9:preview.includes(q)?0.6:labels.includes(q)?0.55:0;
          for(const candidate of [title,id,preview,labels]){let at=0;for(const ch of q){at=candidate.indexOf(ch,at);if(at<0)break;at++;}if(at>=0)best=Math.max(best,0.3+(q.length/Math.max(candidate.length,1))*0.5);}return best;};
        result=result.sort((a,b)=>score(b)-score(a)).slice(0,50);
      }
      return result;
    }
    renderList() {
      const select = byId(this.root, '[data-pages-folder]');
      const oldFolder = select.value;
      select.replaceChildren(new Option('All folders', ''));
      this.folders.forEach(folder => select.append(new Option(folder.name, String(folder.id))));
      const labelSelect = byId(this.root, '[data-pages-label-filter]');
      if (labelSelect) {
        const oldLabel = labelSelect.value;
        labelSelect.replaceChildren(new Option('All labels', ''));
        [...new Set(this.pages.flatMap(page => (page.labels || []).map(item => typeof item === 'string' ? item : item.name)).filter(Boolean))].sort().forEach(name => labelSelect.append(new Option(name, name)));
        if ([...labelSelect.options].some(option => option.value === oldLabel)) labelSelect.value = oldLabel;
      }
      if ([...select.options].some(option => option.value === oldFolder)) select.value = oldFolder;
      const createFolder = byId(this.root, '[data-pages-create-form] [name="folder_id"]');
      createFolder.replaceChildren(new Option('No folder', ''));
      this.folders.forEach(folder => createFolder.append(new Option(folder.name, String(folder.id))));
      const target = byId(this.root, '[data-pages-content]'); target.replaceChildren();
      const rows = this.filteredPages();
      if (!rows.length) { target.append(el(this.doc, 'p', this.pages.length ? 'No pages match these filters.' : 'No pages yet.')); this.renderFolderTree(); return; }
      const pinned = rows.filter(page => page.pinned);
      const ordinary = rows.filter(page => !page.pinned);
      const appendRows = pages => { const list = el(this.doc, 'ul', undefined, 'tc-pages__list'); for (const page of pages) {
        const row = el(this.doc, 'li', undefined, 'tc-pages__row');
        const link = el(this.doc, 'a', page.title || 'Untitled page'); link.href = this.detailHref(page.id);
        row.draggable = this.canEdit && !this.public;
        row.dataset.pageId = String(page.id);
        row.addEventListener('dragstart', event => event.dataTransfer?.setData('text/plain', String(page.id)));
        const folder = this.folders.find(item => item.id === page.folder_id)?.name;
        const meta = el(this.doc, 'span', `${folder ? `${folder} · ` : ''}${page.identifier || ''} · ${page.status}${page.pinned ? ' · Pinned' : ''}${page.labels?.length ? ` · ${page.labels.map(item=>typeof item==='string'?item:item.name).join(', ')}` : ''}`, 'tc-pages__meta');
        row.append(link, meta);
        if (this.canEdit && !this.public) {
          const pin = el(this.doc, 'button', page.pinned ? 'Unpin' : 'Pin', 'tc-button'); pin.type = 'button';
          pin.addEventListener('click', () => void this.updatePage(page.id, {pinned: !page.pinned}, true)); row.append(pin);
          const destination=el(this.doc,'select');destination.setAttribute('aria-label',`Move ${page.title || page.identifier} to folder`);destination.append(new Option('No folder','root'));
          this.folders.forEach(item=>destination.append(new Option(item.name,String(item.id))));destination.value=page.folder_id==null?'root':String(page.folder_id);
          destination.addEventListener('change',()=>void this.movePage(page.id,destination.value==='root'?null:Number(destination.value)));row.append(destination);
        }
        const peek=el(this.doc,'button','Peek','tc-button');peek.type='button';peek.dataset.pagePeek=String(page.id);peek.setAttribute('aria-label',`Peek ${page.title||page.identifier}`);row.append(peek);
        list.append(row);
      } return list;};
      if (pinned.length) target.append(el(this.doc,'h2','Pinned'),appendRows(pinned));
      if (ordinary.length) target.append(el(this.doc,'h2','Pages'),appendRows(ordinary));
      this.renderFolderTree();
    }
    detailHref(id) { return `${this.public ? `/public/${encodeURIComponent(this.projectName)}` : `/${encodeURIComponent(this.projectName)}`}/pages/${id}`; }
    listHref() { return `${this.public ? `/public/${encodeURIComponent(this.projectName)}` : `/${encodeURIComponent(this.projectName)}`}/pages`; }
    renderFolderTree() {
      const target=byId(this.root,'[data-pages-folder-tree]');if(!target)return;target.replaceChildren();
      const add=(parent,depth)=>{for(const folder of this.folders.filter(item=>(item.parent_id??null)===parent).sort((a,b)=>a.name.localeCompare(b.name))){
        const row=el(this.doc,'li',undefined,'tc-pages__folder');row.style.setProperty('--folder-depth',depth);
        row.addEventListener('dragover',event=>{if(this.canEdit&&!this.public)event.preventDefault();});
        row.addEventListener('drop',event=>{event.preventDefault();const id=Number(event.dataTransfer?.getData('text/plain'));if(id)void this.movePage(id,folder.id);});
        const choose=el(this.doc,'button',folder.name,'tc-button');choose.type='button';choose.addEventListener('click',()=>{byId(this.root,'[data-pages-folder]').value=String(folder.id);this.renderList();});row.append(choose);
        if(this.canEdit&&!this.public){const sub=el(this.doc,'button','Add subfolder','tc-button');sub.type='button';sub.addEventListener('click',()=>void this.createFolder(folder.id));const remove=el(this.doc,'button','Delete folder','tc-button');remove.type='button';remove.addEventListener('click',()=>void this.deleteFolder(folder));row.append(sub,remove);}
        target.append(row);add(folder.id,depth+1);
      }};add(null,0);
    }
    async createFolder(parentId=null) {
      const name=this.win.prompt(parentId===null?'Folder name':'Subfolder name')?.trim();if(!name||!this.canEdit)return;
      try{await requestData(this.session,'/folders',{method:'POST',body:JSON.stringify({project_id:this.projectId,parent_id:parentId,name})});await this.load();}catch(error){this.showError(error);}
    }
    async deleteFolder(folder) {
      if(!this.win.confirm(`Delete folder ${folder.name}?`))return;
      try{await requestData(this.session,`/folders/${folder.id}`,{method:'DELETE'});await this.load();}catch(error){this.showError(error);}
    }
    async movePage(id,folderId) { await this.updatePage(id,{folder_id:folderId},true); }
    bindList(listen) {
      listen(byId(this.root, '[data-pages-search]'), 'input', () => this.renderList());
      listen(byId(this.root, '[data-pages-status-filter]'), 'change', () => this.renderList());
      listen(byId(this.root, '[data-pages-folder]'), 'change', () => this.renderList());
      listen(byId(this.root, '[data-pages-label-filter]'), 'change', () => this.renderList());
      const tabs=byId(this.root,'[data-pages-tabs]');if(tabs)listen(tabs,'click',event=>{const button=event.target.closest?.('[data-pages-tab]');if(!button)return;this.activeTab=button.dataset.pagesTab;this.root.querySelectorAll('[data-pages-tab]').forEach(tab=>tab.removeAttribute('aria-current'));button.setAttribute('aria-current','page');this.renderList();});
      listen(byId(this.root, '[data-pages-folder-create]'), 'click', () => void this.createFolder());
      listen(byId(this.root, '[data-pages-folder-tree]'), 'dragover', event => {if(this.canEdit&&!this.public)event.preventDefault();});
      listen(byId(this.root, '[data-pages-folder-tree]'), 'drop', event => {if(event.target!==event.currentTarget)return;event.preventDefault();const id=Number(event.dataTransfer?.getData('text/plain'));if(id)void this.movePage(id,null);});
      listen(byId(this.root, '[data-page-create]'), 'click', () => {byId(this.root,'[data-pages-create-form] [name="status"]').value='draft';byId(this.root, '[data-pages-create-dialog]').showModal();});
      listen(byId(this.root,'[data-page-create-presets]'),'click',event=>{const preset=event.target.closest?.('[data-page-create-preset]');if(!preset)return;byId(this.root,'[data-pages-create-form] [name="status"]').value=preset.dataset.pageCreatePreset;byId(this.root,'[data-pages-create-dialog]').showModal();});
      listen(byId(this.root,'[data-pages-content]'),'click',event=>{const button=event.target.closest?.('[data-page-peek]');if(!button)return;const page=this.pages.find(item=>String(item.id)===button.dataset.pagePeek);if(page)this.openPagePeek(page);});
      listen(byId(this.root,'[data-pages-peek-close]'),'click',()=>byId(this.root,'[data-pages-peek-dialog]').close());
      listen(byId(this.root, '[data-pages-create-form]'), 'submit', event => {
        if (event.submitter?.value === 'cancel') return;
        event.preventDefault(); void this.createPage(new FormData(event.currentTarget));
      });
    }
    async createPage(form) {
      const title = String(form.get('title') || '').trim(); if (!title || !this.canEdit) return;
      try {
        const page = await requestData(this.session, '/pages', {method:'POST', body:JSON.stringify({project_id:this.projectId, title, status:form.get('status') || 'draft', folder_id:form.get('folder_id') ? Number(form.get('folder_id')) : null})});
        this.navigate(this.detailHref(page.id));
      } catch (error) { this.showError(error); }
    }
    async openPagePeek(page) {
      const generation=++this.peekGeneration;
      byId(this.root,'[data-pages-peek-title]').textContent=page.title||'Untitled page';
      const content=byId(this.root,'[data-pages-peek-content]');content.replaceChildren();
      const link=byId(this.root,'[data-pages-peek-open]');link.href=this.detailHref(page.id);
      byId(this.root,'[data-pages-peek-dialog]').showModal();
      if(!this.public){this.renderMarkdown(content,page.content||page.preview||'');return;}
      content.textContent='Loading page…';
      try{
        const fullPage=await requestData(this.session,`/pages/${page.id}`);
        if(generation!==this.peekGeneration||!byId(this.root,'[data-pages-peek-dialog]').open)return;
        byId(this.root,'[data-pages-peek-title]').textContent=fullPage.title||page.title||'Untitled page';
        this.renderMarkdown(content,fullPage.content||'');
      }catch(error){
        if(generation===this.peekGeneration&&byId(this.root,'[data-pages-peek-dialog]').open)content.textContent=`Could not load page: ${error.message}`;
      }
    }
    async updatePage(id, patch, reload = false) {
      try {
        await requestData(this.session, `/pages/${id}`, {method:'PUT', body:JSON.stringify(patch)});
        if (reload) await this.load();
      } catch (error) { this.showError(error); }
    }
    async loadDetail(generation) {
      const status = byId(this.root, '[data-page-status-message]');
      const content = byId(this.root, '[data-page-content]');
      this.commentPaginationError=null;
      try {
        if (this.audience === null) { status.textContent = 'Sign in to view this page.'; return; }
        status.textContent = 'Loading page…'; this.root.setAttribute('aria-busy', 'true');
        const page = await requestData(this.session, `/pages/${this.root.dataset.pageId}`);
        if (!this.current(generation)) return;
        this.page = page;
        this.projectId = page.project_id;
        const project = page.project_id === null ? null : (await this.resolvePageProject(page.project_id,generation));
        if (page.project_id !== null && !project) throw new Error('Project not found');
        if (project) {
          if(this.public&&project.identifier.toLowerCase()!==this.projectName.toLowerCase())throw new Error('Page is outside this public project.');
          this.projectName=project.identifier;
        }
        const access = await this.role(project);
        const [thread, activity, folders, labels, mentions] = await Promise.all([
          this.commentPage(page.id, null),
          requestData(this.session, `/pages/${page.id}/activity?limit=100`),
          page.project_id === null ? Promise.resolve([]) : requestData(this.session, `/folders?project_id=${page.project_id}`),
          page.project_id === null ? Promise.resolve([]) : requestData(this.session, `/labels?project_id=${page.project_id}`),
          page.project_id === null ? Promise.resolve([]) : requestData(this.session, `/projects/${page.project_id}/mention-candidates`).catch(() => []),
        ]);
        if (!this.current(generation)) return;
        this.canEdit = access.edit; this.canComment = access.comment; this.comments = thread.items; this.mentionCandidates = mentions;
        this.commentCursor = thread.cursor; this.hasOlderComments = thread.hasOlder;
        this.folders = folders; this.activity = activity.items || []; this.renderDetail();
        await this.loadAttachments(generation);
        if (!this.current(generation)) return;
        await this.loadCommentAttachments(generation);
        if (!this.current(generation)) return;
        this.wireMentionInput(byId(this.root, '[data-page-comment-form] textarea[name="content"]'));
        if(!this.public&&this.attachmentClient){if(this.canEdit)this.composer('page');if(this.canComment)this.composer('comment');}
        this.renderLabels(page.labels || [], labels);
        await this.followDeepLink(generation);
        if (!this.current(generation)) return;
        content.hidden = false; status.textContent = ''; this.root.setAttribute('aria-busy', 'false');
        const errorNode=byId(this.root,'[data-page-error]');
        if(this.commentPaginationError){
          errorNode.hidden=false;errorNode.replaceChildren(el(this.doc,'p',this.commentPaginationError.message));
          const retry=el(this.doc,'button','Retry older comments','tc-button');retry.type='button';retry.addEventListener('click',()=>void this.loadOlderComments());errorNode.append(retry);
        }
        else errorNode.hidden=true;
      } catch (error) {
        if (!this.current(generation)) return;
        status.textContent = ''; this.root.setAttribute('aria-busy', 'false');
        const node = byId(this.root, '[data-page-error]'); node.hidden = false;
        node.replaceChildren(el(this.doc, 'p', error.message), this.retryButton(() => this.load()));
      }
    }
    renderDetail({renderComments = true} = {}) {
      const page = this.page;
      const back = byId(this.root, 'nav a[href]');
      if (back) {back.href = this.listHref(); back.textContent = 'Pages';}
      const title = byId(this.root, '[data-page-title]'); title.value = page.title; title.disabled = !this.canEdit || this.public;
      const body = byId(this.root, '[data-page-body]'); body.value = page.content || ''; body.disabled = true; body.hidden = true;
      byId(this.root,'[data-page-preview-content]').hidden=false;
      byId(this.root,'[data-page-preview]').hidden=!this.editing;
      const folderControl = byId(this.root, '[data-page-folder-control]');
      folderControl.hidden = !this.canEdit || this.public || page.project_id === null;
      const folder = byId(this.root, '[data-page-folder]'); folder.replaceChildren(new Option('No folder', ''));
      this.folders.forEach(item => folder.append(new Option(item.name, String(item.id))));
      folder.value = page.folder_id == null ? '' : String(page.folder_id); folder.disabled = !this.canEdit || this.public || page.project_id === null;
      const status = byId(this.root, '[data-page-lifecycle]'); status.value = STATUS.has(page.status) ? page.status : 'draft'; status.disabled = !this.canEdit || this.public;
      const pin = byId(this.root, '[data-page-pin]'); pin.hidden = !this.canEdit || this.public || page.project_id === null;
      pin.textContent = page.pinned ? 'Unpin' : 'Pin';
      byId(this.root, '[data-page-delete]').hidden = !this.canEdit || this.public;
      byId(this.root, '[data-page-edit]').hidden = !this.canEdit || this.public;
      byId(this.root, '[data-page-comment-form]').hidden = !this.canComment || this.public;
      byId(this.root, '[data-page-attachment-upload]').hidden = !this.canEdit || this.public;
      byId(this.root, '[data-page-export]').hidden = this.public;
      byId(this.root, '[data-page-folder-crumb]').textContent = page.folder_id == null ? '' : ` / ${this.folders.find(folder => folder.id === page.folder_id)?.name || ''}`;
      this.renderMarkdown(byId(this.root, '[data-page-preview-content]'), page.content || '');
      if (renderComments) this.renderComments();
      const target = byId(this.root, '[data-page-activity]'); target.replaceChildren();
      for (const item of this.activity) target.append(el(this.doc, 'li', `${item.action || item.type || 'Updated'} · ${item.created_at || ''}`));
      byId(this.root, '[data-page-comments-older]').hidden = !this.hasOlderComments;
      byId(this.root, '[data-page-save-status]').textContent = 'Saved';
    }
    renderLabels(selected, available) {
      const target = byId(this.root, '[data-page-labels]'); target.replaceChildren();
      if (!selected.length && !this.canEdit) return;
      target.append(el(this.doc, 'span', 'Labels: '));
      if (this.canEdit && !this.public && this.page.project_id !== null) {
        const select = el(this.doc, 'select'); select.multiple = true; select.setAttribute('aria-label', 'Page labels');
        for (const label of available) { const option = new Option(label.name, label.name, false, selected.includes(label.name)); select.add(option); }
        select.addEventListener('change', () => void this.saveField({labels:[...select.selectedOptions].map(option => option.value)})); target.append(select);
        const form = el(this.doc, 'form', undefined, 'tc-page-label-create'); form.dataset.pageLabelCreate = '';
        const name = el(this.doc, 'input'); name.name = 'name'; name.required = true; name.maxLength = 64; name.placeholder = 'Create label'; name.setAttribute('aria-label', 'New label name');
        const color = el(this.doc, 'input'); color.name = 'color'; color.type = 'color'; color.value = '#64748b'; color.setAttribute('aria-label', 'New label color');
        const create = el(this.doc, 'button', 'Add label', 'tc-button'); create.type = 'submit';
        form.append(name, color, create); form.addEventListener('submit', event => {event.preventDefault(); void this.createLabel(new FormData(form));}); target.append(form);
      } else target.append(el(this.doc, 'span', selected.join(', ') || 'None'));
    }
    renderComments() {
      const target = byId(this.root, '[data-page-comment-list]'); target.replaceChildren();
      for (const comment of this.comments) {
        const row = el(this.doc, 'li'); row.id = `comment-${comment.id}`; row.dataset.commentId = String(comment.id);
        const body = el(this.doc, 'div'); this.renderMarkdown(body, comment.content || '');
        const meta = el(this.doc, 'p', `${comment.author_display_name || comment.author || 'User'} · ${comment.created_at || ''}`);
        row.append(meta, body);
        const attachments=this.commentAttachments.get(comment.id)||[];
        if(attachments.length){const list=el(this.doc,'ul');for(const attachment of attachments)list.append(this.attachmentRow(attachment,`comment-attachment-${attachment.id}`));row.append(list);}
        if (!this.public && this.session.state.user && (comment.user_id === this.session.state.user.id || this.canEdit)) {
          const edit = el(this.doc, 'button', 'Edit', 'tc-button'); edit.type = 'button'; edit.addEventListener('click', () => this.editComment(comment, row));
          const remove = el(this.doc, 'button', 'Delete', 'tc-button'); remove.type = 'button'; remove.addEventListener('click', () => void this.deleteComment(comment.id)); row.append(edit, remove);
        }
        target.append(row);
      }
    }
    editComment(comment, row) {
      const editorGeneration=++this.commentEditorGeneration;
      this.activeCommentEditor={id:comment.id,generation:editorGeneration};
      this.commentEditing = true;
      const input = el(this.doc, 'textarea'); input.value = comment.content; input.setAttribute('aria-label', 'Edit comment');
      let draftVersion=0;input.addEventListener('input',()=>draftVersion++);
      this.wireMentionInput(input);
      const save = el(this.doc, 'button', 'Save', 'tc-button'); save.type = 'button';
      const status=el(this.doc,'span');status.dataset.commentSaveStatus='';
      const cancel = el(this.doc, 'button', 'Cancel', 'tc-button'); cancel.type = 'button'; cancel.addEventListener('click', () => {
        if(this.activeCommentEditor?.generation===editorGeneration){this.commentEditorGeneration++;this.activeCommentEditor=null;this.commentEditing=false;this.renderComments();}
      });
      save.addEventListener('click', async () => {
        const submittedVersion=draftVersion;
        try { const updated = await requestData(this.session, `/comments/${comment.id}`, {method:'PUT', body:JSON.stringify({content:input.value})});
          this.comments = this.comments.map(item => item.id === comment.id ? updated : item);
          const sameEditor=this.activeCommentEditor?.generation===editorGeneration&&this.activeCommentEditor.id===comment.id&&row.contains(input);
          if(draftVersion===submittedVersion&&sameEditor){this.commentEditorGeneration++;this.activeCommentEditor=null;this.commentEditing = false; this.renderComments();}
          else if(sameEditor)status.textContent='Saved previous version. Newer changes remain unsaved.';
        } catch (error) { this.showError(error); }
      });
      row.replaceChildren(input, save, cancel, status); input.focus();
    }
    async deleteComment(id) {
      if (!this.win.confirm('Delete this comment?')) return;
      try { await requestData(this.session, `/comments/${id}`, {method:'DELETE'}); this.comments = this.comments.filter(item => item.id !== id); this.renderComments(); }
      catch (error) { this.showError(error); }
    }
    bindDetail(listen) {
      const title = byId(this.root, '[data-page-title]');
      listen(title, 'change', () => void this.saveField({title:title.value}));
      listen(byId(this.root, '[data-page-lifecycle]'), 'change', event => void this.saveField({status:event.currentTarget.value}));
      listen(byId(this.root, '[data-page-folder]'), 'change', event => void this.saveField({folder_id:event.currentTarget.value ? Number(event.currentTarget.value) : null}));
      listen(byId(this.root, '[data-page-pin]'), 'click', () => void this.saveField({pinned:!this.page?.pinned}));
      listen(byId(this.root, '[data-page-edit]'), 'click', () => this.setEditing(true));
      listen(byId(this.root, '[data-page-cancel]'), 'click', () => this.setEditing(false));
      listen(byId(this.root, '[data-page-save]'), 'click', () => void this.saveBody());
      listen(this.root, 'keydown', event => {
        if ((event.metaKey || event.ctrlKey) && event.key.toLowerCase() === 's' && this.editing) {
          event.preventDefault(); void this.saveBody();
        }
      });
      listen(byId(this.root, '[data-page-body]'), 'input', event => {
        byId(this.root, '[data-page-save]').disabled = Boolean(this.composers.get('page')?.pending) || event.currentTarget.value === (this.page?.content || '');
        byId(this.root, '[data-page-save-status]').textContent = 'Unsaved changes';
        if (!byId(this.root, '[data-page-preview-content]').hidden) this.renderMarkdown(byId(this.root, '[data-page-preview-content]'), event.currentTarget.value);
      });
      listen(byId(this.root, '[data-page-preview]'), 'click', event => {
        const editing = !byId(this.root,'[data-page-body]').hidden;
        byId(this.root,'[data-page-body]').hidden=editing;
        const preview = byId(this.root, '[data-page-preview-content]'); preview.hidden = !editing;
        if(editing)this.renderMarkdown(preview,byId(this.root,'[data-page-body]').value);
        event.currentTarget.textContent=editing?'Edit Markdown':'Preview';
        event.currentTarget.setAttribute('aria-pressed', String(editing));
      });
      listen(byId(this.root,'[data-page-files]'),'change',event=>void this.uploadFiles(event.currentTarget.files));
      listen(byId(this.root,'[data-page-attachment-upload]'),'dragover',event=>event.preventDefault());
      listen(byId(this.root,'[data-page-attachment-upload]'),'drop',event=>{event.preventDefault();void this.uploadFiles(event.dataTransfer?.files, undefined, 'drop');});
      listen(byId(this.root, '[data-page-comment-form]'), 'submit', event => { event.preventDefault(); void this.createComment(new FormData(event.currentTarget)); });
      listen(byId(this.root,'[data-page-comment-form] textarea[name="content"]'),'input',()=>{this.commentDraftVersion++;});
      listen(byId(this.root,'[data-page-comment-files]'),'change',event=>{this.commentDraftVersion++;void this.uploadFiles(event.currentTarget.files,null);});
      const commentForm=byId(this.root,'[data-page-comment-form]');
      listen(this.root,'lific:attachment-busy',event=>{
        if(commentForm.contains(event.target))commentForm.querySelector('[type=submit]').disabled=event.detail.busy;
        else byId(this.root,'[data-page-save]').disabled=event.detail.busy||!this.editing||byId(this.root,'[data-page-body]').value===(this.page?.content||'');
      });
      this.wireMentionInput(byId(this.root, '[data-page-comment-form] textarea[name="content"]'));
      listen(byId(this.root, '[data-page-comments-older]'), 'click', () => void this.loadOlderComments());
      listen(byId(this.root, '[data-page-delete]'), 'click', () => void this.deletePage());
      listen(byId(this.root, '[data-page-export]'), 'click', () => this.downloadMarkdown());
      listen(this.root, 'click', event => {
        const anchor = event.target.closest?.('a[href]');
        const match = anchor?.dataset.pageAttachmentLink || anchor?.getAttribute('href')?.match(/^\/(?:api\/)?attachments\/(\d+)(?:\/(?:preview|thumbnail))?$/)?.[1];
        if (!match || !this.attachmentClient) return;
        const attachmentId=Number(Array.isArray(match)?match[1]:match);
        const row=this.doc.getElementById(`attachment-${attachmentId}`)||this.doc.getElementById(`comment-attachment-${attachmentId}`);
        if(row){event.preventDefault();row.scrollIntoView({block:'center'});row.classList.add('tc-page-attachment--target');return;}
        event.preventDefault(); void this.downloadAttachment(Number(Array.isArray(match) ? match[1] : match), anchor);
      });
    }
    setEditing(editing) {
      this.editing = editing;
      if (!editing) {
        this.composers.get('page')?.cancelAll();
        byId(this.root, '[data-page-body]').value = this.page?.content || '';
        this.renderMarkdown(byId(this.root, '[data-page-preview-content]'), this.page?.content || '');
        byId(this.root, '[data-page-save-status]').textContent = 'Saved';
      }
      byId(this.root, '[data-page-body]').disabled = !editing;
      byId(this.root, '[data-page-edit]').hidden = editing;
      byId(this.root, '[data-page-save]').disabled = Boolean(this.composers.get('page')?.pending) || !editing || byId(this.root, '[data-page-body]').value === (this.page?.content || '');
      byId(this.root, '[data-page-cancel]').hidden = !editing;
      if (editing) byId(this.root, '[data-page-body]').focus();
      byId(this.root,'[data-page-body]').hidden=!editing;
      byId(this.root,'[data-page-preview-content]').hidden=editing;
      byId(this.root,'[data-page-preview]').hidden=!editing;
      byId(this.root,'[data-page-preview]').textContent=editing?'Preview':'Edit Markdown';
      byId(this.root,'[data-page-preview]').setAttribute('aria-pressed',String(!editing));
    }
    async saveBody() {
      if(this.composers.get('page')?.pending)return;
      const body = byId(this.root, '[data-page-body]').value;
      const ok = await this.saveField({content:body});
      if (ok && byId(this.root,'[data-page-body]').value === (this.page?.content || '')) this.setEditing(false);
      else if (ok) {byId(this.root,'[data-page-save-status]').textContent='Saved previous version. Unsaved changes remain.';byId(this.root,'[data-page-save]').disabled=false;}
    }
    async saveField(patch) {
      if (!this.page || !this.canEdit || this.public) return false;
      const generation = this.generation; const id = this.page.id;
      byId(this.root, '[data-page-save-status]').textContent = 'Saving…';
      try {
        const page = await requestData(this.session, `/pages/${id}`, {method:'PUT', body:JSON.stringify(patch)});
        if (!this.current(generation) || this.page?.id !== id) return false;
        if (this.page.project_id !== page.project_id) return false;
        this.page = page;
        const bodyDraft = byId(this.root, '[data-page-body]').value;
        const wasEditing = this.editing;
        this.renderDetail({renderComments:false});
        if (wasEditing) {byId(this.root, '[data-page-body]').value = bodyDraft; this.setEditing(true);}
        const bodyRemainsDirty = wasEditing && bodyDraft !== (page.content || '');
        byId(this.root, '[data-page-save-status]').textContent = bodyRemainsDirty ? 'Saved previous version. Unsaved changes remain.' : 'Saved';
        if(bodyRemainsDirty)byId(this.root,'[data-page-save]').disabled=false;
        return true;
      } catch (error) { this.showError(error); byId(this.root, '[data-page-save-status]').textContent = 'Could not save. Your changes are still here.'; return false; }
    }
    async createComment(form) {
      if(this.composers.get('comment')?.pending)return;
      const content = String(form.get('content') || '').trim(); if (!content || !this.canComment || this.public) return;
      const submittedContent=byId(this.root,'[data-page-comment-form] textarea[name="content"]')?.value||'';
      const fileInput=byId(this.root,'[data-page-comment-files]');
      const draftVersion=this.commentDraftVersion;
      try {
        const comment = await requestData(this.session, `/pages/${this.page.id}/comments`, {method:'POST', body:JSON.stringify({content})});
        this.comments.push(comment); this.renderComments();
        const composer=byId(this.root,'[data-page-comment-form] textarea[name="content"]');
        if(this.commentDraftVersion===draftVersion&&composer?.value===submittedContent){composer.value='';if(fileInput)fileInput.value='';this.composers.get('comment')?.cancelAll();}
        await this.loadCommentAttachments(this.generation);
      } catch (error) { this.showError(error); }
    }
    async createLabel(form) {
      const name = String(form.get('name') || '').trim();
      if (!name || !this.canEdit || this.public || this.page?.project_id === null) return;
      try {
        const label = await requestData(this.session, '/labels', {method:'POST', body:JSON.stringify({project_id:this.page.project_id,name,color:form.get('color') || '#64748b'})});
        const current = [...byId(this.root, '[data-page-labels] select').selectedOptions].map(option => option.value);
        const labelName = label.name || name;
        const available = [...byId(this.root, '[data-page-labels] select').options].map(option => ({name:option.value}));
        if (!available.some(item => item.name.toLowerCase() === labelName.toLowerCase())) available.push({name:labelName});
        this.renderLabels([...new Set([...current,labelName])], available);
        await this.saveField({labels:[...new Set([...current,labelName])]});
      } catch (error) { this.showError(error); }
    }
    wireMentionInput(input) {
      if (!input || !this.mentionCandidates.length || input.dataset.pageMentionsBound === 'true') return;
      input.dataset.pageMentionsBound = 'true';
      input.setAttribute('aria-autocomplete', 'list'); input.setAttribute('aria-expanded', 'false');
      input.addEventListener('input', () => {
        const before = input.value.slice(0, input.selectionStart);
        const match = before.match(/(^|\s)@([\p{L}\p{N}_.-]*)$/u);
        const current = input.parentElement.querySelector('[data-page-mention-menu]');
        if (!match) {current?.remove(); input.setAttribute('aria-expanded', 'false'); return;}
        const query = match[2].toLowerCase();
        const matches = this.mentionCandidates.filter(user => user.username.toLowerCase().startsWith(query)
          || (user.display_name || '').toLowerCase().startsWith(query)).slice(0, 8);
        if (!matches.length) {current?.remove(); input.setAttribute('aria-expanded', 'false'); return;}
        const menu = el(this.doc, 'ul', undefined, 'tc-page-mention-menu'); menu.dataset.pageMentionMenu = ''; menu.setAttribute('role', 'listbox');
        for (const user of matches) {
          const row = el(this.doc, 'li'); const option = el(this.doc, 'button', `${user.display_name || user.username} · @${user.username}`);
          option.type = 'button'; option.setAttribute('role', 'option');
          option.addEventListener('mousedown', event => {event.preventDefault(); this.selectMention(input, match, user);});
          row.append(option); menu.append(row);
        }
        if (current) current.replaceWith(menu); else input.insertAdjacentElement('afterend', menu);
        input.setAttribute('aria-expanded', 'true'); this.activeMentionIndex = 0;
      });
      input.addEventListener('keydown', event => {
        const menu = input.parentElement.querySelector('[data-page-mention-menu]');
        if (!menu) return;
        const options = [...menu.querySelectorAll('[role="option"]')];
        if (event.key === 'Escape') {menu.remove(); input.setAttribute('aria-expanded', 'false'); event.preventDefault();}
        else if (event.key === 'ArrowDown' || event.key === 'ArrowUp') {
          event.preventDefault(); const step = event.key === 'ArrowDown' ? 1 : -1;
          this.activeMentionIndex = (this.activeMentionIndex + step + options.length) % options.length;
          options.forEach((option, index) => option.setAttribute('aria-selected', String(index === this.activeMentionIndex)));
        } else if ((event.key === 'Enter' || event.key === 'Tab') && options.length) {
          event.preventDefault(); const before = input.value.slice(0, input.selectionStart); const match = before.match(/(^|\s)@([\p{L}\p{N}_.-]*)$/u);
          const user = this.mentionCandidates.find(candidate => options[this.activeMentionIndex]?.textContent.includes(`@${candidate.username}`));
          if (match && user) this.selectMention(input, match, user);
        }
      });
    }
    selectMention(input, match, user) {
      const start = input.selectionStart - match[2].length - 1;
      const before = input.value.slice(0, start); const after = input.value.slice(input.selectionEnd);
      const inserted = `@${user.username} `; input.value = before + inserted + after;
      input.setSelectionRange(before.length + inserted.length, before.length + inserted.length);
      input.parentElement.querySelector('[data-page-mention-menu]')?.remove(); input.setAttribute('aria-expanded', 'false');
      input.dispatchEvent(new this.win.Event('input', {bubbles:true}));
    }
    async loadOlderComments() {
      if (!this.commentCursor) return false;
      try { const result = await this.commentPage(this.page.id, this.commentCursor);
        this.comments = [...result.items, ...this.comments]; this.commentCursor = result.cursor;
        this.hasOlderComments = result.hasOlder; this.commentPaginationError=null; this.renderComments();
        byId(this.root, '[data-page-comments-older]').hidden = !this.hasOlderComments;
        byId(this.root, '[data-page-comments-older]').textContent='Load older comments';
        byId(this.root, '[data-page-error]').hidden=true;
        return true;
      } catch (error) {
        this.commentPaginationError=error;this.hasOlderComments=true;
        const older=byId(this.root, '[data-page-comments-older]');older.hidden=false;older.textContent='Retry loading older comments';
        const errorNode=byId(this.root,'[data-page-error]');errorNode.hidden=false;
        errorNode.replaceChildren(el(this.doc,'p',error.message));
        const retry=el(this.doc,'button','Retry older comments','tc-button');retry.type='button';retry.addEventListener('click',()=>void this.loadOlderComments());errorNode.append(retry);
        return false;
      }
    }
    async commentPage(id, cursor) {
      const params = new URLSearchParams({order:'desc',limit:'51'});
      if (cursor) {params.set('before_created_at', cursor.created_at); params.set('before_id', String(cursor.id));}
      const result = await this.session.request(`/pages/${id}/comments?${params}`);
      if (!result?.ok) throw new Error(result?.error || 'Could not load page comments.');
      const descending = result.data || [];
      const hasOlder = descending.length > 50 || result.headers?.get('x-comment-has-more') === 'true';
      const items = (descending.length > 50 ? descending.slice(0, 50) : descending).reverse();
      return {items, hasOlder, cursor: items.length ? {created_at:items[0].created_at,id:items[0].id} : cursor};
    }
    async deletePage() {
      if (!this.canEdit || this.public || !this.win.confirm(`Delete ${this.page.identifier}?`)) return;
      try { await requestData(this.session, `/pages/${this.page.id}`, {method:'DELETE'}); this.navigate(this.listHref()); }
      catch (error) { this.showError(error); }
    }
    async downloadMarkdown() {
      try {
        const resolved=this.session.resolve(`/export/pages/${encodeURIComponent(this.page.identifier)}`);
        if(resolved.kind!=='private')throw new Error('Page export is unavailable in this view.');
        const headers=new Headers();let token=null;try{token=this.win.localStorage?.getItem('lific_token');}catch{}if(token)headers.set('Authorization',`Bearer ${token}`);
        const response=await this.win.fetch(resolved.url,{headers,credentials:'same-origin'});if(!response.ok)throw new Error(`Export failed (HTTP ${response.status}).`);
        const href=this.trackUrl(this.win.URL.createObjectURL(await response.blob()));const link=el(this.doc,'a');link.href=href;
        link.download=response.headers.get('Content-Disposition')?.match(/filename="?([^";]+)"?/)?.[1]||`${this.page.identifier}.zip`;link.click();
      } catch(error){this.showError(error);}
    }
    async downloadAttachment(id, anchor) {
      const result = await this.fetchAttachment(id, 'original', (href, metadata) => {
        const link = el(this.doc, 'a'); link.href = href; link.download = metadata.filename; link.click();
      });
      if (!result.ok) this.showError(new Error(result.error));
    }
    async fetchAttachment(id, variant, use) {
      if (!this.attachmentClient) return {ok:false,error:'Attachment viewer is unavailable.'};
      const chunks = [];
      return this.attachmentClient.streamDownload(id, {variant, filename:'attachment', open:metadata => ({
        write: chunk => chunks.push(chunk),
        close: () => {
          const href = this.trackUrl(this.win.URL.createObjectURL(new Blob(chunks, {type:metadata.contentType || 'application/octet-stream'})));
          use(href, metadata);
        },
        abort: () => {chunks.length = 0;},
      })});
    }
    renderMarkdown(target, source) {
      target.innerHTML = markdown(source);
      for (const image of target.querySelectorAll('[data-page-attachment]')) {
        void (async()=>{
          let result=await this.fetchAttachment(Number(image.dataset.pageAttachment),'thumbnail',href=>{image.src=href;});
          if(!result.ok)result=await this.fetchAttachment(Number(image.dataset.pageAttachment),'original',href=>{image.src=href;});
          if(!result.ok){const fallback=el(this.doc,'span',`[${image.alt||'Image attachment'} unavailable]`);image.replaceWith(fallback);const status=byId(this.root,'[data-page-attachment-status]');if(status)status.textContent=result.error||'Image attachment preview unavailable.';}
        })();
      }
      const candidates = new Map(this.mentionCandidates.map(user => [user.username.toLowerCase(), user.display_name || user.username]));
      for (const mention of target.querySelectorAll('[data-page-mention]')) {
        const username = mention.dataset.pageMention; const display = candidates.get(username.toLowerCase());
        if (display) {mention.textContent = `@${display}`; mention.title = `@${username}`;}
      }
    }
    realtime(event) {
      if (!event || !/^(page|comment)\./.test(event.type) || (event.project_id != null && this.projectId != null && event.project_id !== this.projectId)) return;
      if (this.root.dataset.topcoatPages === 'list') void this.load();
      else if (!this.editing && !this.commentEditing) void this.load();
    }
    retryButton(callback) { const button = el(this.doc, 'button', 'Try again', 'tc-button'); button.type='button'; button.addEventListener('click', callback); return button; }
    navigate(href) { this.win.dispatchEvent(new this.win.CustomEvent('lific:navigate', {detail:{href,history:'push'}})); }
    showError(error) {
      const node = byId(this.root, this.root.dataset.topcoatPages === 'list' ? '[data-pages-error]' : '[data-page-error]');
      node.hidden = false; node.replaceChildren(el(this.doc, 'p', error.message));
    }
    trackUrl(url) {this.objectUrls.add(url);return url;}
    async loadAttachments(generation) {
      if(!this.page||!this.attachmentClient)return;
      try{const result=await this.attachmentClient.list({entity_type:'page',entity_id:this.page.id});if(!this.current(generation)||!result.ok)return;
        this.attachments=result.data||[];const target=byId(this.root,'[data-page-attachment-list]');target.replaceChildren();
        for(const attachment of this.attachments)target.append(this.attachmentRow(attachment,`attachment-${attachment.id}`));
      }catch(error){this.showError(error);}
    }
    attachmentRow(attachment,id) {
      const row=el(this.doc,'li');row.id=id;row.dataset.attachmentId=String(attachment.id);
      const name=el(this.doc,'button',attachment.filename,'tc-button');name.type='button';name.addEventListener('click',()=>void this.downloadAttachment(attachment.id,name));
      const preview=el(this.doc,'button','View','tc-button');preview.type='button';preview.addEventListener('click',()=>void this.viewAttachment(attachment));row.append(name,preview);return row;
    }
    async viewAttachment(attachment) {
      const viewer=byId(this.root,'[data-page-attachment-viewer]');const status=byId(this.root,'[data-page-attachment-status]');
      const generation=++this.previewGeneration;
      const isCurrent=()=>generation===this.previewGeneration;
      viewer.hidden=false;viewer.replaceChildren();status.textContent='';
      if(/^text\//i.test(attachment.mime||'')||/^(application\/(json|xml)|application\/x-yaml)$/i.test(attachment.mime||'')){
        const result=await this.attachmentClient.text(attachment.id);
        if(!isCurrent())return;
        if(result.ok){const pre=el(this.doc,'pre');pre.textContent=result.text;viewer.append(pre);return;}
        status.textContent=`Could not preview ${attachment.filename}: ${result.error}`;return;
      }
      if(/^image\//i.test(attachment.mime||'')&&attachment.mime!=='image/svg+xml'){
        let result=await this.fetchAttachment(attachment.id,'thumbnail',href=>{if(!isCurrent())return;const image=el(this.doc,'img');image.src=href;image.alt=attachment.filename;viewer.append(image);});
        if(!isCurrent())return;
        if(!result.ok)result=await this.fetchAttachment(attachment.id,'original',href=>{if(!isCurrent())return;const image=el(this.doc,'img');image.src=href;image.alt=attachment.filename;viewer.append(image);});
        if(!isCurrent())return;
        if(!result.ok)status.textContent=`Preview unavailable for ${attachment.filename}; use its filename to download the original.`;
        return;
      }
      if(isCurrent())status.textContent=`Preview unavailable for ${attachment.filename}; use its filename to download the original.`;
    }
    async loadCommentAttachments(generation) {
      if(!this.attachmentClient)return;
      for(const comment of this.comments){try{const result=await this.attachmentClient.list({entity_type:'comment',entity_id:comment.id});if(!this.current(generation)||!result.ok)continue;this.commentAttachments.set(comment.id,result.data||[]);}catch{}}
      if(this.current(generation))this.renderComments();
    }
    composer(kind) {
      if(this.composers.has(kind))return this.composers.get(kind);
      const textarea=kind==='page'?byId(this.root,'[data-page-body]'):byId(this.root,'[data-page-comment-form] textarea[name="content"]');
      const parent=kind==='page'?byId(this.root,'[data-page-attachments]'):byId(this.root,'[data-page-comment-form]');
      const host=el(this.doc,'div');parent.append(host);
      const write=value=>{textarea.value=value;textarea.dispatchEvent(new this.win.Event('input',{bubbles:true}));};
      const composer=this.win.LificTopcoatAttachments.createComposer({root:host,client:this.attachmentClient,win:this.win,
        textarea,text:{read:()=>textarea.value,write},target:kind==='page'?()=>({entity_type:'page',entity_id:this.page.id}):null,
        onStatus:message=>{byId(this.root,'[data-page-attachment-status]').textContent=message.replace(/^Uploaded (.*)\.$/,'Uploaded $1');},
        onUploaded:async(row,snippet)=>{
          if(kind==='page'&&!this.editing)this.setEditing(true);
          const start=textarea.selectionStart??textarea.value.length,end=textarea.selectionEnd??start;
          const before=textarea.value.slice(0,start),after=textarea.value.slice(end),insert=`${before&&!before.endsWith('\n')?'\n':''}${snippet}${after&&!after.startsWith('\n')?'\n':''}`;
          write(before+insert+after);textarea.setSelectionRange(start+insert.length,start+insert.length);
          if(kind==='page')await this.loadAttachments(this.generation);
        }});
      this.composers.set(kind,composer);return composer;
    }
    async uploadFiles(files,target={entity_type:'page',entity_id:this.page?.id},source='picker') {
      const kind=target?.entity_type==='page'?'page':'comment';
      if(!(kind==='page'?this.canEdit:this.canComment)||this.public||!files?.length||!this.attachmentClient)return;
      await this.composer(kind).enqueue(files,{source});
    }
    async followDeepLink(generation) {
      const location=this.win.location;const params=new URLSearchParams(location.search||'');
      const commentId=(params.get('comment')||location.hash.match(/comment-(\d+)/)?.[1]);
      if(commentId){while(this.current(generation)&&!this.comments.some(comment=>String(comment.id)===String(commentId))&&this.hasOlderComments){if(!await this.loadOlderComments())break;}
        this.doc.getElementById(`comment-${commentId}`)?.scrollIntoView({block:'center'});}
      const attachmentRef=(params.get('att')||location.hash.match(/(?:attachment-|att)(\d+)(?:-L\d+-\d+)?/)?.[1]);
      const attachmentId=String(attachmentRef||'').match(/(?:att)?(\d+)/)?.[1];
      if(attachmentId){const row=this.doc.getElementById(`attachment-${attachmentId}`)||this.doc.getElementById(`comment-attachment-${attachmentId}`);row?.scrollIntoView({block:'center'});row?.classList.add('tc-page-attachment--target');}
    }
    dispose() { if (this.disposed) return; this.disposed = true; this.generation++; this.win.clearTimeout(this.timer); this.listeners.forEach(remove => remove());this.uploads.forEach(task=>task.abort());this.composers.forEach(composer=>composer.dispose());this.composers.clear();this.objectUrls.forEach(url=>this.win.URL.revokeObjectURL(url)); }
  }
  function attach(root, options) { return new PagesController(root, options); }
  function mount(doc = globalThis.document, options) {
    return [...doc.querySelectorAll('[data-topcoat-pages]')].map(root => attach(root, options));
  }
  globalThis.LificTopcoatPages = Object.freeze({PagesController, attach, mount, markdown});
  if (globalThis.document) mount();
})();

(() => {
  'use strict';

  const $ = (root, selector) => root.querySelector(selector);
  const text = (doc, tag, value, className) => {
    const node = doc.createElement(tag);
    node.textContent = value == null ? '' : String(value);
    if (className) node.className = className;
    return node;
  };
  const identity = session => session?.state?.publicProject?.toUpperCase() || null;
  const projectName = value => /^[A-Za-z][A-Za-z0-9_-]*$/.test(value || '');
  const commentCursor = comments => comments.length ? comments[0] : null;
  const issueUrl = (project, identifier) => `/public/${encodeURIComponent(project)}/issues/${encodeURIComponent(identifier)}`;
  const pageUrl = (project, id) => `/public/${encodeURIComponent(project)}/pages/${id}`;
  const publicIssue = issue => ({
    title: issue.title || 'Untitled issue', identifier: issue.identifier || '', status: issue.status || '',
    priority: issue.priority || '', description: issue.description || issue.body || '', id: issue.id,
  });

  function appendInline(doc, parent, source, project) {
    const input = String(source ?? '');
    const pattern = /!\[([^\]]*)\]\((?:\/api)?\/attachments\/(\d+)(?:\/thumbnail)?\)|\[([^\]]+)\]\(([^)\s]+)\)|`([^`]+)`|\*\*([^*]+)\*\*|__([^_]+)__|\*([^*]+)\*|_([^_]+)_|~~([^~]+)~~/g;
    let cursor = 0, match;
    while ((match = pattern.exec(input))) {
      if (match.index > cursor) parent.append(doc.createTextNode(input.slice(cursor, match.index)));
      if (match[2]) {
        const image = doc.createElement('img'); image.dataset.publicImage = match[2]; image.alt = match[1]; image.loading = 'lazy'; parent.append(image);
      } else if (match[3]) {
        const url = match[4];
        const attachment = url.match(/^\/(?:api\/)?attachments\/(\d+)(?:\/thumbnail)?$/);
        const safe = /^https?:\/\//i.test(url) || url.startsWith(`/public/${project}/`);
        if (attachment) { const link = doc.createElement('a'); link.href = '#download'; link.dataset.publicDownload = attachment[1]; link.textContent = match[3]; parent.append(link); }
        else if (safe) { const link = doc.createElement('a'); link.href = url; link.textContent = match[3]; link.rel = 'nofollow noopener'; parent.append(link); }
        else parent.append(doc.createTextNode(match[3]));
      } else if (match[5]) {
        parent.append(text(doc, 'code', match[5]));
      } else if (match[6] || match[7]) {
        parent.append(text(doc, 'strong', match[6] || match[7]));
      } else if (match[10]) {
        parent.append(text(doc, 'del', match[10]));
      } else {
        parent.append(text(doc, 'em', match[8] || match[9]));
      }
      cursor = pattern.lastIndex;
    }
    if (cursor < input.length) parent.append(doc.createTextNode(input.slice(cursor)));
  }

  function renderMarkdown(doc, target, source, project) {
    target.replaceChildren();
    const lines = String(source ?? '').replace(/\r\n?/g, '\n').split('\n');
    let paragraph = [];
    let list = null;
    let fence = false;
    let code = [];
    const tableCells = line => {
      const value = line.trim().replace(/^\|/, '').replace(/\|$/, '');
      return value.split(/(?<!\\)\|/).map(cell => cell.trim().replace(/\\\|/g, '|'));
    };
    const tableSeparator = line => tableCells(line).length > 0 && tableCells(line).every(cell => /^:?-{3,}:?$/.test(cell));
    const flushParagraph = () => {
      if (!paragraph.length) return;
      const p = doc.createElement('p'); paragraph.forEach((line, index) => { if (index) p.append(doc.createElement('br')); appendInline(doc, p, line, project); });
      target.append(p); paragraph = [];
    };
    const flushList = () => { if (list) { target.append(list); list = null; } };
    const flushAll = () => { flushParagraph(); flushList(); };
    for (let index = 0; index < lines.length; index++) {
      const line = lines[index];
      if (fence) { if (/^\s*```/.test(line)) { const pre = doc.createElement('pre'); pre.append(text(doc, 'code', code.join('\n'))); target.append(pre); code = []; fence = false; } else code.push(line); continue; }
      if (/^\s*```/.test(line)) { flushAll(); fence = true; continue; }
      if (!line.trim()) { flushAll(); continue; }
      const heading = line.match(/^(#{1,6})\s+(.+)$/);
      if (heading) { flushAll(); const h = doc.createElement(`h${heading[1].length}`); appendInline(doc, h, heading[2], project); target.append(h); continue; }
      if (/^>\s?/.test(line)) { flushAll(); const quote = doc.createElement('blockquote'); const p = doc.createElement('p'); appendInline(doc, p, line.replace(/^>\s?/, ''), project); quote.append(p); target.append(quote); continue; }
      if (lines[index + 1] && line.includes('|') && tableSeparator(lines[index + 1])) {
        flushAll(); const headers = tableCells(line), separators = tableCells(lines[index + 1]);
        if (headers.length === separators.length) {
          const table = doc.createElement('table'), head = doc.createElement('thead'), header = doc.createElement('tr');
          const alignments = separators.map(cell => cell.startsWith(':') && cell.endsWith(':') ? 'center' : cell.endsWith(':') ? 'right' : cell.startsWith(':') ? 'left' : '');
          headers.forEach((value, column) => { const th = doc.createElement('th'); th.scope = 'col'; if (alignments[column]) th.style.textAlign = alignments[column]; appendInline(doc, th, value, project); header.append(th); });
          head.append(header); table.append(head); const body = doc.createElement('tbody'); index += 2;
          while (index < lines.length && lines[index].includes('|') && lines[index].trim()) {
            const cells = tableCells(lines[index]), row = doc.createElement('tr');
            headers.forEach((_, column) => { const td = doc.createElement('td'); if (alignments[column]) td.style.textAlign = alignments[column]; appendInline(doc, td, cells[column] || '', project); row.append(td); });
            body.append(row); index++;
          }
          table.append(body); target.append(table); index--; continue;
        }
      }
      const item = line.match(/^\s*(?:[-*+]\s+|\d+\.\s+)(.*)$/);
      if (item) { flushParagraph(); if (!list) list = doc.createElement(/^\s*\d+\./.test(line) ? 'ol' : 'ul'); const li = doc.createElement('li'); const task = item[1].match(/^\[([ xX])\]\s+(.*)$/); if (task) { const checkbox = doc.createElement('input'); checkbox.type = 'checkbox'; checkbox.disabled = true; checkbox.checked = task[1].toLowerCase() === 'x'; checkbox.setAttribute('aria-label', checkbox.checked ? 'Completed task' : 'Incomplete task'); li.append(checkbox); appendInline(doc, li, task[2], project); } else appendInline(doc, li, item[1], project); list.append(li); continue; }
      flushList(); paragraph.push(line);
    }
    if (fence) { const pre = doc.createElement('pre'); pre.append(text(doc, 'code', code.join('\n'))); target.append(pre); }
    flushAll();
  }

  class PublicController {
    constructor(root, {window: win = globalThis.window, session = win.lificSession} = {}) {
      this.root = root; this.win = win; this.doc = root.ownerDocument; this.session = session;
      this.project = root.dataset.publicProject; this.kind = root.dataset.topcoatPublic; this.identifier = root.dataset.publicIdentifier || '';
      this.generation = 0; this.disposed = false; this.audience = identity(session); this.projectRow = null; this.issue = null; this.page = null;
      this.comments = []; this.hasOlder = false; this.cursor = null; this.commentAttachments = new Map(); this.attachments = [];
      this.client = win.LificTopcoatAttachments?.createClient({session, win}); this.objectUrls = new Set(); this.listeners = [];
      const listen = (target, type, fn) => { target.addEventListener(type, fn); this.listeners.push(() => target.removeEventListener(type, fn)); };
      listen(root, 'click', event => void this.click(event));
      for (const name of ['lific:session-change', 'lific:account-change', 'lific:scope-change']) listen(win, name, () => this.transition());
      listen(win, 'hashchange', () => void this.followDeepLink());
      listen(win, 'popstate', () => void this.followDeepLink());
      listen(win, 'pagehide', event => { if (!event.persisted) this.dispose(); });
      listen(win, 'pageshow', event => { if (event.persisted) this.restore(); });
      void this.load();
    }
    current(generation) { return !this.disposed && generation === this.generation && identity(this.session) === this.project.toUpperCase(); }
    async read(path) {
      if (!this.session || identity(this.session) !== this.project.toUpperCase()) throw new Error('Public project scope changed.');
      const resolved = this.session.resolve(path, 'GET');
      if (!['public', 'synthetic'].includes(resolved?.kind)) throw new Error('Public reads are unavailable for this route.');
      const result = await this.session.request(path, {method: 'GET', credentials: 'omit'});
      if (!result?.ok) { const error = new Error(result?.error || `HTTP ${result?.status || 500}`); error.status = result.status; throw error; }
      return result;
    }
    async data(path) { return (await this.read(path)).data; }
    async load() {
      const generation = ++this.generation;
      const status = $(this.root, '[data-public-status]'); const output = $(this.root, '[data-public-content]');
      this.root.setAttribute('aria-busy', 'true'); status.textContent = 'Loading…'; output.hidden = true;
      $(this.root, '[data-public-error]').hidden = true;
      try {
        if (!projectName(this.project) || !this.current(generation)) throw new Error('Public project is unavailable.');
        const projects = await this.data('/projects'); if (!this.current(generation)) return;
        this.projectRow = projects.find(row => String(row.identifier || '').toLowerCase() === this.project.toLowerCase());
        if (!this.projectRow) throw new Error('This public project is unavailable.');
        const index = await this.data(`/projects/${this.projectRow.id}/index`); if (!this.current(generation)) return;
        if (this.kind === 'issues' || this.kind === 'board') this.renderIssueIndex(output, index);
        else if (this.kind === 'pages') this.renderPageIndex(output, index);
        else if (this.kind === 'issue-detail') await this.loadIssue(generation, output);
        else if (this.kind === 'page-detail') await this.loadPage(generation, output);
        if (!this.current(generation)) return;
        output.hidden = false; status.textContent = ''; this.root.setAttribute('aria-busy', 'false');
        await this.followDeepLink(generation);
      } catch (error) {
        if (!this.current(generation)) return;
        status.textContent = ''; this.root.setAttribute('aria-busy', 'false');
        const node = $(this.root, '[data-public-error]'); node.hidden = false; node.textContent = error.message;
      }
    }
    renderIssueIndex(output, index) {
      const rows = Array.isArray(index.issues) ? index.issues : [];
      if (this.kind === 'issues') {
        const list = text(this.doc, 'ul', undefined, 'tc-public__list');
        for (const raw of rows) { const issue = publicIssue(raw); const li = this.doc.createElement('li');
          const link = this.doc.createElement('a'); link.href = issueUrl(this.project, issue.identifier); link.textContent = `${issue.identifier} · ${issue.title}`;
          const meta = text(this.doc, 'span', [issue.status, issue.priority].filter(Boolean).join(' · '), 'tc-public__meta'); li.append(link, meta); list.append(li); }
        if (!rows.length) output.append(text(this.doc, 'p', 'No public issues.')); else output.append(list);
      } else {
        const statuses = ['backlog', 'todo', 'active', 'done', 'cancelled']; const board = text(this.doc, 'div', undefined, 'tc-public__board');
        for (const status of statuses) { const lane = text(this.doc, 'section', undefined, 'tc-public__lane'); lane.dataset.publicLane = status; lane.append(text(this.doc, 'h2', status));
          for (const raw of rows.filter(issue => (issue.status || 'backlog') === status)) { const issue = publicIssue(raw); const card = this.doc.createElement('a'); card.className = 'tc-public__card'; card.href = issueUrl(this.project, issue.identifier); card.textContent = `${issue.identifier} · ${issue.title}`; lane.append(card); }
          board.append(lane); }
        output.append(board);
      }
    }
    renderPageIndex(output, index) {
      const rows = Array.isArray(index.pages) ? index.pages : [];
      if (!rows.length) { output.append(text(this.doc, 'p', 'No public pages.')); return; }
      const list = text(this.doc, 'ul', undefined, 'tc-public__list');
      for (const page of rows) { const li = this.doc.createElement('li'); const link = this.doc.createElement('a');
        link.href = pageUrl(this.project, page.id); link.textContent = page.title || 'Untitled page'; li.append(link);
        if (page.status) li.append(text(this.doc, 'span', page.status, 'tc-public__meta'));
        if (page.preview) li.append(text(this.doc, 'p', page.preview, 'tc-public__preview'));
        list.append(li); }
      output.append(list);
    }
    async loadIssue(generation, output) {
      const resolved = await this.data(`/issues/resolve/${encodeURIComponent(this.identifier)}`);
      if (!this.current(generation)) return;
      const id = typeof resolved === 'number' ? resolved : resolved.id;
      if (!Number.isSafeInteger(id) || id < 0) throw new Error('This public issue is unavailable.');
      const issue = await this.data(`/issues/${id}`); if (!this.current(generation)) return;
      if (issue.project_id != null && issue.project_id !== this.projectRow.id) throw new Error('This public issue is unavailable.');
      this.issue = issue;
      const title = text(this.doc, 'h2', issue.title || 'Untitled issue'); title.id = `issue-${id}`; output.append(title);
      output.append(text(this.doc, 'p', [issue.identifier || this.identifier, issue.status, issue.priority].filter(Boolean).join(' · '), 'tc-public__meta'));
      const body = this.doc.createElement('article'); body.className = 'tc-public__markdown'; renderMarkdown(this.doc, body, issue.description || issue.body || '', this.project); output.append(body);
      this.hydrateImages(body, generation);
      const attach = await this.loadAttachments('issue', id, generation); if (!this.current(generation)) return;
      this.renderAttachments(output, attach, 'issue');
      await this.loadComments('issue', id, generation); if (!this.current(generation)) return;
      await this.loadCommentAttachments(generation); if (!this.current(generation)) return;
      this.renderComments(output);
    }
    async loadPage(generation, output) {
      const id = Number(this.identifier); if (!Number.isSafeInteger(id) || id < 0) throw new Error('This public page is unavailable.');
      const page = await this.data(`/pages/${id}`); if (!this.current(generation)) return;
      if (page.project_id != null && page.project_id !== this.projectRow.id) throw new Error('This public page is unavailable.');
      this.page = page;
      const title = text(this.doc, 'h2', page.title || 'Untitled page'); title.id = `page-${id}`; output.append(title);
      if (page.status) output.append(text(this.doc, 'p', page.status, 'tc-public__meta'));
      const body = this.doc.createElement('article'); body.className = 'tc-public__markdown'; renderMarkdown(this.doc, body, page.content || page.body || '', this.project); output.append(body);
      this.hydrateImages(body, generation);
      const attach = await this.loadAttachments('page', id, generation); if (!this.current(generation)) return;
      this.renderAttachments(output, attach, 'page');
      await this.loadComments('page', id, generation); if (!this.current(generation)) return;
      await this.loadCommentAttachments(generation); if (!this.current(generation)) return;
      this.renderComments(output);
    }
    async loadAttachments(entityType, id, generation) {
      if (!this.client) return [];
      const result = await this.client.list({entity_type: entityType, entity_id: id});
      if (!this.current(generation)) return [];
      if (!result.ok) throw new Error(result.error || 'Could not load public attachments.');
      this.attachments = result.data || []; return this.attachments;
    }
    async loadComments(entityType, id, generation, cursor = null, existing = []) {
      const params = new URLSearchParams({order: 'desc', limit: '51'});
      if (cursor) { params.set('before_created_at', cursor.created_at); params.set('before_id', String(cursor.id)); }
      const result = await this.read(`/${entityType === 'issue' ? 'issues' : 'pages'}/${id}/comments?${params}`);
      if (!this.current(generation)) return;
      const descending = Array.isArray(result.data) ? result.data : result.data?.items || [];
      this.hasOlder = descending.length > 50 || result.headers?.get?.('x-comment-has-more') === 'true';
      const page = (descending.length > 50 ? descending.slice(0, 50) : descending).reverse();
      this.comments = [...page, ...existing]; this.cursor = commentCursor(page) || cursor;
    }
    async loadCommentAttachments(generation) {
      if (!this.client) return;
      const next = new Map();
      for (const comment of this.comments) {
        const result = await this.client.list({entity_type: 'comment', entity_id: comment.id});
        if (!this.current(generation)) return;
        if (result.ok) next.set(comment.id, result.data || []);
      }
      this.commentAttachments = next;
    }
    renderComments(output) {
      const section = text(this.doc, 'section', undefined, 'tc-public__comments'); section.dataset.publicComments = '';
      section.append(text(this.doc, 'h3', 'Comments'));
      const list = this.doc.createElement('ol');
      for (const comment of this.comments) {
        const row = this.doc.createElement('li'); row.id = `comment-${comment.id}`;
        const author = comment.author_display_name || comment.author || 'Public contributor';
        row.append(text(this.doc, 'p', author, 'tc-public__meta'));
        const body = this.doc.createElement('div'); body.className = 'tc-public__markdown'; renderMarkdown(this.doc, body, comment.content || '', this.project); row.append(body);
        for (const attachment of this.commentAttachments.get(comment.id) || []) row.append(this.attachmentLink(attachment, `comment-attachment-${attachment.id}`));
        list.append(row);
      }
      if (!this.comments.length) section.append(text(this.doc, 'p', 'No public comments.'));
      else section.append(list);
      if (this.hasOlder) { const more = this.doc.createElement('a'); more.href = '#older-comments'; more.dataset.publicMoreComments = ''; more.textContent = 'Load older comments'; section.append(more); }
      output.append(section);
      void this.hydrateImages(section, this.generation);
    }
    renderAttachments(output, attachments, entity) {
      if (!attachments.length) return;
      const section = text(this.doc, 'section', undefined, 'tc-public__attachments'); section.append(text(this.doc, 'h3', 'Attachments'));
      const list = this.doc.createElement('ul');
      for (const attachment of attachments) list.append(this.attachmentLink(attachment, `attachment-${attachment.id}`));
      section.append(list); output.append(section);
    }
    attachmentLink(attachment, id) {
      const row = this.doc.createElement('li'); row.id = id; row.dataset.publicAttachment = String(attachment.id);
      const link = this.doc.createElement('a'); link.href = '#download'; link.dataset.publicDownload = String(attachment.id);
      link.textContent = `${attachment.filename || 'Attachment'}${Number.isFinite(attachment.size_bytes) ? ` · ${formatBytes(attachment.size_bytes)}` : ''}`;
      row.append(link); return row;
    }
    async hydrateImages(target, generation) {
      if (!this.client) return;
      for (const image of target.querySelectorAll('[data-public-image]')) {
        const id = Number(image.dataset.publicImage); const result = await this.client.thumbnail(id);
        if (!this.current(generation)) return;
        if (result.ok && result.blob) { const url = this.win.URL.createObjectURL(result.blob); this.objectUrls.add(url); image.src = url; }
        else { const fallback = text(this.doc, 'a', `Download image ${image.alt || id}`); fallback.href = '#download'; fallback.dataset.publicDownload = String(id); image.replaceWith(fallback); }
      }
    }
    async olderComments() {
      const generation = this.generation;
      const type = this.kind === 'issue-detail' ? 'issue' : 'page';
      const id = type === 'issue' ? this.issue?.id : this.page?.id;
      if (!id || !this.cursor || !this.hasOlder) return false;
      const existing = this.comments;
      const previousCursor = this.cursor;
      try {
        await this.loadComments(type, id, generation, this.cursor, existing);
        if (!this.current(generation)) return false;
        this.commentAttachments.clear(); await this.loadCommentAttachments(generation);
        if (!this.current(generation)) return false;
        $(this.root, '[data-public-content]').querySelector('[data-public-comments]')?.remove();
        this.renderComments($(this.root, '[data-public-content]'));
        return this.comments.length > existing.length || this.cursor?.id !== previousCursor?.id || this.cursor?.created_at !== previousCursor?.created_at;
      } catch (error) {
        const status = text(this.doc, 'p', `Could not load older comments: ${error.message}`, 'tc-public__error');
        $(this.root, '[data-public-comments]')?.append(status);
        return false;
      }
    }
    async download(id, anchor) {
      if (!this.client) return;
      const generation = this.generation;
      anchor.setAttribute('aria-busy', 'true');
      const chunks = [];
      const result = await this.client.streamDownload(id, {filename: anchor.textContent, open: () => ({write: chunk => chunks.push(chunk), close() {}, abort() {chunks.length = 0;}})});
      if (!this.current(generation)) return;
      anchor.removeAttribute('aria-busy');
      if (!result.ok) { anchor.textContent = `${anchor.textContent} · ${result.error}`; return; }
      const blob = new Blob(chunks, {type: result.contentType || 'application/octet-stream'});
      const url = this.win.URL.createObjectURL(blob); this.objectUrls.add(url);
      const link = this.doc.createElement('a'); link.href = url; link.download = result.filename || anchor.textContent; link.hidden = true;
      this.doc.body.append(link); link.click(); link.remove();
    }
    async click(event) {
      const more = event.target.closest('[data-public-more-comments]');
      if (more && this.root.contains(more)) { event.preventDefault(); await this.olderComments(); return; }
      const download = event.target.closest('[data-public-download]');
      if (download && this.root.contains(download)) { event.preventDefault(); await this.download(Number(download.dataset.publicDownload), download); }
    }
    async followDeepLink(generation = this.generation) {
      const query = new URLSearchParams(this.win.location.search || '');
      const comment = query.get('comment') || this.win.location.hash.match(/comment-(\d+)/)?.[1];
      const attachment = query.get('att')?.match(/(?:att)?(\d+)/)?.[1] || this.win.location.hash.match(/(?:att|attachment-)(\d+)(?:-L\d+-\d+)?/)?.[1];
      const targetVisible = () => (!comment || this.doc.getElementById(`comment-${comment}`))
        && (!attachment || this.doc.getElementById(`attachment-${attachment}`) || this.doc.getElementById(`comment-attachment-${attachment}`));
      while ((comment || attachment) && !targetVisible() && this.hasOlder && this.current(generation)) {
        if (!await this.olderComments()) break;
      }
      if (!this.current(generation)) return;
      if (comment) { const row = this.doc.getElementById(`comment-${comment}`); if (row) { row.id = `comment-${comment}`; row.scrollIntoView?.({block: 'center'}); } }
      if (attachment) { const row = this.doc.getElementById(`attachment-${attachment}`) || this.doc.getElementById(`comment-attachment-${attachment}`); row?.scrollIntoView?.({block: 'center'}); row?.classList.add('tc-public__target'); }
    }
    transition() {
      const next = identity(this.session); if (next === this.audience) return;
      this.audience = next; this.generation++; this.clear();
      this.project = this.root.dataset.publicProject;
      void this.load();
    }
    restore() {
      this.disposed = false; this.audience = identity(this.session); this.generation++; this.clear(); void this.load();
    }
    clear() {
      this.comments = []; this.attachments = []; this.commentAttachments.clear();
      $(this.root, '[data-public-content]').replaceChildren();
      for (const url of this.objectUrls) this.win.URL.revokeObjectURL(url); this.objectUrls.clear();
    }
    dispose() { this.disposed = true; this.generation++; for (const remove of this.listeners) remove(); this.listeners = []; this.clear(); }
  }
  function formatBytes(size) { if (!Number.isFinite(size) || size < 0) return ''; if (size < 1024) return `${size} B`; const units=['KB','MB','GB']; let value=size/1024, unit=0; while(value>=1024&&unit<units.length-1){value/=1024;unit++;} return `${value.toFixed(value>=10?0:1)} ${units[unit]}`; }
  function attach(root, options) { const controller = new PublicController(root, options); return {controller, dispose: () => controller.dispose()}; }
  const api = Object.freeze({PublicController, attach, renderMarkdown, appendInline, formatBytes});
  if (typeof module !== 'undefined' && module.exports) module.exports = api;
  if (typeof globalThis !== 'undefined') globalThis.LificTopcoatPublic = api;
  if (typeof window !== 'undefined') {
    const mount = () => { for (const root of document.querySelectorAll('[data-topcoat-public]:not([data-mounted])')) { root.dataset.mounted = 'true'; attach(root); } };
    if (document.readyState === 'loading') document.addEventListener('DOMContentLoaded', mount, {once: true}); else mount();
  }
})();

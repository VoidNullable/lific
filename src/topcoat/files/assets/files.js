(() => {
  'use strict';

  const PAGE_SIZE = 50;
  const MIME_FILTERS = [null, 'image', 'video', 'audio', 'text', 'pdf', 'archive', 'other'];
  const SORTS = ['created_at', 'size', 'filename'];
  const $ = (root, selector) => root.querySelector(selector);
  const escapeHtml = value => String(value ?? '').replace(/[&<>"']/g, c => ({'&':'&amp;','<':'&lt;','>':'&gt;','"':'&quot;',"'":'&#39;'}[c]));
  const formatBytes = bytes => {
    if (!Number.isFinite(bytes) || bytes <= 0) return '0 B';
    const units = ['B', 'KB', 'MB', 'GB', 'TB'];
    const rank = Math.min(Math.floor(Math.log(bytes) / Math.log(1024)), units.length - 1);
    const value = bytes / (1024 ** rank);
    return `${value.toFixed(rank === 0 || value >= 10 ? 0 : 1)} ${units[rank]}`;
  };
  const formatCountdown = seconds => {
    if (seconds <= 0) return 'swept on the next pass';
    const hours = Math.floor(seconds / 3600);
    if (hours >= 24) { const days = Math.floor(hours / 24); return `swept in ${days} day${days === 1 ? '' : 's'}`; }
    if (hours >= 1) return `swept in ${hours}h`;
    return `swept in ${Math.max(1, Math.floor(seconds / 60))} min`;
  };
  const canDelete = ({uploaderId, viewerId, isAdmin, canEdit, orphan = false}) =>
    Boolean(isAdmin || (!orphan && canEdit) || viewerId === null || (uploaderId !== null && uploaderId === viewerId));
  const entityHref = (project, entity) => {
    const identifier = String(entity.identifier || '');
    const pageOwner = value => {
      const marker = value.lastIndexOf('-DOC-');
      return marker > 0 && /^\d+$/.test(value.slice(marker + 5)) ? value.slice(0, marker) : project;
    };
    if (entity.entity_type === 'comment' && entity.page_id !== null && /^\d+$/.test(String(entity.page_id))) {
      return `/${encodeURIComponent(pageOwner(identifier))}/pages/${entity.page_id}`;
    }
    if (entity.entity_type === 'page' && /^\d+$/.test(String(entity.entity_id))) {
      return `/${encodeURIComponent(pageOwner(identifier))}/pages/${entity.entity_id}`;
    }
    if ((entity.entity_type === 'issue' || entity.entity_type === 'comment') && identifier) {
      const marker = identifier.lastIndexOf('-');
      const owner = marker > 0 && /^\d+$/.test(identifier.slice(marker + 1)) ? identifier.slice(0, marker) : project;
      return `/${encodeURIComponent(owner)}/issues/${encodeURIComponent(identifier)}`;
    }
    return null;
  };

  const identity = session => session.state.publicProject === null && session.state.user
    ? `private:${session.state.user.id}` : session.state.publicProject ? `public:${session.state.publicProject}` : null;
  async function requestData(session, path, options) {
    const result = await session.request(path, options);
    if (!result?.ok) throw new Error(result?.error || 'Request failed');
    return result.data;
  }

  function detectDelimiter(filename, sample) {
      const lower = filename.toLowerCase();
      if (lower.endsWith(".tsv") || lower.endsWith(".tab"))
          return "\t";
      if (lower.endsWith(".csv"))
          return ",";
      const firstLine = sample.split("\n", 1)[0] ?? "";
      const candidates = [",", "\t", ";", "|"];
      let best = ",";
      let bestCount = 0;
      for (const candidate of candidates) {
          const count = firstLine.split(candidate).length - 1;
          if (count > bestCount) {
              best = candidate;
              bestCount = count;
          }
      }
      return best;
  }
  function parseDelimited(text, options = {}) {
      const delimiter = options.delimiter ?? ",";
      const maxRows = options.maxRows ?? 200;
      const src = text.charCodeAt(0) === 0xfeff ? text.slice(1) : text;
      const rows = [];
      let totalRows = 0;
      let columnCount = 0;
      let field = "";
      let row = [];
      let inQuotes = false;
      let sawAnyChar = false;
      const endField = () => {
          row.push(field);
          field = "";
      };
      const endRow = () => {
          endField();
          const isBlank = row.length === 1 && row[0] === "";
          if (!isBlank) {
              columnCount = Math.max(columnCount, row.length);
              if (rows.length === 0)
                  rows.push(row);
              else {
                  totalRows += 1;
                  if (rows.length <= maxRows)
                      rows.push(row);
              }
          }
          row = [];
      };
      for (let i = 0; i < src.length; i++) {
          const ch = src[i];
          sawAnyChar = true;
          if (inQuotes) {
              if (ch === '"') {
                  if (src[i + 1] === '"') {
                      field += '"';
                      i += 1;
                  }
                  else {
                      inQuotes = false;
                  }
              }
              else {
                  field += ch;
              }
              continue;
          }
          if (ch === '"' && field === "") {
              inQuotes = true;
              continue;
          }
          if (ch === delimiter) {
              endField();
              continue;
          }
          if (ch === "\r")
              continue;
          if (ch === "\n") {
              endRow();
              continue;
          }
          field += ch;
      }
      if (sawAnyChar && (field !== "" || row.length > 0))
          endRow();
      const headers = rows.shift() ?? [];
      return {
          headers,
          rows,
          totalRows,
          truncated: totalRows > rows.length,
          columnCount: Math.max(columnCount, headers.length),
      };
  }
  function compareCells(a, b) {
      const left = a?.trim() ?? "";
      const right = b?.trim() ?? "";
      if (left === right)
          return 0;
      if (left === "")
          return 1;
      if (right === "")
          return -1;
      const ln = Number(left);
      const rn = Number(right);
      if (Number.isFinite(ln) && Number.isFinite(rn))
          return ln - rn;
      return left.localeCompare(right, undefined, { numeric: true, sensitivity: "base" });
  }
  function sortRows(rows, column, direction) {
      const sign = direction === "asc" ? 1 : -1;
      return [...rows].sort((a, b) => {
          const left = a[column] ?? "";
          const right = b[column] ?? "";
          if (left.trim() === "" && right.trim() !== "")
              return 1;
          if (right.trim() === "" && left.trim() !== "")
              return -1;
          return sign * compareCells(left, right);
      });
  }

  const HUNK_RE = /^@@+ -(\d+)(?:,(\d+))? \+(\d+)(?:,(\d+))? @@/;
  function cleanPath(raw) {
      const path = raw.trim().replace(/\t.*$/, "");
      if (path === "/dev/null")
          return path;
      return path.replace(/^[ab]\//, "");
  }
  function displayFor(oldPath, newPath) {
      if (oldPath === "/dev/null")
          return newPath;
      if (newPath === "/dev/null")
          return oldPath;
      if (oldPath && newPath && oldPath !== newPath)
          return `${oldPath} -> ${newPath}`;
      return newPath || oldPath || "(unknown file)";
  }
  function emptyFile() {
      return {
          oldPath: "",
          newPath: "",
          display: "(unknown file)",
          additions: 0,
          deletions: 0,
          binary: false,
          lines: [],
      };
  }
  function parseUnifiedDiff(text) {
      const files = [];
      let current = null;
      let oldNo = 0;
      let newNo = 0;
      const startFile = () => {
          const file = emptyFile();
          files.push(file);
          oldNo = 0;
          newNo = 0;
          return file;
      };
      const lines = text.split("\n");
      for (let i = 0; i < lines.length; i++) {
          const line = lines[i].replace(/\r$/, "");
          if (line.startsWith("diff --git ") || line.startsWith("diff -")) {
              const file = startFile();
              current = file;
              file.lines.push({ kind: "meta", text: line, oldNo: null, newNo: null });
              const m = line.match(/^diff --git (\S+) (\S+)$/);
              if (m) {
                  file.oldPath = cleanPath(m[1]);
                  file.newPath = cleanPath(m[2]);
                  file.display = displayFor(file.oldPath, file.newPath);
              }
              continue;
          }
          if (line.startsWith("--- ") && lines[i + 1]?.startsWith("+++ ")) {
              let file;
              if (current && current.lines.every((l) => l.kind === "meta")) {
                  file = current;
              }
              else {
                  file = startFile();
                  current = file;
              }
              file.oldPath = cleanPath(line.slice(4));
              file.newPath = cleanPath(lines[i + 1].slice(4));
              file.display = displayFor(file.oldPath, file.newPath);
              file.lines.push({ kind: "meta", text: line, oldNo: null, newNo: null });
              file.lines.push({ kind: "meta", text: lines[i + 1], oldNo: null, newNo: null });
              i += 1;
              continue;
          }
          if (!current) {
              if (!HUNK_RE.test(line))
                  continue;
              current = startFile();
          }
          const file = current;
          const hunk = line.match(HUNK_RE);
          if (hunk) {
              oldNo = Number(hunk[1]);
              newNo = Number(hunk[3]);
              file.lines.push({ kind: "hunk", text: line, oldNo: null, newNo: null });
              continue;
          }
          if (/^(index |old mode |new mode |new file mode |deleted file mode |similarity index |rename |copy |GIT binary patch|Binary files )/.test(line)) {
              if (line.startsWith("GIT binary patch") || line.startsWith("Binary files")) {
                  file.binary = true;
              }
              file.lines.push({ kind: "meta", text: line, oldNo: null, newNo: null });
              continue;
          }
          if (line.startsWith("+")) {
              file.lines.push({ kind: "add", text: line.slice(1), oldNo: null, newNo });
              file.additions += 1;
              newNo += 1;
              continue;
          }
          if (line.startsWith("-")) {
              file.lines.push({ kind: "del", text: line.slice(1), oldNo, newNo: null });
              file.deletions += 1;
              oldNo += 1;
              continue;
          }
          if (line.startsWith("\\")) {
              file.lines.push({ kind: "meta", text: line, oldNo: null, newNo: null });
              continue;
          }
          if (line.startsWith(" ") || line === "") {
              if (line === "" && i === lines.length - 1)
                  continue;
              file.lines.push({ kind: "context", text: line.slice(1), oldNo, newNo });
              oldNo += 1;
              newNo += 1;
              continue;
          }
          file.lines.push({ kind: "meta", text: line, oldNo: null, newNo: null });
      }
      return {
          files,
          additions: files.reduce((n, f) => n + f.additions, 0),
          deletions: files.reduce((n, f) => n + f.deletions, 0),
      };
  }
  function summarizeDiff(diff) {
      const count = diff.files.length;
      const noun = count === 1 ? "file" : "files";
      return `${count} ${noun} changed, +${diff.additions} -${diff.deletions}`;
  }
  function looksLikeDiff(text) {
      return /^@@+ -\d+(?:,\d+)? \+\d+(?:,\d+)? @@/m.test(text);
  }

  function renderTable(doc, target, source, filename) {
    const data = parseDelimited(source, {delimiter: detectDelimiter(filename, source)});
    const table = doc.createElement('table'); table.className = 'tc-files__data';
    const caption = doc.createElement('caption'); caption.textContent = `${data.columnCount} columns, ${data.totalRows} rows${data.truncated ? ` · showing first ${data.rows.length} rows` : ''}`; table.append(caption);
    const head = doc.createElement('thead'), header = doc.createElement('tr'), body = doc.createElement('tbody');
    let column = null, direction = 'asc';
    const cells = Array.from({length: data.columnCount}, (_, index) => {
      const cell = doc.createElement('th'); cell.scope = 'col'; cell.setAttribute('aria-sort', 'none');
      const button = doc.createElement('button'); button.type = 'button';
      const name = data.headers[index] || `Column ${index + 1}`; button.textContent = name; button.setAttribute('aria-label', `Sort by ${name}`);
      button.addEventListener('click', () => {direction = column === index && direction === 'asc' ? 'desc' : 'asc'; column = index; render();});
      cell.append(button); header.append(cell); return cell;
    });
    function render() {
      for (const [index, cell] of cells.entries()) cell.setAttribute('aria-sort', column === index ? direction === 'asc' ? 'ascending' : 'descending' : 'none');
      body.replaceChildren();
      for (const values of column === null ? data.rows : sortRows(data.rows, column, direction)) {
        const row = doc.createElement('tr');
        for (let index = 0; index < data.columnCount; index++) {const cell = doc.createElement('td'); cell.textContent = values[index] || ''; row.append(cell);}
        body.append(row);
      }
    }
    head.append(header); table.append(head, body); target.append(table); render();
  }
  function renderJson(doc, target, value) {
    function node(label, value, initiallyOpen = false) {
      if (value === null || typeof value !== 'object') {
        const scalar = doc.createElement('div'); scalar.dataset.jsonScalar = ''; scalar.textContent = `${label ? `${label}: ` : ''}${JSON.stringify(value)}`; return scalar;
      }
      const array = Array.isArray(value), keys = Object.keys(value), details = doc.createElement('details'), summary = doc.createElement('summary'), children = doc.createElement('div');
      summary.textContent = `${label ? `${label}: ` : ''}${array ? '[' : '{'}${keys.length} ${array ? 'items' : 'keys'}${array ? ']' : '}'}`;
      details.append(summary, children); let shown = 100;
      function render() {
        children.replaceChildren();
        if (!details.open) return;
        for (const key of keys.slice(0, shown)) children.append(node(key, value[key]));
        if (keys.length > shown) {
          const more = doc.createElement('button'); more.type = 'button'; more.textContent = `Show ${Math.min(100, keys.length - shown)} more items`;
          more.addEventListener('click', () => {shown += 100; render();}); children.append(more);
        }
      }
      details.addEventListener('toggle', render); details.open = initiallyOpen; render(); return details;
    }
    const tree = node('', value, true); tree.classList.add('tc-files__json'); target.append(tree);
  }
  function renderDiff(doc, target, source) {
    const diff = parseUnifiedDiff(source);
    if (!diff.files.length) {const pre = doc.createElement('pre'); pre.textContent = source; target.append(pre); return;}
    const summary = doc.createElement('p'); summary.textContent = summarizeDiff(diff); target.append(summary);
    for (const file of diff.files) {
      const title = doc.createElement('h3'); title.textContent = `${file.display} · +${file.additions} -${file.deletions}${file.binary ? ' · binary file' : ''}`; target.append(title);
      const table = doc.createElement('table'); table.className = 'tc-files__diff'; table.setAttribute('aria-label', `Changes in ${file.display}`);
      const body = doc.createElement('tbody');
      for (const line of file.lines) {
        const row = doc.createElement('tr'); row.dataset.diffKind = line.kind;
        for (const [side, number] of [['Old', line.oldNo], ['New', line.newNo]]) {const cell = doc.createElement('td'); cell.className = 'tc-files__line'; cell.textContent = number ?? ''; if (number !== null) cell.setAttribute('aria-label', `${side} line ${number}`); row.append(cell);}
        const cell = doc.createElement('td'), code = doc.createElement('code'); code.textContent = `${line.kind === 'add' ? '+ ' : line.kind === 'del' ? '- ' : ''}${line.text}`; cell.append(code); row.append(cell); body.append(row);
      }
      table.append(body); target.append(table);
    }
  }

  class FilesController {
    constructor(root, {window: win = globalThis.window, session = win.lificSession, attachmentClient = null} = {}) {
      this.root = root; this.win = win; this.doc = root.ownerDocument; this.session = session;
      this.attachmentClient = attachmentClient || win.LificTopcoatAttachments?.createClient({session, win});
      this.projectName = root.dataset.projectIdentifier;
      this.project = null; this.rows = []; this.orphans = []; this.links = new Map();
      this.mime = null; this.uploader = ''; this.sort = 'created_at'; this.totalCount = 0; this.totalBytes = 0; this.hasMore = false;
      this.loadingMore = false; this.canEdit = false; this.isAdmin = false; this.viewerId = session?.state?.user?.id ?? null;
      this.audience = session ? identity(session) : null; this.generation = 0; this.disposed = false; this.previewGeneration = 0;
      this.listeners = []; this.aborters = new Set(); this.objectUrls = new Set();
      const listen = (target, type, fn) => { target.addEventListener(type, fn); this.listeners.push(() => target.removeEventListener(type, fn)); };
      listen(root, 'click', event => this.click(event));
      listen(root, 'change', event => this.change(event));
      listen(root, 'cancel', event => { if (event.target.matches('[data-files-viewer]')) { this.previewGeneration++; this.clearViewer(); } });
      for (const name of ['lific:session-change', 'lific:account-change', 'lific:scope-change']) listen(win, name, () => this.transition());
      listen(win, 'pagehide', event => { if (!event.persisted) this.dispose(); });
      listen(win, 'pageshow', event => { if (event.persisted) this.transition(); });
      void this.load();
    }
    current(generation) { return !this.disposed && generation === this.generation && this.audience === identity(this.session); }
    transition() {
      const next = identity(this.session);
      if (next === this.audience) return;
      this.audience = next; this.generation++; this.previewGeneration++;
      for (const controller of this.aborters) controller.abort(); this.aborters.clear();
      this.clearViewer(); this.project = null; this.rows = []; this.orphans = []; this.links.clear();
      this.viewerId = this.session?.state?.user?.id ?? null;
      $(this.root, '[data-files-list]').replaceChildren(); $(this.root, '[data-files-orphans]').replaceChildren();
      this.root.setAttribute('aria-busy', 'true');
      void this.load();
    }
    async load() {
      if (!this.session || this.disposed) return;
      const generation = ++this.generation;
      this.root.setAttribute('aria-busy', 'true');
      const status = $(this.root, '[data-files-status]'); status.textContent = 'Loading files…';
      $(this.root, '[data-files-error]').hidden = true;
      try {
        if (!this.audience) throw new Error('Sign in to view project files.');
        const projects = await requestData(this.session, '/projects');
        if (!this.current(generation)) return;
        const project = projects.find(row => row.identifier.toLowerCase() === this.projectName.toLowerCase());
        if (!project) throw new Error(`Project ${this.projectName} not found`);
        this.project = project;
        const role = await requestData(this.session, `/projects/${project.id}/my-role`);
        if (!this.current(generation)) return;
        this.canEdit = Boolean(!role.enforced || role.is_admin || ['lead', 'maintainer', 'admin'].includes(role.role));
        this.isAdmin = Boolean(role.is_admin);
        if (this.viewerId === null) {
          const me = await requestData(this.session, '/auth/me');
          if (!this.current(generation)) return;
          this.viewerId = me.id;
        }
        await Promise.all([this.loadPage(true, generation), this.loadOrphans(generation)]);
        if (!this.current(generation)) return;
        this.render();
        status.textContent = `${this.totalCount} ${this.totalCount === 1 ? 'file' : 'files'} · ${formatBytes(this.totalBytes)}`;
        this.root.setAttribute('aria-busy', 'false');
      } catch (error) {
        if (!this.current(generation)) return;
        this.root.setAttribute('aria-busy', 'false'); status.textContent = '';
        this.showError(error.message, true);
      }
    }
    async loadPage(replace, generation = this.generation) {
      if (!replace && this.loadingMore) return;
      const offset = replace ? 0 : this.rows.length;
      const query = new URLSearchParams();
      if (this.mime) query.set('mime_class', this.mime);
      if (this.uploader) query.set('uploader', this.uploader);
      query.set('sort', this.sort); query.set('limit', String(PAGE_SIZE)); query.set('offset', String(offset));
      this.loadingMore = true;
      try {
        const data = await requestData(this.session, `/projects/${this.project.id}/attachments?${query}`);
        if (!this.current(generation)) return;
        this.rows = replace ? data.items : [...this.rows, ...data.items];
        this.totalCount = data.total_count; this.totalBytes = data.total_bytes; this.hasMore = data.has_more;
        this.render();
      } finally {
        if (this.current(generation)) { this.loadingMore = false; this.render(); }
      }
    }
    async loadOrphans(generation = this.generation) {
      const data = await requestData(this.session, `/projects/${this.project.id}/attachments/orphans`);
      if (!this.current(generation)) return;
      this.orphans = data.items; this.renderOrphans();
    }
    async reload() {
      if (!this.project) return this.load();
      const generation = ++this.generation;
      try {
        await Promise.all([this.loadPage(true, generation), this.loadOrphans(generation)]);
        if (this.current(generation)) this.render();
      } catch (error) { if (this.current(generation)) this.showError(error.message); }
    }
    render() {
      const list = $(this.root, '[data-files-list]'); list.replaceChildren();
      for (const row of this.rows) list.append(this.renderRow(row));
      $(this.root, '[data-files-count]').textContent = `${this.totalCount} ${this.totalCount === 1 ? 'file' : 'files'}`;
      $(this.root, '[data-files-bytes]').textContent = formatBytes(this.totalBytes);
      $(this.root, '[data-files-more]').hidden = !this.hasMore || this.loadingMore;
      const uploader = $(this.root, '[data-files-uploader]');
      const selected = this.uploader;
      const names = [...new Set(this.rows.map(row => row.uploader).filter(Boolean))].sort((a,b) => a.localeCompare(b));
      uploader.replaceChildren(new Option('All uploaders', '', false, !selected));
      for (const name of names) uploader.add(new Option(name, name, false, name === selected));
      for (const button of this.root.querySelectorAll('[data-files-mime]')) button.setAttribute('aria-pressed', String((button.dataset.filesMime || null) === this.mime));
      this.renderOrphans();
    }
    renderRow(row) {
      const item = this.doc.createElement('article'); item.className = 'tc-files__row'; item.dataset.fileId = String(row.id);
      const header = this.doc.createElement('div'); header.className = 'tc-files__row-main';
      const toggle = this.doc.createElement('button'); toggle.className = 'tc-files__expand'; toggle.type = 'button';
      toggle.dataset.filesExpand = String(row.id); toggle.setAttribute('aria-expanded', String(this.links.has(row.id)));
      toggle.textContent = this.links.has(row.id) ? '▾' : '▸'; toggle.setAttribute('aria-label', `Show where ${row.filename} is used`);
      const view = this.doc.createElement('button'); view.type = 'button'; view.className = 'tc-files__name';
      view.dataset.filesView = String(row.id); view.textContent = row.filename; view.title = `Preview or download ${row.filename}`;
      const mime = this.doc.createElement('span'); mime.className = 'tc-files__meta'; mime.textContent = row.mime_class;
      const size = this.doc.createElement('span'); size.className = 'tc-files__meta'; size.textContent = formatBytes(row.size_bytes);
      const uploader = this.doc.createElement('span'); uploader.className = 'tc-files__meta'; uploader.textContent = row.uploader_display_name || row.uploader || 'unknown';
      const created = this.doc.createElement('time'); created.className = 'tc-files__meta'; created.dateTime = row.created_at; created.textContent = new Date(row.created_at).toLocaleDateString();
      header.append(toggle, view, mime, size, uploader, created);
      const del = this.doc.createElement('button'); del.type = 'button'; del.className = 'tc-files__delete'; del.dataset.filesDelete = String(row.id); del.textContent = 'Delete';
      del.hidden = !canDelete({uploaderId: row.uploader_id, viewerId: this.viewerId, isAdmin: this.isAdmin, canEdit: this.canEdit});
      header.append(del); item.append(header);
      if (this.links.has(row.id)) item.append(this.renderLinks(this.links.get(row.id), row));
      return item;
    }
    renderLinks(data, row) {
      const section = this.doc.createElement('div'); section.className = 'tc-files__links';
      const entities = data?.entities?.length ? data.entities.map(entity => {
        if (entity.entity_type !== 'comment') return entity;
        const local = row.entities.find(candidate => candidate.entity_type === 'comment' && candidate.entity_id === entity.entity_id);
        return local ? {...entity, identifier: local.identifier, page_id: local.page_id} : entity;
      }) : row.entities;
      if (!entities.length) { section.append(this.doc.createTextNode(data ? 'No references in this project.' : 'Could not load references.')); }
      for (const entity of entities) {
        const href = entityHref(this.projectName, entity);
        if (!href) continue;
        const link = this.doc.createElement('a'); link.href = href; link.textContent = entity.entity_type === 'comment' ? `${entity.identifier || 'Comment'} (comment)` : entity.identifier || entity.title;
        link.title = `${entity.title} · ${entity.entity_type}`; section.append(link);
      }
      if (data?.duplicates?.length) {
        const dup = this.doc.createElement('div'); dup.className = 'tc-files__duplicates';
        const label = this.doc.createElement('p'); label.className = 'tc-files__meta';
        label.textContent = 'Identical content also appears in other project attachments:'; dup.append(label);
        const list = this.doc.createElement('ul');
        for (const entry of data.duplicates) {
          const item = this.doc.createElement('li');
          const filename = this.doc.createElement('span');
          filename.textContent = `${entry.filename} (#${entry.attachment_id})`;
          item.append(filename);
          if (entry.entities?.length) {
            const usages = this.doc.createElement('ul');
            for (const entity of entry.entities) {
              const href = entityHref(this.projectName, entity);
              if (!href) continue;
              const usage = this.doc.createElement('li');
              const link = this.doc.createElement('a'); link.href = href;
              link.textContent = entity.identifier || entity.title;
              usage.append(link); usages.append(usage);
            }
            if (usages.childElementCount) item.append(usages);
          }
          list.append(item);
        }
        dup.append(list); section.append(dup);
      }
      return section;
    }
    renderOrphans() {
      const target = $(this.root, '[data-files-orphans]');
      $(this.root, '[data-files-orphan-count]').textContent = `(${this.orphans.length})`;
      target.replaceChildren();
      if (!this.orphans.length) { target.textContent = 'No unlinked uploads.'; return; }
      for (const orphan of this.orphans) {
        const row = this.doc.createElement('article'); row.className = 'tc-files__orphan';
        const title = this.doc.createElement('button'); title.type = 'button'; title.dataset.filesOrphanView = String(orphan.id); title.textContent = orphan.filename;
        const info = this.doc.createElement('span'); info.textContent = `${formatBytes(orphan.size_bytes)} · ${orphan.uploader || 'unknown'} · ${formatCountdown(orphan.seconds_until_sweep)}`;
        row.append(title, info);
        if (canDelete({uploaderId: orphan.uploader_id, viewerId: this.viewerId, isAdmin: this.isAdmin, canEdit: this.canEdit, orphan: true})) {
          const del = this.doc.createElement('button'); del.type = 'button'; del.dataset.filesDelete = String(orphan.id); del.dataset.orphan = 'true'; del.textContent = 'Delete'; row.append(del);
        }
        target.append(row);
      }
    }
    async expand(id) {
      if (this.links.has(id)) { this.links.delete(id); this.render(); return; }
      const generation = this.generation;
      this.links.set(id, null); this.render();
      try {
        const links = await requestData(this.session, `/attachments/${id}/links`);
        if (!this.current(generation)) return;
        this.links.set(id, links);
      } catch { if (!this.current(generation)) return; this.links.set(id, null); }
      this.render();
    }
    showError(message, retry = false) {
      const error = $(this.root, '[data-files-error]'); error.hidden = false; error.replaceChildren();
      const text = this.doc.createElement('span'); text.textContent = message; error.append(text);
      if (retry) { const button = this.doc.createElement('button'); button.type = 'button'; button.dataset.filesRetry = ''; button.textContent = 'Try again'; error.append(button); }
    }
    async remove(id, orphan = false) {
      const generation = this.generation;
      const row = [...this.rows, ...this.orphans].find(item => item.id === id);
      if (!row || !canDelete({uploaderId: row.uploader_id, viewerId: this.viewerId, isAdmin: this.isAdmin, canEdit: this.canEdit, orphan})) return;
      const links = orphan ? {entities: []} : await requestData(this.session, `/attachments/${id}/links`).catch(() => ({entities: []}));
      if (!this.current(generation)) return;
      const count = links.entities?.length || 0;
      if (!this.win.confirm(count ? `Delete ${row.filename} and its ${count} reference${count === 1 ? '' : 's'}?` : `Delete ${row.filename}?`)) return;
      if (!this.current(generation)) return;
      try {
        await requestData(this.session, `/attachments/${id}`, {method: 'DELETE'});
        if (!this.current(generation)) return;
        $(this.root, '[data-files-error]').hidden = true;
        await this.reload();
      } catch (error) { this.showError(`Couldn't delete the file: ${error.message}`); }
    }
    async preview(id) {
      const row = [...this.rows, ...this.orphans].find(item => item.id === id);
      if (!row || !this.attachmentClient) return;
      const generation = ++this.previewGeneration;
      const dialog = $(this.root, '[data-files-viewer]'); const content = $(dialog, '[data-files-viewer-content]');
      $(dialog, '[data-files-viewer-title]').textContent = row.filename;
      $(dialog, '[data-files-viewer-status]').textContent = 'Loading preview…'; content.replaceChildren();
      const resolved = this.session.resolve(`/attachments/${id}`);
      $(dialog, '[data-files-download]').href = resolved.url;
      $(dialog, '[data-files-download]').download = row.filename;
      if (!dialog.open) dialog.showModal();
      const kind = this.win.LificTopcoatAttachments.viewerKind(row);
      if (kind === 'image') {
        let result = await this.attachmentClient.thumbnail(id);
        if (!result.ok) {
          const chunks = [];
          result = await this.attachmentClient.streamDownload(id, {open: () => ({
            write: chunk => chunks.push(chunk), close() {}, abort() { chunks.length = 0; },
          })});
          if (result.ok) result = {...result, blob: new Blob(chunks, {type: result.contentType || row.mime})};
        }
        if (generation !== this.previewGeneration) return;
        if (result.ok && result.blob) { const url = this.win.URL.createObjectURL(result.blob); this.objectUrls.add(url); const image = this.doc.createElement('img'); image.src = url; image.alt = row.alt_text || row.filename; content.append(image); $(dialog, '[data-files-viewer-status]').textContent = ''; }
        else $(dialog, '[data-files-viewer-status]').textContent = `${result.error || 'Preview unavailable.'} Use Download original.`;
      } else if (kind === 'video' || kind === 'audio') {
        const media = this.doc.createElement(kind); media.controls = true; media.preload = 'metadata';
        media.addEventListener('error', () => {
          if (generation !== this.previewGeneration) return;
          media.remove(); $(dialog, '[data-files-viewer-status]').textContent = 'Playback not supported in this browser. Use Download original.';
        }, {once: true});
        media.src = resolved.url; content.append(media); $(dialog, '[data-files-viewer-status]').textContent = '';
      } else if (['text', 'diff', 'csv', 'json'].includes(kind)) {
        const result = await this.attachmentClient.text(id);
        if (generation !== this.previewGeneration) return;
        if (result.ok) {
          const status = $(dialog, '[data-files-viewer-status]'); status.textContent = '';
          if (kind === 'csv') renderTable(this.doc, content, result.text, row.filename);
          else if (kind === 'json') {
            try {renderJson(this.doc, content, JSON.parse(result.text));}
            catch {status.textContent = 'Invalid JSON. Use Download original.'; const pre = this.doc.createElement('pre'); pre.textContent = result.text; content.append(pre);}
          } else if (kind === 'diff' || looksLikeDiff(result.text)) renderDiff(this.doc, content, result.text);
          else {const pre = this.doc.createElement('pre'); pre.textContent = result.text; content.append(pre);}
        }
        else $(dialog, '[data-files-viewer-status]').textContent = `${result.error || 'Preview unavailable.'} Use Download original.`;
      } else {
        const result = await this.attachmentClient.preview(id);
        if (generation !== this.previewGeneration) return;
        const status = $(dialog, '[data-files-viewer-status]');
        if (result.ok && result.data?.kind === 'zip') {
          const list = this.doc.createElement('ul');
          for (const entry of result.data.entries || []) { const item = this.doc.createElement('li'); item.textContent = `${entry.name} · ${formatBytes(entry.size)}`; list.append(item); }
          content.append(list); status.textContent = result.data.truncated ? `Archive preview truncated (${result.data.total_entries} entries).` : `${result.data.total_entries} archive entries.`;
        } else if (result.ok && result.data?.kind === 'sqlite') {
          const list = this.doc.createElement('ul');
          for (const table of result.data.tables || []) { const item = this.doc.createElement('li'); item.textContent = `${table.name} · ${table.rows} rows`; list.append(item); }
          content.append(list); status.textContent = `${result.data.tables?.length || 0} database tables.`;
        } else status.textContent = 'Preview unavailable for this file. Use Download original.';
      }
    }
    clearViewer() {
      const dialog = $(this.root, '[data-files-viewer]');
      if (dialog?.open) dialog.close();
      if (dialog) { $(dialog, '[data-files-viewer-title]').textContent = ''; $(dialog, '[data-files-viewer-content]').replaceChildren(); $(dialog, '[data-files-viewer-status]').textContent = ''; }
      for (const url of this.objectUrls) this.win.URL.revokeObjectURL(url); this.objectUrls.clear();
    }
    async click(event) {
      const target = event.target.closest('button'); if (!target || !this.root.contains(target)) return;
      if (target.matches('[data-files-mime]')) { this.mime = target.dataset.filesMime || null; await this.refilter(); }
      else if (target.matches('[data-files-expand]')) await this.expand(Number(target.dataset.filesExpand));
      else if (target.matches('[data-files-more]')) { if (!this.loadingMore) await this.loadPage(false).catch(error => this.showError(error.message)); }
      else if (target.matches('[data-files-orphans-toggle]')) { const box = $(this.root, '[data-files-orphans]'); box.hidden = !box.hidden; target.setAttribute('aria-expanded', String(!box.hidden)); }
      else if (target.matches('[data-files-delete]')) await this.remove(Number(target.dataset.filesDelete), target.dataset.orphan === 'true');
      else if (target.matches('[data-files-view],[data-files-orphan-view]')) await this.preview(Number(target.dataset.filesView || target.dataset.filesOrphanView));
      else if (target.matches('[data-files-retry]')) await this.load();
      else if (target.matches('[data-files-viewer-close]')) { this.previewGeneration++; this.clearViewer(); }
    }
    async change(event) {
      if (event.target.matches('[data-files-uploader]')) this.uploader = event.target.value;
      else if (event.target.matches('[data-files-sort]')) this.sort = SORTS.includes(event.target.value) ? event.target.value : 'created_at';
      else return;
      await this.refilter();
    }
    async refilter() {
      if (!this.project) return;
      const generation = ++this.generation;
      this.links.clear(); this.rows = []; this.hasMore = false; this.loadingMore = true;
      this.root.setAttribute('aria-busy', 'true'); this.render();
      try { await this.loadPage(true, generation); if (this.current(generation)) this.root.setAttribute('aria-busy', 'false'); }
      catch (error) { if (this.current(generation)) { this.root.setAttribute('aria-busy', 'false'); this.showError(error.message); } }
    }
    dispose() {
      this.disposed = true; this.generation++; this.previewGeneration++;
      for (const remove of this.listeners) remove(); this.listeners = [];
      for (const controller of this.aborters) controller.abort(); this.aborters.clear();
      this.clearViewer();
    }
  }

  function attach(root, options) { const controller = new FilesController(root, options); return {controller, dispose: () => controller.dispose()}; }
  const api = Object.freeze({PAGE_SIZE, MIME_FILTERS: Object.freeze(MIME_FILTERS), SORTS: Object.freeze(SORTS), formatBytes, formatCountdown, canDelete, entityHref, detectDelimiter, parseDelimited, sortRows, parseUnifiedDiff, summarizeDiff, FilesController, attach});
  if (typeof module !== 'undefined' && module.exports) module.exports = api;
  if (typeof globalThis !== 'undefined') globalThis.LificTopcoatFiles = api;
  if (typeof window !== 'undefined') {
    const mount = () => { for (const root of document.querySelectorAll('[data-topcoat-files]:not([data-mounted])')) { root.dataset.mounted = 'true'; attach(root); } };
    if (document.readyState === 'loading') document.addEventListener('DOMContentLoaded', mount, {once: true}); else mount();
  }
})();

/* Private command palette and route-owned registration API. */
(() => {
  'use strict';
  const KINDS = ['issue', 'page', 'plan', 'project', 'module', 'folder'];
  const LABELS = {issue: 'Issues', page: 'Pages', plan: 'Plans', project: 'Projects', module: 'Modules', folder: 'Folders'};
  const boundary = /[\s\-_/.,()[\]{}<>:;!?"'`]/;

  function parseRefQuery(query) {
    const q = query.trim();
    const bare = q.match(/^#?(\d+)$/);
    const page = q.match(/^doc[\s-]*(\d+)$/i);
    const qualifiedPage = q.match(/^([a-z][a-z0-9_]*)[\s-]*doc[\s-]*(\d+)$/i);
    const issue = q.match(/^([a-z][a-z0-9_]*?)[\s-]*(\d+)$/i);
    const parsed = bare ? {kind: 'issue', project: null, n: Number(bare[1])}
      : page ? {kind: 'page', project: null, n: Number(page[1])}
      : qualifiedPage ? {kind: 'page', project: qualifiedPage[1].toUpperCase(), n: Number(qualifiedPage[2])}
      : issue ? {kind: 'issue', project: issue[1].toUpperCase(), n: Number(issue[2])} : null;
    return parsed && Number.isSafeInteger(parsed.n) ? parsed : null;
  }

  function fuzzyScore(query, text) {
    const q = query.toLowerCase(), t = text.toLowerCase();
    const direct = t.indexOf(q);
    if (direct >= 0) return direct === 0 ? 0.95 : boundary.test(t[direct - 1]) ? 0.9 : 0.8;
    let qi = 0, first = -1, last = -1, run = 0, longest = 0, words = 0;
    for (let ti = 0; ti < t.length && qi < q.length; ti++) {
      if (q[qi] === t[ti]) {
        if (first < 0) first = ti;
        if (ti === last + 1) run++;
        else {run = 1; if (boundary.test(ti ? t[ti - 1] : ' ')) words++;}
        longest = Math.max(longest, run); last = ti; qi++;
      }
    }
    return qi === q.length ? Math.min(0.7, q.length / Math.max(1, last - first + 1) * 0.4 + longest / q.length * 0.4 + words / q.length * 0.2) : 0;
  }

  function distance(a, b, budget) {
    if (Math.abs(a.length - b.length) > budget) return budget + 1;
    let previous = Array.from({length: b.length + 1}, (_, i) => i);
    for (let i = 1; i <= a.length; i++) {
      const current = [i];
      for (let j = 1; j <= b.length; j++) current[j] = Math.min(previous[j] + 1, current[j - 1] + 1, previous[j - 1] + (a[i - 1] === b[j - 1] ? 0 : 1));
      if (Math.min(...current) > budget) return budget + 1;
      previous = current;
    }
    return previous[b.length];
  }

  function quality(term, text, fuzzy = true) {
    const t = String(text ?? '').toLowerCase();
    if (t === term) return 1;
    const at = t.indexOf(term);
    if (at >= 0) return term.length < 2 ? 0.6 : at === 0 ? 0.9 : boundary.test(t[at - 1]) ? 0.8 : 0.6;
    const budget = Math.max(1, Math.floor(term.length * 0.2));
    return fuzzy && term.length >= 4 && t.split(/[^a-z0-9]+/).some(word => word && distance(term, word, budget) <= budget) ? 0.4 : 0;
  }

  function searchDocuments(query, docs) {
    const terms = query.toLowerCase().split(/\s+/).filter(Boolean);
    const hits = terms.length ? docs.map(doc => {
      const fields = [[5, doc.title], [4, doc.identifier], [4, String(doc.identifier ?? '').match(/\d+$/)?.[0], false],
        ...(doc.labels ?? []).map(label => [2, label]), [1, doc.preview]];
      const scores = terms.map(term => Math.max(...fields.map(([weight, text, fuzzy]) => weight * quality(term, text, fuzzy))));
      return {doc, score: scores.every(score => score > 0) ? scores.reduce((a, b) => a + b, 0) / (terms.length * 5) : 0};
    }).filter(hit => hit.score > 0).sort((a, b) => b.score - a.score || String(b.doc.updated_at ?? '').localeCompare(String(a.doc.updated_at ?? '')) || a.doc.identifier.localeCompare(b.doc.identifier)) : [];
    const counts = new Map();
    return hits.filter(({doc}) => {const count = counts.get(doc.kind) ?? 0; counts.set(doc.kind, count + 1); return count < 8;});
  }

  function catalogResults(query, catalog) {
    const q = query.trim(), hits = [];
    for (const project of catalog.projects) {
      const match = fuzzyScore(q, project.name) || fuzzyScore(q, project.identifier);
      if (match >= 0.3) hits.push({kind: 'project', title: project.name, identifier: project.identifier,
        route: `/${project.identifier}/overview`, score: [project.name, project.identifier].some(value => value.toLowerCase() === q.toLowerCase()) ? 2.6 :
          [project.name, project.identifier].some(value => value.toLowerCase().startsWith(q.toLowerCase())) ? Math.max(2.2, match) : match});
    }
    for (const [kind, rows, segment] of [['module', catalog.modules, 'modules'], ['folder', catalog.folders, 'pages'], ['plan', catalog.plans, 'plans']]) {
      for (const row of rows ?? []) {
        const project = catalog.projects.find(project => project.id === row.project_id);
        const title = row.title ?? row.name, score = fuzzyScore(q, title);
        if (project && score >= 0.3) hits.push({kind, title, identifier: row.identifier, sub: project.name, score,
          route: `/${project.identifier}/${segment}${kind === 'folder' ? '' : `/${row.id}`}`});
      }
    }
    return hits.sort((a, b) => b.score - a.score);
  }

  function scopedResults(results, projects) {
    return results.filter(result => {
      const match = typeof result.route === 'string' && result.route.match(/^\/([A-Za-z][A-Za-z0-9_-]*)\/(overview|issues|pages|modules|plans)(?:\/([^/?#]+))?$/);
      const project = match && projects.find(project => project.identifier.toLowerCase() === match[1].toLowerCase());
      return project && KINDS.includes(result.kind) && typeof result.title === 'string' &&
        (match[2] === 'issues' ? !match[3] || /^[A-Za-z][A-Za-z0-9_-]*-\d+$/.test(match[3]) :
          match[2] === 'overview' ? !match[3] : !match[3] || /^\d+$/.test(match[3]));
    });
  }

  function recentResults(entries, projects, route) {
    const recent = entries.filter(entry => entry && ['issue', 'page', 'plan'].includes(entry.type)).map(entry => ({
      kind: entry.type, title: entry.title, identifier: entry.identifier, recent: true, score: 1,
      route: `/${entry.project}/${entry.type === 'issue' ? 'issues' : entry.type === 'page' ? 'pages' : 'plans'}/${entry.routeId}`,
    }));
    return scopedResults(recent, projects).filter(hit => hit.route.toLowerCase() !== route.toLowerCase()).slice(0, 5);
  }

  function snippetSegments(snippet) {
    const segments = [];
    let cursor = 0;
    while (cursor < snippet.length) {
      const start = snippet.indexOf('**', cursor), end = start < 0 ? -1 : snippet.indexOf('**', start + 2);
      if (end < 0) {segments.push({text: snippet.slice(cursor), highlighted: false}); break;}
      if (start > cursor) segments.push({text: snippet.slice(cursor, start), highlighted: false});
      if (end > start + 2) segments.push({text: snippet.slice(start + 2, end), highlighted: true});
      cursor = end + 2;
    }
    return segments;
  }

  function mount({window: win = globalThis.window, document: doc = win?.document,
    session = win?.lificSession, sync = win?.lificSync} = {}) {
    const dialog = doc?.querySelector('[data-topcoat-palette]');
    if (!dialog || !session) return null;
    const input = dialog.querySelector('[data-palette-input]');
    const list = dialog.querySelector('[data-palette-results]');
    const status = dialog.querySelector('[data-palette-status]');
    const errorNode = dialog.querySelector('[data-palette-error]');
    const modeNode = dialog.querySelector('[data-palette-mode]');
    const help = doc.querySelector('[data-palette-help]');
    let helpFocus = null;
    const owners = new Map();
    let disposed = false, open = false, returnFocus = null, epoch = 0, searchGeneration = 0;
    let identity = privateIdentity(), mode = {type: 'root'}, query = '', selected = 0, moved = false;
    let catalog = {projects: [], modules: [], folders: [], plans: []};
    let catalogGeneration = 0;
    let projectsAt = 0, catalogAt = 0, projectsPending = null, catalogPending = null;
    let items = [], local = [], remote = [], searching = false, error = '', timer = null, queuedEnter = null;

    function privateIdentity() {
      const state = session.state;
      return state.publicProject === null && state.user && !win.location.pathname.startsWith('/public/') &&
        !['/login', '/signup'].includes(win.location.pathname) ? state.user.id : null;
    }
    const currentProject = () => catalog.projects.find(project => project.identifier.toLowerCase() ===
      win.location.pathname.match(/^\/([A-Za-z][A-Za-z0-9_-]*)\//)?.[1]?.toLowerCase());
    const allowed = action => !action.requires || session.affordances?.()[action.requires] === true;
    const key = item => item.type === 'action' ? `action:${item.action.id}` : item.type === 'child' ? `child:${item.child.title}` : `nav:${item.result.route}`;
    const current = generation => !disposed && generation === epoch && identity !== null && privateIdentity() === identity;
    const searchCurrent = (generation, scope) => open && mode.type === 'root' && searchGeneration === generation && current(scope);

    async function request(path) {
      const response = await session.request(path);
      if (!response?.ok) throw new Error(response?.error || 'Search unavailable.');
      return response.data;
    }

    async function ensureProjects() {
      if (Date.now() - projectsAt < 60000) return catalog.projects;
      if (projectsPending) return projectsPending;
      const scope = epoch;
      const load = catalogGeneration;
      const pending = request('/projects').then(projects => {
        if (current(scope) && load === catalogGeneration) {
          if (!Array.isArray(projects)) throw new Error('Invalid project search response.');
          const changed = catalog.projects.length !== projects.length || projects.some(project =>
            !catalog.projects.some(previous => previous.id === project.id && previous.identifier === project.identifier));
          catalog = {...catalog, projects}; projectsAt = Date.now();
          if (changed) {catalog.modules = []; catalog.folders = []; catalog.plans = []; catalogAt = 0;}
        }
        return projects;
      }).finally(() => {if (projectsPending === pending) projectsPending = null;});
      projectsPending = pending;
      return pending;
    }

    async function ensureCatalog() {
      if (Date.now() - catalogAt < 60000) return;
      if (catalogPending) return catalogPending;
      const scope = epoch;
      const load = catalogGeneration;
      const pending = (async () => {
        await ensureProjects();
        const projects = catalog.projects;
        const loaded = {modules: [], folders: [], plans: []};
        for (let start = 0; start < projects.length && current(scope) && load === catalogGeneration; start += 4) {
          const batch = await Promise.all(projects.slice(start, start + 4).map(async project => {
            const responses = await Promise.allSettled(['modules', 'folders', 'plans'].map(kind => request(`/${kind}?project_id=${project.id}`)));
            return {project, responses};
          }));
          for (const {project, responses} of batch) ['modules', 'folders', 'plans'].forEach((kind, i) => {
            const response = responses[i];
            if (response.status === 'fulfilled' && Array.isArray(response.value)) loaded[kind].push(...response.value.filter(row => row.project_id === project.id));
          });
        }
        if (current(scope) && load === catalogGeneration) {catalog = {...catalog, ...loaded}; catalogAt = Date.now(); if (open && mode.type === 'root') publishLocal(true);}
      })().catch(failure => {if (current(scope) && load === catalogGeneration && open) {error = failure.message; render();}})
        .finally(() => {if (catalogPending === pending) catalogPending = null;});
      catalogPending = pending;
      return pending;
    }

    function documents(project) {
      const model = project && sync?.peekProject?.(project.id);
      return model?.status === 'ready' ? [...model.issues, ...model.pages] : [];
    }

    function docResult(doc, project, score) {
      return {kind: doc.kind, title: doc.title, identifier: doc.identifier, sub: doc.preview || project.name,
        route: `/${project.identifier}/${doc.kind === 'page' ? 'pages' : 'issues'}/${doc.kind === 'page' ? doc.id : doc.identifier}`, score};
    }

    function referenceLocal(ref) {
      const active = currentProject();
      return catalog.projects.flatMap(project => {
        const relevant = ref.project ? project.identifier.toLowerCase() === ref.project.toLowerCase() : ref.kind === 'issue' || project.id === active?.id;
        const identifier = `${project.identifier}-${ref.kind === 'page' ? 'DOC-' : ''}${ref.n}`;
        const row = relevant && documents(project).find(doc => doc.kind === ref.kind && doc.identifier.toLowerCase() === identifier.toLowerCase());
        return row ? [docResult(row, project, !ref.project && project.id === active?.id ? 4 : 3)] : [];
      });
    }

    function readRecents() {
      try {const entries = JSON.parse(win.localStorage.getItem('lific_recents') || '[]'); return Array.isArray(entries) ? entries : [];}
      catch {return [];}
    }

    function publishLocal(keepSelection = false) {
      const q = query.trim(), active = currentProject(), ref = parseRefQuery(q);
      if (q) {
        local = [
          ...(ref ? referenceLocal(ref) : []),
          ...searchDocuments(q, documents(active)).map(({doc, score}) => docResult(doc, active, 1.2 + score * 1.3)),
          ...catalogResults(q, catalog),
          ...[...owners.values()].flatMap(owner => owner.results ?? []).map(result => ({...result, score: fuzzyScore(q, result.title)})).filter(result => result.score >= 0.3),
        ];
      } else local = [...recentResults(readRecents(), catalog.projects, win.location.pathname),
        ...catalog.projects.map(project => ({kind: 'project', title: project.name, identifier: project.identifier, route: `/${project.identifier}/overview`, score: 0.5}))];
      publish(keepSelection);
    }

    function publish(keepSelection) {
      const old = keepSelection && moved && items[selected] ? key(items[selected]) : null, previous = selected;
      if (mode.type === 'prompt') items = [];
      else if (mode.type === 'submenu') items = (mode.action.children() ?? []).filter(child => !query.trim() || fuzzyScore(query.trim(), child.title) >= 0.3).map(child => ({type: 'child', child}));
      else {
        const actions = [...owners.values()].flatMap(registration => registration.actions.map(action => ({action, registration}))).filter(entry => allowed(entry.action));
        actions.push({action: {id: 'new-project', title: 'New project', run: () => navigate('/projects/new')}, registration: null});
        const hits = parseRefQuery(query) ? [] : query.trim() ? actions.map(entry => ({...entry, score: fuzzyScore(query.trim(), entry.action.title)}))
          .filter(hit => hit.score >= 0.3).sort((a, b) => b.score - a.score) : actions;
        const seen = new Set(), counts = new Map();
        const results = scopedResults([...local, ...remote].sort((a, b) => b.score - a.score), catalog.projects).filter(result => {
          const count = counts.get(result.kind) ?? 0;
          if (seen.has(result.route) || count >= 8) return false;
          seen.add(result.route); counts.set(result.kind, count + 1); return true;
        });
        const groups = [false, true].flatMap(server => KINDS.map(kind => {
          const rows = results.filter(result => !result.recent && result.kind === kind && Boolean(result.remote) === server);
          return {label: `${LABELS[kind]}${server ? rows.some(row => row.partial) ? ' (server, partial matches)' : ' (server)' : ''}`, rows, best: rows[0]?.score ?? 0};
        }).filter(group => group.rows.length).sort((a, b) => b.best - a.best));
        const recents = results.filter(result => result.recent);
        if (recents.length) groups.unshift({label: 'Recent', rows: recents});
        items = [...hits.map(entry => ({type: 'action', ...entry, group: 'Actions'})),
          ...groups.flatMap(group => group.rows.map(result => ({type: 'nav', result, group: group.label})))];
      }
      const next = old ? items.findIndex(item => key(item) === old) : -1;
      selected = next >= 0 ? next : old ? Math.min(previous, Math.max(0, items.length - 1)) : 0;
      render();
    }

    function render() {
      list.replaceChildren();
      let group = null;
      items.forEach((item, index) => {
        if (item.group && item.group !== group) {
          group = item.group; const heading = doc.createElement('div'); heading.className = 'tc-palette__group'; heading.textContent = group; list.append(heading);
        }
        const result = item.result ?? item.action ?? item.child;
        const button = doc.createElement('button'); button.type = 'button'; button.tabIndex = -1;
        button.id = `tc-palette-option-${index}`; button.setAttribute('role', 'option'); button.setAttribute('aria-selected', String(index === selected));
        if (item.type === 'nav') button.dataset.paletteRoute = result.route;
        const label = doc.createElement('span'); label.className = 'tc-palette__label';
        const title = doc.createElement('span'); title.textContent = result.title; label.append(title);
        if (result.sub) {
          const sub = doc.createElement('span'); sub.className = 'tc-palette__sub';
          // FTS markers are formatting only; server text never becomes HTML.
          for (const segment of snippetSegments(result.sub)) {
            const text = doc.createElement(segment.highlighted ? 'strong' : 'span'); text.textContent = segment.text; sub.append(text);
          }
          label.append(sub);
        }
        button.append(label);
        if (result.identifier || result.hint) {const hint = doc.createElement('span'); hint.className = 'tc-palette__hint'; hint.textContent = result.identifier ?? result.hint; button.append(hint);}
        button.addEventListener('mouseenter', () => {selected = index; moved = true; updateSelection();});
        button.addEventListener('click', event => {selected = index; pick(event.ctrlKey || event.metaKey);});
        button.addEventListener('mousedown', event => {if (event.button === 1) event.preventDefault();});
        button.addEventListener('auxclick', event => {if (event.button === 1) {event.preventDefault(); selected = index; pick(true);}});
        list.append(button);
      });
      modeNode.hidden = mode.type === 'root'; modeNode.textContent = mode.type === 'root' ? '' : mode.action.title.replace(/…$/, '');
      input.placeholder = mode.type === 'prompt' ? mode.action.prompt.placeholder ?? 'Type a value…' : mode.type === 'submenu' ? 'Filter…' : 'Jump or act…';
      input.setAttribute('aria-expanded', String(mode.type !== 'prompt'));
      status.textContent = mode.type === 'prompt' ? 'Enter to save · Esc to cancel' : searching ? 'Searching…' : items.length ? '' :
        query.trim() ? `Nothing matches “${query.trim()}”` : mode.type === 'submenu' ? 'Nothing here' : 'No projects yet';
      errorNode.hidden = !error; errorNode.textContent = error;
      list.setAttribute('aria-busy', String(searching));
      updateSelection();
    }

    function updateSelection() {
      for (const option of list.querySelectorAll('[role=option]')) option.setAttribute('aria-selected', String(option.id === `tc-palette-option-${selected}`));
      if (items[selected]) {input.setAttribute('aria-activedescendant', `tc-palette-option-${selected}`); doc.getElementById(`tc-palette-option-${selected}`)?.scrollIntoView({block: 'nearest'});}
      else input.removeAttribute('aria-activedescendant');
    }

    async function references(ref, generation, scope) {
      const active = currentProject();
      const projects = catalog.projects.filter(project => ref.project ? project.identifier.toLowerCase() === ref.project.toLowerCase() : ref.kind === 'issue' || project.id === active?.id);
      const hits = [];
      for (let start = 0; start < projects.length && searchCurrent(generation, scope); start += 4) {
        const batch = await Promise.all(projects.slice(start, start + 4).map(async project => {
          const identifier = `${project.identifier}-${ref.kind === 'page' ? 'DOC-' : ''}${ref.n}`;
          try {
            const row = ref.kind === 'page' ? (await request(`/pages?project_id=${project.id}`)).find(page => page.sequence === ref.n) : await request(`/issues/resolve/${encodeURIComponent(identifier)}`);
            return row ? docResult({...row, kind: ref.kind}, project, !ref.project && project.id === active?.id ? 4 : 3) : null;
          } catch {return null;}
        }));
        hits.push(...batch.filter(Boolean));
      }
      return hits;
    }

    function cancelSearch() {
      searchGeneration++; if (timer) win.clearTimeout(timer); timer = null; searching = false; queuedEnter = null;
    }

    async function searchRemote(generation, scope, q) {
      if (!searchCurrent(generation, scope)) return;
      searching = true; render();
      try {await ensureProjects();}
      catch (failure) {if (searchCurrent(generation, scope)) {searching = false; error = failure.message; render();} return;}
      if (!searchCurrent(generation, scope)) return;
      publishLocal(true);
      const ref = parseRefQuery(q), wantFts = local.filter(hit => ['issue', 'page'].includes(hit.kind)).length < 5;
      const [reference, fts] = await Promise.allSettled([ref ? references(ref, generation, scope) : Promise.resolve([]),
        wantFts ? request(`/search?${new URLSearchParams({query: q})}`) : Promise.resolve([])]);
      if (!searchCurrent(generation, scope)) return;
      remote = reference.status === 'fulfilled' ? reference.value : [];
      if (fts.status === 'fulfilled' && Array.isArray(fts.value)) {
        remote.push(...fts.value.flatMap((row, index) => {
          const project = catalog.projects.find(project => project.id === row.project_id);
          return project && ['issue', 'page'].includes(row.result_type) ? [{kind: row.result_type, title: row.title, identifier: row.identifier,
            sub: row.snippet || project.name, route: `/${project.identifier}/${row.result_type === 'page' ? 'pages' : 'issues'}/${row.result_type === 'page' ? row.id : row.identifier}`,
            score: 1 - index * .03, remote: true, partial: row.partial_match === true}] : [];
        }));
      } else if (fts.status === 'rejected') error = fts.reason.message;
      searching = false; publish(true);
      if (queuedEnter) {const pending = queuedEnter; queuedEnter = null; if (items[selected]?.type === 'nav' && items[selected].result.score >= 3) pick(pending.newTab);}
    }

    function runSearch(immediate = false) {
      cancelSearch(); remote = []; error = ''; moved = false;
      if (mode.type !== 'root') {publish(false); return;}
      publishLocal();
      const q = query.trim(), generation = searchGeneration, scope = epoch;
      if (q) {
        const fire = () => {timer = null; void searchRemote(generation, scope, q);};
        if (immediate) fire(); else timer = win.setTimeout(fire, 120);
      }
    }

    function navigate(href, newTab = false) {
      if (newTab) win.open(href, '_blank', 'noopener');
      else win.dispatchEvent(new win.CustomEvent('lific:navigate', {detail: {href, history: 'push'}}));
    }

    function pick(newTab = false) {
      const item = items[selected];
      if (!item || privateIdentity() !== identity) return;
      if (item.type === 'nav') {hide(); navigate(item.result.route, newTab);}
      else if (item.type === 'child') {if (allowed(mode.action)) {hide(); item.child.run();}}
      else if (allowed(item.action)) {
        const action = item.action;
        if (action.run) {hide(); action.run();}
        else {
          cancelSearch(); mode = {type: action.children ? 'submenu' : 'prompt', action, registration: item.registration};
          query = mode.type === 'prompt' ? action.prompt.initial ?? '' : ''; input.value = query;
          publish(false); input.focus(); if (mode.type === 'prompt') input.select();
        }
      }
    }

    async function show() {
      if (disposed || privateIdentity() === null || doc.querySelector('dialog[open]')) return;
      transition();
      returnFocus = doc.activeElement; open = true; mode = {type: 'root'}; query = ''; input.value = ''; remote = []; local = []; error = '';
      dialog.showModal(); input.focus(); runSearch();
      const scope = epoch, catalogScope = catalogGeneration;
      try {await ensureProjects(); if (current(scope) && open && mode.type === 'root') {publishLocal(true); void ensureCatalog();}}
      catch (failure) {if (current(scope) && catalogScope === catalogGeneration && open) {error = failure.message; render();}}
    }

    function hide() {
      cancelSearch(); open = false; mode = {type: 'root'};
      dialog.close(); if (returnFocus?.isConnected) returnFocus.focus(); returnFocus = null;
    }

    function hideHelp() {
      if (help?.open) {help.close(); if (helpFocus?.isConnected) helpFocus.focus(); helpFocus = null;}
    }

    function showHelp() {
      const shortcuts = [
        {scope: 'Global', keys: '⌘/Ctrl K', label: 'Open command palette'},
        {scope: 'Global', keys: '⌘/Ctrl P', label: 'Open command palette'},
        {scope: 'Global', keys: '⌘/Ctrl \\', label: 'Collapse or expand the sidebar'},
        {scope: 'Global', keys: '?', label: 'Show this shortcut list'},
        {scope: 'Global', keys: 'Esc', label: 'Close the open dialog'},
        {scope: 'Command palette', keys: '↓ ↑', label: 'Move selection'},
        {scope: 'Command palette', keys: 'Enter', label: 'Open / run'},
        {scope: 'Command palette', keys: '⌘/Ctrl Enter', label: 'Open in a new tab'},
        {scope: 'Command palette', keys: '⌫', label: 'Step back out of a submenu'},
        {scope: 'Command palette', keys: 'Esc', label: 'Close'},
        ...[...owners.values()].flatMap(owner => owner.shortcuts ?? []),
      ];
      const container = help.querySelector('[data-shortcut-list]'); container.replaceChildren();
      const groups = new Map();
      for (const shortcut of shortcuts) {
        if (!groups.has(shortcut.scope)) groups.set(shortcut.scope, []);
        groups.get(shortcut.scope).push(shortcut);
      }
      for (const [scope, entries] of groups) {
        const heading = doc.createElement('h3'); heading.textContent = scope; const list = doc.createElement('ul');
        for (const entry of entries) {
          const row = doc.createElement('li'), label = doc.createElement('span'), keys = doc.createElement('kbd');
          label.textContent = entry.label; keys.textContent = entry.keys; row.append(label, keys); list.append(row);
        }
        container.append(heading, list);
      }
      helpFocus = doc.activeElement; help.showModal(); help.querySelector('[data-shortcut-close]').focus();
    }

    function stepBack() {
      if (mode.type === 'root') hide();
      else {mode = {type: 'root'}; query = ''; input.value = ''; runSearch(); input.focus();}
    }

    function transition() {
      const next = privateIdentity();
      if (next !== identity) {
        hide(); hideHelp(); identity = next; epoch++; owners.clear(); catalog = {projects: [], modules: [], folders: [], plans: []};
        projectsAt = 0; catalogAt = 0; projectsPending = null; catalogPending = null; local = []; remote = []; items = []; error = ''; render();
      } else if (open) publish(true);
      for (const trigger of doc.querySelectorAll('[data-palette-open], [data-shortcut-open]')) trigger.hidden = next === null;
    }

    function onKeydown(event) {
      if (event.isComposing || event.defaultPrevented) return;
      const typing = ['INPUT', 'TEXTAREA', 'SELECT'].includes(doc.activeElement?.tagName) || doc.activeElement?.isContentEditable;
      if (privateIdentity() !== null && !typing && event.key === '?' && !event.metaKey && !event.ctrlKey &&
          (!doc.querySelector('dialog[open]') || help?.open)) {
        event.preventDefault(); if (help?.open) hideHelp(); else if (help) showHelp();
      } else if (privateIdentity() !== null && !typing && !doc.querySelector('dialog[open]') &&
          (event.metaKey || event.ctrlKey) && event.key === '\\') {
        event.preventDefault(); doc.querySelector('[data-sidebar-toggle]')?.click();
      } else if (help?.open && event.key === 'Escape') {event.preventDefault(); event.stopPropagation(); hideHelp();}
      else if ((event.metaKey || event.ctrlKey) && ['k', 'p'].includes(event.key.toLowerCase())) {
        if (privateIdentity() !== null && (!doc.querySelector('dialog[open]') || open)) {event.preventDefault(); if (open) hide(); else void show();}
      } else if (open && event.key === 'Escape') {event.preventDefault(); event.stopPropagation(); stepBack();}
      else if (open && event.target === input) {
        if (event.key === 'Enter') {
          event.preventDefault();
          if (mode.type === 'prompt') {const action = mode.action, value = query.trim(); hide(); if (value && allowed(action)) action.prompt.submit(value);}
          else if (mode.type === 'root' && parseRefQuery(query) && !moved && (timer || searching) && (items[0]?.type !== 'nav' || items[0].result.score < (parseRefQuery(query).project || !currentProject() ? 3 : 4))) {
            queuedEnter = {newTab: event.metaKey || event.ctrlKey};
            if (timer) {win.clearTimeout(timer); timer = null; void searchRemote(searchGeneration, epoch, query.trim());}
          } else pick(event.metaKey || event.ctrlKey);
        } else if (mode.type !== 'prompt' && ['ArrowDown', 'ArrowUp'].includes(event.key)) {
          event.preventDefault(); moved = true; selected = Math.max(0, Math.min(items.length - 1, selected + (event.key === 'ArrowDown' ? 1 : -1))); updateSelection();
        } else if (event.key === 'Backspace' && !query && mode.type === 'submenu') {event.preventDefault(); stepBack();}
      }
    }

    const onInput = () => {query = input.value; if (mode.type !== 'prompt') runSearch();};
    const onClick = event => {
      if (event.target.closest?.('[data-palette-open]')) void show();
      else if (event.target.closest?.('[data-shortcut-open]') && privateIdentity() !== null && !doc.querySelector('dialog[open]') && help) showHelp();
    };
    const onCancel = event => {event.preventDefault(); stepBack();};
    const onClose = () => hide();
    const onHelpCancel = event => {event.preventDefault(); hideHelp();};
    const onHelpClose = () => hideHelp();
    const onHelpBackdrop = event => {const rect = help.getBoundingClientRect(); if (event.target === help && (event.clientX < rect.left || event.clientX > rect.right || event.clientY < rect.top || event.clientY > rect.bottom)) hideHelp();};
    const onBackdrop = event => {const rect = dialog.getBoundingClientRect(); if (event.target === dialog && (event.clientX < rect.left || event.clientX > rect.right || event.clientY < rect.top || event.clientY > rect.bottom)) hide();};
    const onRoute = () => {cancelSearch(); if (mode.type !== 'root') hide(); hideHelp(); owners.clear(); if (privateIdentity() !== identity) transition(); else if (open) runSearch(true);};
    const onCatalog = event => {
      if (privateIdentity() === null) return;
      catalogGeneration++; projectsPending = null; catalogPending = null; catalogAt = 0;
      const snapshot = event.detail?.projects;
      catalog = {projects: Array.isArray(snapshot) ? snapshot.map(project => ({...project})) : [], modules: [], folders: [], plans: []};
      projectsAt = Array.isArray(snapshot) ? Date.now() : 0;
      if (open && mode.type === 'root') {runSearch(); void ensureCatalog();}
    };
    const onSync = () => {if (open && mode.type === 'root') publishLocal(true);};
    const unsubscribe = sync?.subscribe?.(onSync);
    const events = [['keydown', onKeydown], ['lific:account-change', transition], ['lific:session-change', transition], ['lific:scope-change', transition],
      ['popstate', onRoute], ['hashchange', onRoute], ['lific:project-catalog', onCatalog]];
    for (const [name, listener] of events) win.addEventListener(name, listener);
    doc.addEventListener('click', onClick); input.addEventListener('input', onInput); dialog.addEventListener('cancel', onCancel); dialog.addEventListener('click', onBackdrop);
    dialog.querySelector('[data-palette-close]').addEventListener('click', onClose);
    help?.addEventListener('cancel', onHelpCancel); help?.addEventListener('click', onHelpBackdrop);
    help?.querySelector('[data-shortcut-close]').addEventListener('click', onHelpClose);
    transition();

    return {
      open: show, close: hide,
      register(owner, registration) {
        if (typeof owner !== 'string' || !owner) throw new TypeError('Palette owner must have a name.');
        const value = {actions: registration.actions ?? [], results: registration.results ?? [], shortcuts: registration.shortcuts ?? []};
        for (const action of value.actions) {
          const modes = [typeof action.run === 'function', typeof action.children === 'function', typeof action.prompt?.submit === 'function'].filter(Boolean).length;
          if (typeof action.id !== 'string' || typeof action.title !== 'string' || modes !== 1 || action.requires && !['edit', 'manage', 'comment', 'publish', 'admin'].includes(action.requires)) throw new TypeError('Invalid palette action.');
        }
        if (privateIdentity() !== null) {
          const previous = owners.get(owner);
          if (mode.type !== 'root' && mode.registration === previous) hide();
          owners.set(owner, value); if (open) publishLocal(true);
        }
        return () => {if (owners.get(owner) === value) {
          if (mode.type !== 'root' && mode.registration === value) hide();
          owners.delete(owner); if (open) publishLocal(true);
        }};
      },
      dispose() {
        hide(); hideHelp(); disposed = true; epoch++; owners.clear();
        for (const [name, listener] of events) win.removeEventListener(name, listener);
        doc.removeEventListener('click', onClick); input.removeEventListener('input', onInput); dialog.removeEventListener('cancel', onCancel); dialog.removeEventListener('click', onBackdrop);
        dialog.querySelector('[data-palette-close]').removeEventListener('click', onClose); unsubscribe?.();
        help?.removeEventListener('cancel', onHelpCancel); help?.removeEventListener('click', onHelpBackdrop);
        help?.querySelector('[data-shortcut-close]').removeEventListener('click', onHelpClose);
      },
    };
  }

  const api = {parseRefQuery, searchDocuments, catalogResults, scopedResults, recentResults, snippetSegments, mount};
  if (typeof module !== 'undefined' && module.exports) module.exports = api;
  if (typeof window !== 'undefined') {window.LificTopcoatPalette = api; window.lificPalette?.dispose?.(); window.lificPalette = mount();}
})();

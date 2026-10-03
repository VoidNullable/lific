(() => {
  'use strict';
  const SECTIONS = new Set(['issues', 'modules', 'pages', 'plans']);
  const PROJECT = /^[A-Za-z][A-Za-z0-9_-]*$/;
  const ISSUE = /^[A-Za-z][A-Za-z0-9_-]*-\d+$/;

  function selection(value) {
    const project = value?.project;
    return value?.public !== true && Number.isSafeInteger(project?.id) && project.id > 0 &&
      typeof project.identifier === 'string' && PROJECT.test(project.identifier)
      ? {project: Object.freeze({id: project.id, identifier: project.identifier}),
        section: SECTIONS.has(value.section) ? value.section : null} : null;
  }

  async function load(request, project, section) {
    let result;
    switch (section) {
      case 'issues':
        result = await request(`/issues?project_id=${project.id}&order_by=updated&order=desc&limit=5`);
        break;
      case 'modules':
        result = await request(`/modules?project_id=${project.id}`);
        break;
      case 'pages': {
        const results = await Promise.all(['draft', 'active', 'complete'].map(status =>
          request(`/pages?project_id=${project.id}&status=${status}&order_by=updated&order=desc&limit=5`)));
        result = results.find(value => !value.ok && [401, 403, 404].includes(value.status)) ??
          results.find(value => !value.ok) ?? {ok: true, data: results.flatMap(value => {
          if (!Array.isArray(value.data)) throw new Error('Invalid recent resource response.');
          return value.data;
        })};
        break;
      }
      case 'plans':
        result = await request(`/plans?project_id=${project.id}&limit=10`);
        break;
    }
    if (!result.ok) return result;
    if (!Array.isArray(result.data)) throw new Error('Invalid recent resource response.');
    let candidates = result.data.map(row => {
      const label = section === 'modules' ? row?.name : row?.title;
      if (row?.project_id !== project.id || !Number.isSafeInteger(row.id) || row.id < 1 ||
        typeof label !== 'string' || (section === 'issues' && !ISSUE.test(row.identifier)) ||
        (['modules', 'pages'].includes(section) && typeof row.updated_at !== 'string') ||
        (section === 'plans' && typeof row.status !== 'string')) {
        throw new Error('Invalid recent resource response.');
      }
      return row;
    });
    if (section === 'modules') candidates.sort((a, b) => b.updated_at.localeCompare(a.updated_at));
    if (section === 'pages') candidates = candidates
      .filter(row => ['draft', 'active', 'complete'].includes(row.status))
      .sort((a, b) => b.updated_at.localeCompare(a.updated_at) || b.id - a.id);
    if (section === 'plans') candidates = candidates.filter(row => row.status !== 'archived');
    const rows = candidates.slice(0, 5).map(row => Object.freeze({
      href: `/${project.identifier}/${section}/${section === 'issues' ? row.identifier : row.id}`,
      label: section === 'modules' ? row.name : row.title,
      identifier: section === 'issues' ? row.identifier : null,
    }));
    return {ok: true, data: Object.freeze(rows)};
  }

  function createClient({request, publish = () => {}}) {
    let generation = 0;
    let projectKey = null;
    let cache = new Map();
    let state = Object.freeze({project: null, section: null, rows: Object.freeze([]),
      visible: false, loading: false, error: null, open: false});
    const update = changes => {state = Object.freeze({...state, ...changes}); publish(state);};
    return {
      peek: () => state,
      async activate(value) {
        const selected = selection(value);
        const current = ++generation;
        const key = selected ? `${selected.project.id}:${selected.project.identifier}` : null;
        if (key !== projectKey) {cache = new Map(); projectKey = key;}
        if (!selected || selected.section === null) {
          update({project: selected?.project ?? null, section: null, rows: Object.freeze([]), visible: false, loading: false, error: null});
          return;
        }
        const {project, section} = selected;
        update({project: Object.freeze({...project}), section, rows: cache.get(section) ?? Object.freeze([]),
          visible: true, loading: true, error: null});
        let result;
        try { result = await load(request, project, section); }
        catch (error) { result = {ok: false, error: error.message || 'Could not load recent resources.'}; }
        if (generation !== current) return;
        if (result.ok) {
          cache.set(section, result.data);
          update({rows: result.data, loading: false});
        } else {
          if ([401, 403, 404].includes(result.status)) cache.clear();
          update({rows: cache.get(section) ?? Object.freeze([]), loading: false,
            error: result.error || 'Could not load recent resources.'});
        }
      },
      setOpen(open) {update({open: Boolean(open)});},
      invalidate() {
        generation++;
        projectKey = null;
        cache.clear();
        update({project: null, section: null, rows: Object.freeze([]), visible: false, loading: false, error: null});
      },
      dispose() {generation++; cache.clear();},
    };
  }

  function attach(root, {session, catalog = null, win = root.ownerDocument.defaultView}) {
    const doc = root.ownerDocument;
    const toggle = root.querySelector('[data-recents-toggle]');
    const content = root.querySelector('[data-recents-content]');
    const status = root.querySelector('[data-recents-status]');
    const list = root.querySelector('[data-recents-list]');
    const error = root.querySelector('[data-recents-error]');
    let renderedRows = null;
    let routeKey = null;
    let audience = null;
    function route() {
      const value = win.location.hash.startsWith('#/')
        ? win.location.hash.slice(1) : `${win.location.pathname}${win.location.search}`;
      return {key: value.split('#')[0], path: value.split(/[?#]/)[0]};
    }
    function render(state) {
      root.hidden = !state.visible;
      toggle.textContent = state.section ? `Recent ${state.section}` : 'Recent resources';
      toggle.setAttribute('aria-expanded', String(state.open));
      content.hidden = !state.open;
      content.setAttribute('aria-busy', String(state.loading));
      status.textContent = state.loading ? `Loading recent ${state.section}…`
        : state.rows.length === 0 && !state.error && state.visible ? `No recent ${state.section}.` : '';
      status.hidden = !status.textContent;
      error.textContent = state.error ?? '';
      error.hidden = !state.error;
      if (renderedRows !== state.rows) {
        const focusedLink = list.contains(doc.activeElement) ? doc.activeElement.closest('a') : null;
        const focusedHref = focusedLink?.getAttribute('href');
        list.replaceChildren(...state.rows.map(row => {
          const item = doc.createElement('li');
          const link = doc.createElement('a');
          link.setAttribute('href', row.href);
          const label = row.identifier ? `${row.identifier}: ${row.label}` : row.label;
          link.setAttribute('title', label);
          link.setAttribute('aria-label', label);
          if (row.identifier) {
            const identifier = doc.createElement('span');
            identifier.setAttribute('class', 'tc-recents__identifier');
            identifier.textContent = `#${row.identifier.split('-').at(-1)}`;
            link.append(identifier);
          }
          const title = doc.createElement('span');
          title.setAttribute('class', 'tc-recents__label');
          title.textContent = row.label;
          link.append(title);
          item.append(link);
          return item;
        }));
        renderedRows = state.rows;
        if (focusedHref) {
          const replacement = Array.from(list.querySelectorAll('a'))
            .find(link => link.getAttribute('href') === focusedHref);
          if (replacement) replacement.focus({preventScroll: true});
          else if (state.visible) toggle.focus({preventScroll: true});
        }
      }
      const path = route().path.toLowerCase();
      for (const link of list.querySelectorAll('a')) {
        const href = link.getAttribute('href').toLowerCase();
        if (path === href || path.startsWith(`${href}/`)) link.setAttribute('aria-current', 'page');
        else link.removeAttribute('aria-current');
      }
    }
    const client = createClient({request: path => session.request(path), publish: render});
    const disclosureKey = 'lific:sidebar:recents-open';
    try {client.setOpen(win.sessionStorage.getItem(disclosureKey) === '1');}
    catch { /* Per-tab disclosure persistence is optional. */ }
    const activate = (force = false) => {
      const current = route();
      const privateScope = session.state.publicProject === null && session.state.user !== null &&
        !current.path.startsWith('/public/');
      const identity = privateScope ? `private:${session.state.user.id}` : 'anonymous';
      if (identity !== audience) {
        // Catalog generations are scoped to an account, just like rows.
        if (audience?.startsWith('private:')) catalog = null;
        audience = identity;
        routeKey = null;
        client.invalidate();
      }
      const match = current.path.match(/^\/([A-Za-z][A-Za-z0-9_-]*)\/([^/]+)(?:\/|$)/);
      const project = privateScope && match ? catalog?.projects.find(value =>
        value.identifier.toLowerCase() === match[1].toLowerCase()) : null;
      const section = match?.[2]?.toLowerCase() ?? null;
      const key = `${identity}:${project?.id ?? ''}:${current.key}`;
      if (!force && key === routeKey) return Promise.resolve();
      routeKey = key;
      return client.activate({project, section, public: !privateScope});
    };
    const onToggle = () => {
      client.setOpen(!client.peek().open);
      try {win.sessionStorage.setItem(disclosureKey, client.peek().open ? '1' : '0');}
      catch { /* Current disclosure stays usable without storage. */ }
    };
    const onRoute = () => {void activate();};
    const onEntry = () => {void activate(true);};
    const onSession = () => {routeKey = null; client.invalidate(); void activate();};
    const onCatalog = event => {
      if (catalog && event.detail.generation <= catalog.generation) return;
      catalog = event.detail;
      void activate();
    };
    const listeners = [['popstate', onRoute], ['hashchange', onRoute],
      ['lific:route-change', onEntry], ['lific:project-catalog', onCatalog],
      ['lific:account-change', onRoute], ['lific:scope-change', onRoute], ['lific:session-change', onSession]];
    toggle.addEventListener('click', onToggle);
    for (const [name, callback] of listeners) win.addEventListener(name, callback);
    render(client.peek());
    void activate();
    return {
      client,
      refresh: () => activate(true),
      dispose() {
        client.dispose();
        toggle.removeEventListener('click', onToggle);
        for (const [name, callback] of listeners) win.removeEventListener(name, callback);
      },
    };
  }
  globalThis.LificTopcoatRecents = {createClient, attach};
})();

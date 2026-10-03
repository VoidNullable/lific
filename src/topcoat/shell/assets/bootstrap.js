/* Connect the shell components to the session-scoped REST API. */
(() => {
  'use strict';

  function freezeCatalog(generation, projects, groups) {
    return Object.freeze({
      generation,
      projects: Object.freeze(projects.map(project => Object.freeze({...project}))),
      groups: Object.freeze(groups.map(group => Object.freeze({...group,
        project_ids: Object.freeze([...group.project_ids])}))),
    });
  }

  function privateIdentity(session) {
    const state = session?.state;
    return state?.publicProject === null && state.user && Number.isSafeInteger(state.user.id)
      ? `private:${state.user.id}` : null;
  }

  function responseData(response, label) {
    if (!response?.ok) throw new Error(response?.error || `Could not ${label}.`);
    return response.data;
  }

  function assertCurrent(isCurrent) {
    if (!isCurrent()) throw new Error('Project catalog session changed.');
  }

  function createCatalogAdapter(session, nextGeneration, isCurrent) {
    const request = (path, options) => session.request(path, options);
    return {
      scope: 'private',
      current: () => freezeCatalog(0, [], []),
      async fetch() {
        assertCurrent(isCurrent);
        const [projectResponse, groupResponse] = await Promise.all([
          request('/projects'), request('/project-groups'),
        ]);
        assertCurrent(isCurrent);
        const projects = responseData(projectResponse, 'load projects');
        const groups = responseData(groupResponse, 'load project groups');
        if (!Array.isArray(projects) || !Array.isArray(groups)) {
          throw new Error('Invalid project catalog response.');
        }
        return freezeCatalog(nextGeneration(), projects, groups);
      },
      async command(command) {
        assertCurrent(isCurrent);
        let path;
        let method;
        let body;
        switch (command?.type) {
          case 'reorder_projects':
            path = '/projects/reorder'; method = 'PUT'; body = {ids: command.ids}; break;
          case 'reorder_groups':
            path = '/project-groups/reorder'; method = 'PUT'; body = {ids: command.ids}; break;
          case 'assign_project':
            path = '/project-groups/assign'; method = 'PUT';
            body = {project_id: command.project_id, group_id: command.group_id}; break;
          case 'delete_group':
            path = `/project-groups/${encodeURIComponent(command.id)}`; method = 'DELETE'; break;
          case 'create_group':
            path = '/project-groups'; method = 'POST'; body = {name: command.name}; break;
          case 'rename_group':
            path = `/project-groups/${encodeURIComponent(command.id)}`;
            method = 'PATCH'; body = {name: command.name}; break;
          default: throw new Error('Unsupported project catalog command.');
        }
        const options = {method};
        if (body !== undefined) options.body = JSON.stringify(body);
        const response = await request(path, options);
        assertCurrent(isCurrent);
        const result = responseData(response, 'save project changes');
        const snapshot = await this.fetch();
        assertCurrent(isCurrent);
        return {result, snapshot};
      },
    };
  }

  function makeRecentsRoot(document, desktop, metadata) {
    let root = desktop.querySelector('[data-topcoat-recents]');
    if (root) return root;
    root = document.createElement('section');
    root.className = 'tc-recents';
    root.dataset.topcoatRecents = '';
    root.setAttribute('aria-label', 'Recent resources');
    root.hidden = true;
    if (metadata.identifier) root.dataset.projectIdentifier = metadata.identifier;
    if (metadata.projectId) root.dataset.projectId = metadata.projectId;
    if (metadata.section) root.dataset.recentSection = metadata.section;
    const toggle = document.createElement('button');
    toggle.type = 'button';
    toggle.className = 'tc-recents__heading';
    toggle.setAttribute('data-recents-toggle', '');
    toggle.setAttribute('aria-expanded', 'false');
    toggle.setAttribute('aria-controls', 'tc-sidebar-recents-list');
    const content = document.createElement('div');
    content.id = 'tc-sidebar-recents-list';
    content.setAttribute('data-recents-content', '');
    content.hidden = true;
    content.setAttribute('aria-busy', 'false');
    const status = document.createElement('p');
    status.className = 'tc-recents__status';
    status.setAttribute('data-recents-status', '');
    status.setAttribute('role', 'status');
    status.setAttribute('aria-live', 'polite');
    const error = document.createElement('p');
    error.className = 'tc-recents__error';
    error.setAttribute('data-recents-error', '');
    error.setAttribute('role', 'status');
    error.setAttribute('aria-live', 'polite');
    error.hidden = true;
    const list = document.createElement('ul');
    list.className = 'tc-recents__list';
    list.setAttribute('data-recents-list', '');
    content.append(status, list, error);
    root.append(toggle, content);
    desktop.append(root);
    return root;
  }

  function mobileCatalogGeneration(document) {
    try {
      const panel = document.querySelector('[data-mobile-navigation]');
      const generation = JSON.parse(panel?.dataset.mobileCatalog ?? '{}').generation;
      return Number.isSafeInteger(generation) && generation >= 0 ? generation : 0;
    } catch { return 0; }
  }

  function mount({window: win = globalThis.window, document: doc = win?.document,
    session = win?.lificSession, projects = win?.LificTopcoatProjects,
    recents = win?.LificTopcoatRecents} = {}) {
    const shell = doc?.querySelector('.tc-shell');
    const desktop = shell?.querySelector('.tc-shell__desktop');
    if (!desktop || !session || !projects?.attach || !recents?.attach) return null;

    let generation = mobileCatalogGeneration(doc);
    let requestGeneration = 0;
    let identity = null;
    let projectApp = null;
    let recentApp = null;
    let recentsRoot = null;
    let disposed = false;
    let projectRoot = desktop.querySelector('[data-topcoat-projects]');

    function clearPrivateComponents() {
      projectApp?.destroy();
      recentApp?.dispose();
      projectApp = null;
      recentApp = null;
      projectRoot?.remove();
      projectRoot = null;
      if (recentsRoot) {
        recentsRoot.remove();
        recentsRoot = null;
      }
    }

    function transition(force = false) {
      const nextIdentity = privateIdentity(session);
      if (!force && nextIdentity === identity && projectApp) return;
      const identityChanged = nextIdentity !== identity;
      identity = nextIdentity;
      const currentRequest = ++requestGeneration;
      clearPrivateComponents();
      if (identityChanged) {
        win.lificMobileNavigation?.setCatalog?.(freezeCatalog(++generation, [], []));
      }
      if (!identity) {
        return;
      }
      projectRoot = doc.createElement('div');
      projectRoot.dataset.topcoatProjects = '';
      projectRoot.className = 'tc-shell__project-tree';
      desktop.prepend(projectRoot);
      const currentIdentity = identity;
      const current = () => !disposed && requestGeneration === currentRequest &&
        identity === currentIdentity && privateIdentity(session) === currentIdentity;
      const adapter = createCatalogAdapter(session, () => ++generation, current);
      const route = win.location?.pathname ?? '/';
      const activeIdentifier = route.match(/^\/public\/([A-Za-z][A-Za-z0-9_-]*)\/(?:issues|pages)(?:\/|$)/i)?.[1] ??
        route.match(/^\/([A-Za-z][A-Za-z0-9_-]*)\/(?:overview|issues|graph|modules|pages|files|plans|activity|insights|board|settings)(?:\/|$)/i)?.[1] ?? null;
      projectApp = projects.attach(projectRoot, adapter, {
        activeIdentifier: activeIdentifier?.toUpperCase() ?? null,
      });
      const recentRoute = route.match(/^\/([A-Za-z][A-Za-z0-9_-]*)\/(issues|modules|pages|plans)(?:\/|$)/i);
      recentsRoot = makeRecentsRoot(doc, desktop, {
        identifier: recentRoute?.[1]?.toUpperCase() ?? null,
        section: recentRoute?.[2]?.toLowerCase() ?? null,
        projectId: doc.body?.dataset.lificProjectId || null,
      });
      recentApp = recents.attach(recentsRoot, {session, catalog: null, win});
    }

    const onCatalog = event => {
      if (identity && event.detail?.generation > 0) {
        generation = Math.max(generation, event.detail.generation);
        win.lificMobileNavigation?.setCatalog?.(event.detail);
      }
    };
    const onNavigate = event => {
      const detail = event.detail;
      if (typeof detail?.href !== 'string' || !detail.href.startsWith('/') || detail.href.startsWith('//') ||
          !['push', 'replace'].includes(detail.history)) return;
      let destination;
      try { destination = new URL(detail.href, win.location.href); }
      catch { return; }
      if (destination.origin !== win.location.origin) return;
      const publicProject = session.state.publicProject ??
        win.location.pathname.match(/^\/public\/([A-Za-z][A-Za-z0-9_-]*)(?:\/|$)/i)?.[1] ?? null;
      if (publicProject) {
        const path = destination.pathname.match(/^\/public\/([A-Za-z][A-Za-z0-9_-]*)(?:\/|$)/i);
        if (!path || path[1].toLowerCase() !== publicProject.toLowerCase()) return;
      }
      win.location[detail.history === 'replace' ? 'replace' : 'assign'](destination.href);
    };
    win.addEventListener('lific:project-catalog', onCatalog);
    win.addEventListener('lific:navigate', onNavigate);
    const listeners = ['lific:account-change', 'lific:session-change', 'lific:scope-change']
      .map(name => [name, () => transition()]);
    for (const [name, listener] of listeners) win.addEventListener(name, listener);
    transition();
    return {
      refresh: () => transition(true),
      dispose() {
        disposed = true;
        requestGeneration++;
        clearPrivateComponents();
        win.removeEventListener('lific:project-catalog', onCatalog);
        win.removeEventListener('lific:navigate', onNavigate);
        for (const [name, listener] of listeners) win.removeEventListener(name, listener);
      },
    };
  }

  const api = {mount, freezeCatalog, privateIdentity, createCatalogAdapter};
  if (typeof module !== 'undefined' && module.exports) module.exports = api;
  if (typeof window !== 'undefined') {
    window.LificTopcoatShellBootstrap = api;
    window.lificTopcoatShell?.dispose?.();
    window.lificTopcoatShell = mount();
  }
})();

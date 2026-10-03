(function (root) {
  'use strict';

  function subtabKey(view, projectId) {
    return `lific:subtab:${view}:${projectId}`;
  }

  function loadSubtab(view, projectId, valid, storage) {
    if (!projectId) return null;
    try {
      const target = storage || root.localStorage;
      const stored = target.getItem(subtabKey(view, projectId));
      return stored && valid.includes(stored) ? stored : null;
    } catch {
      return null;
    }
  }

  function saveSubtab(view, projectId, id, storage) {
    if (!projectId) return false;
    try {
      const target = storage || root.localStorage;
      target.setItem(subtabKey(view, projectId), id);
      return true;
    } catch {
      return false;
    }
  }

  function publicHrefIsScoped(href, project) {
    const path = href.startsWith('#') ? href.slice(1) : href;
    const prefix = `/public/${project}`;
    if (path === prefix) return true;
    const suffix = path.startsWith(prefix) ? path.slice(prefix.length) : '';
    return suffix.startsWith('/') || suffix.startsWith('?') || suffix.startsWith('#');
  }

  function routeFamily(path) {
    const segments = path.replace(/^\/public(?=\/)/, '').split('/');
    const family = segments[2];
    return family === 'issues' || family === 'board' ? 'issues' : family || 'other';
  }

  function shouldFade(from, to, reducedMotion) {
    return !reducedMotion && routeFamily(from) !== routeFamily(to);
  }

  const api = {subtabKey, loadSubtab, saveSubtab, publicHrefIsScoped, routeFamily, shouldFade};
  root.LificTopcoatPageChrome = api;
  if (typeof root.document === 'undefined') return;

  function selectTab(list, id) {
    for (const tab of list.querySelectorAll('[data-subtab-id]')) {
      const selected = tab.dataset.subtabId === id;
      tab.setAttribute('aria-selected', String(selected));
      tab.tabIndex = selected ? 0 : -1;
    }
  }

  for (const list of root.document.querySelectorAll('[data-subtabs]')) {
    const tabs = [...list.querySelectorAll('[data-subtab-id]')];
    const view = list.dataset.view;
    const projectId = list.dataset.projectId;
    const stored = loadSubtab(view, projectId, tabs.map(tab => tab.dataset.subtabId));
    if (stored) selectTab(list, stored);

    list.addEventListener('click', event => {
      const tab = event.target.closest('[data-subtab-id]');
      if (!tab || !list.contains(tab)) return;
      const id = tab.dataset.subtabId;
      selectTab(list, id);
      saveSubtab(view, projectId, id);
      list.dispatchEvent(new CustomEvent('lific:subtab-change', {
        bubbles: true, detail: {view, projectId, id},
      }));
    });

    list.addEventListener('keydown', event => {
      const tab = event.target.closest('[data-subtab-id]');
      if (!tab) return;
      const index = tabs.indexOf(tab);
      const next = event.key === 'ArrowRight' ? (index + 1) % tabs.length
        : event.key === 'ArrowLeft' ? (index + tabs.length - 1) % tabs.length
        : event.key === 'Home' ? 0
        : event.key === 'End' ? tabs.length - 1
        : -1;
      if (next < 0) return;
      event.preventDefault();
      tabs[next].focus();
      tabs[next].click();
    });
  }

  root.document.addEventListener('click', async event => {
    const copy = event.target.closest('[data-copy-identifier]');
    if (copy) {
      const value = copy.dataset.copyIdentifier;
      try {
        await root.navigator.clipboard.writeText(value);
        copy.dataset.copyState = 'copied';
        root.setTimeout(() => { delete copy.dataset.copyState; }, 1000);
      } catch {
        copy.dispatchEvent(new CustomEvent('lific:copy-error', {
          bubbles: true, detail: {value},
        }));
      }
      return;
    }
    const action = event.target.closest('[data-page-action-id]');
    if (action && action.tagName === 'BUTTON') {
      action.dispatchEvent(new CustomEvent('lific:page-action', {
        bubbles: true,
        detail: {
          id: action.dataset.pageActionId,
          command: action.dataset.pageActionCommand,
        },
      }));
    }
  });

  root.addEventListener('lific:navigate', event => {
    const destination = new URL(event.detail.href, root.location.href);
    const reducedMotion = root.matchMedia('(prefers-reduced-motion: reduce)').matches;
    if (!shouldFade(root.location.pathname, destination.pathname, reducedMotion)) return;
    const main = root.document.querySelector('.tc-shell__main');
    if (!main) return;
    main.classList.remove('tc-route-fade');
    void main.offsetWidth;
    main.classList.add('tc-route-fade');
    root.setTimeout(() => main.classList.remove('tc-route-fade'), 180);
  });
})(globalThis);

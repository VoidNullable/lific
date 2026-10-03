/* Keep existing #/ bookmarks while new navigation uses direct URLs. Load
 * this before the session bridge so public hash links stay anonymous. */
(() => {
  function restoreRoute() {
    if (!location.hash.startsWith('#/')) return;
    const route = location.hash.slice(1);
    const fragmentStart = route.indexOf('#');
    const pathAndQuery = fragmentStart < 0 ? route : route.slice(0, fragmentStart);
    const queryStart = pathAndQuery.indexOf('?');
    const path = queryStart < 0 ? pathAndQuery : pathAndQuery.slice(0, queryStart);
    // Assign URL components rather than resolving an untrusted //host link.
    const destination = new URL(location.href);
    destination.pathname = path;
    destination.search = queryStart < 0 ? '' : pathAndQuery.slice(queryStart);
    destination.hash = fragmentStart < 0 ? '' : route.slice(fragmentStart);
    const publicProject = path.match(/^\/public\/([A-Za-z][A-Za-z0-9_-]*)(?:\/|$)/)?.[1];
    if (publicProject && document.body) {
      document.body.dataset.lificPublicProject = publicProject.toUpperCase();
      document.body.dataset.lificRequireSession = 'false';
    }
    location.replace(destination.href);
  }
  restoreRoute();
  window.addEventListener('hashchange', restoreRoute);

  const shell = document.querySelector('.tc-shell');
  // Auth and standalone experiment screens have no docked navigation.
  if (!shell || shell.dataset.layout === 'auth') return;
  const toggle = shell.querySelector('[data-sidebar-toggle]');
  const handle = shell.querySelector('[data-sidebar-resize]');

  const widthKey = 'lific:sidebar:width';
  const collapsedKey = 'lific:sidebar:collapsed';
  const clamp = (value, min, max) => Math.min(max, Math.max(min, value));
  function read(key) {
    try { return localStorage.getItem(key); } catch { return null; }
  }
  function persist(key, value) {
    try {
      if (value === null) localStorage.removeItem(key);
      else localStorage.setItem(key, value);
    } catch {
      // Preference storage is optional; current navigation stays usable.
    }
  }
  const storedWidth = read(widthKey);
  let preferred = storedWidth !== null && storedWidth.trim() && Number.isFinite(Number(storedWidth))
    ? clamp(Number(storedWidth), 180, 400) : null;
  let collapsed = read(collapsedKey) === '1';
  let metrics;
  function size(fontSize) {
    const scale = Number.isFinite(fontSize) && fontSize > 0 ? fontSize / 16 : 1;
    const min = 180 * Math.max(1, scale);
    const max = Math.max(400, min);
    metrics = {min, max, width: clamp(preferred ?? 230 * scale, min, max)};
    shell.style.setProperty('--tc-sidebar-width', `${metrics.width}px`);
    handle.setAttribute('aria-valuemin', min);
    handle.setAttribute('aria-valuemax', max);
    handle.setAttribute('aria-valuenow', metrics.width);
  }
  const fontSize = () => parseFloat(getComputedStyle(document.documentElement).fontSize);
  const renderCollapsed = () => {
    shell.dataset.sidebarCollapsed = String(collapsed);
    toggle.setAttribute('aria-expanded', String(!collapsed));
    const label = collapsed ? 'Expand sidebar' : 'Collapse sidebar';
    toggle.setAttribute('aria-label', label);
    toggle.textContent = label;
  };
  size(fontSize());
  renderCollapsed();
  new ResizeObserver(([entry]) => {
    // A hidden probe must not discard a temporary text-size constraint.
    if (entry.contentRect.width > 0) size(entry.contentRect.width);
  }).observe(shell.querySelector('[data-sidebar-probe]'));
  toggle.addEventListener('click', () => {
    collapsed = !collapsed;
    persist(collapsedKey, collapsed ? '1' : '0');
    renderCollapsed();
  });
  handle.addEventListener('keydown', event => {
    const delta = event.key === 'ArrowLeft' ? -10 : event.key === 'ArrowRight' ? 10 : 0;
    if (delta === 0) return;
    event.preventDefault();
    preferred = clamp(metrics.width + delta, metrics.min, metrics.max);
    size(fontSize());
    persist(widthKey, String(preferred));
  });
  handle.addEventListener('dblclick', () => {
    preferred = null;
    persist(widthKey, null);
    size(fontSize());
  });
  let drag = null;
  handle.addEventListener('pointerdown', event => {
    if (drag !== null || event.button !== 0 || !event.isPrimary) return;
    event.preventDefault();
    handle.setPointerCapture(event.pointerId);
    drag = {id: event.pointerId, x: event.clientX, width: metrics.width, changed: false,
      cursor: document.body.style.cursor, userSelect: document.body.style.userSelect};
    document.body.style.cursor = 'col-resize';
    document.body.style.userSelect = 'none';
  });
  handle.addEventListener('pointermove', event => {
    if (drag?.id !== event.pointerId) return;
    const width = clamp(drag.width + event.clientX - drag.x, metrics.min, metrics.max);
    if (width !== metrics.width) {
      preferred = width;
      drag.changed = true;
      size(fontSize());
    }
  });
  function finish(event) {
    if (drag?.id !== event.pointerId) return;
    const finished = drag;
    drag = null;
    document.body.style.cursor = finished.cursor;
    document.body.style.userSelect = finished.userSelect;
    if (handle.hasPointerCapture(event.pointerId)) handle.releasePointerCapture(event.pointerId);
    if (finished.changed) persist(widthKey, String(preferred));
  }
  handle.addEventListener('pointerup', finish);
  handle.addEventListener('pointercancel', finish);
  handle.addEventListener('lostpointercapture', finish);
})();

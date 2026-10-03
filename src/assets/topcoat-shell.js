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
})();

export type MobileNavEntry = {
  version: 1;
  session: string;
  depth: 0 | 1 | 2;
  href: string;
  project: string | null;
};

const KEY = "lificMobileNav";
const SESSION = /^[0-9a-f]{8}-[0-9a-f]{4}-4[0-9a-f]{3}-[89ab][0-9a-f]{3}-[0-9a-f]{12}$/i;
const PROJECT = /^[A-Za-z][A-Za-z0-9_-]*$/;

function sessionId(): string {
  if (typeof crypto.randomUUID === "function") return crypto.randomUUID();
  // randomUUID requires a secure context, but local-first LAN HTTP installs
  // still expose getRandomValues. Generate the same version-4 shape there.
  const bytes = crypto.getRandomValues(new Uint8Array(16));
  bytes[6] = (bytes[6] & 15) | 64;
  bytes[8] = (bytes[8] & 63) | 128;
  const hex = Array.from(bytes, (b) => b.toString(16).padStart(2, "0")).join("");
  return `${hex.slice(0, 8)}-${hex.slice(8, 12)}-${hex.slice(12, 16)}-${hex.slice(16, 20)}-${hex.slice(20)}`;
}

/** Versioned, namespaced history state persists ownership across remounts and
 * reloads without a second registry in memory or storage. This is UI state,
 * not an authorization boundary: project availability comes from live props.
 * Keep closed sessions recognizable: Forward must restore their actual pane.
 * Only a popstate at the exact base entry can release a queued action.
 */
export function createMobileNavHistory(render: (entry: MobileNavEntry | null) => void) {
  let pending: { session: string; href: string; action?: () => void } | null = null;
  let traversing = false;

  function current(): MobileNavEntry | null {
    const entry: unknown = history.state?.[KEY];
    if (!entry || typeof entry !== "object" || Array.isArray(entry)) return null;
    const value = entry as Record<string, unknown>;
    if (value.version !== 1 || typeof value.session !== "string" || !SESSION.test(value.session) ||
        value.href !== location.href || ![0, 1, 2].includes(value.depth as number)) return null;
    if (value.depth === 2 ? typeof value.project !== "string" || !PROJECT.test(value.project) : value.project !== null) return null;
    return value as MobileNavEntry;
  }

  function write(entry: MobileNavEntry, replace = false) {
    history[replace ? "replaceState" : "pushState"](
      { ...history.state, [KEY]: entry }, "", entry.href,
    );
  }

  function onPop() {
    traversing = false;
    const entry = current();
    if (pending) {
      const queued = pending;
      pending = null;
      render(null);
      if (entry?.session === queued.session && entry.depth === 0 &&
          location.href === queued.href) queued.action?.();
      else render(entry?.depth ? entry : null);
      return;
    }
    render(entry?.depth ? entry : null);
  }

  function onHash() {
    // Hash navigation outside the drawer wins over a queued destination.
    if (!current()) {
      pending = null;
      render(null);
    }
  }

  window.addEventListener("popstate", onPop);
  window.addEventListener("hashchange", onHash);

  return {
    // Call after assigning the controller, so render may request a desktop
    // close. No entries are pushed when restoring an existing pane.
    restore() {
      const entry = current();
      if (!entry?.depth) return false;
      render(entry);
      return true;
    },
    open(project: string | null) {
      if (traversing) return;
      let entry = current();
      if (!project && entry?.depth === 2) {
        traversing = true;
        history.back();
        return;
      }
      if (!entry || entry.depth === 0) {
        entry = { version: 1, session: sessionId(), depth: 0, href: location.href, project: null };
        write(entry, true);
        entry = { ...entry, depth: 1 };
        write(entry);
      }
      if (project) {
        const replace = entry.depth === 2;
        entry = { ...entry, depth: 2, project };
        write(entry, replace);
      }
      render(entry);
    },
    back() {
      if (traversing) return;
      if (current()?.depth) {
        traversing = true;
        history.back();
      } else render(null);
    },
    close(action?: () => void) {
      if (traversing) return;
      const entry = current();
      render(null);
      if (!entry?.depth) { action?.(); return; }
      pending = { session: entry.session, href: entry.href, action };
      traversing = true;
      history.go(-entry.depth);
    },
    routeChanged() {
      pending = null;
      // A Forward traversal may also change the underlying route. In that
      // case onPop has already restored a valid owned pane at this URL.
      const entry = current();
      if (!entry?.depth) render(null);
    },
    destroy() {
      pending = null;
      window.removeEventListener("popstate", onPop);
      window.removeEventListener("hashchange", onHash);
    },
  };
}

// LIF-471: the public-view switch.
//
// While a `#/public/{PROJECT}/...` route is showing, the whole client runs in
// "public scope": the ordinary components (IssueList, IssueDetail, PageList,
// PageDetail) render exactly as they do for a signed-in reader, but every
// request they make is rewritten onto the anonymous `/public/api` surface
// with no credential attached, every route they build is prefixed with
// `/public`, and every capability check answers "read-only".
//
// This is a plain module variable rather than a rune so `api.ts` (plain TS,
// consulted at request time) and `references.ts` can read it without a
// reactive dependency. `App.svelte` sets it synchronously whenever the route
// changes and before the public branch mounts, so no child ever sees the
// wrong answer.
//
// It is deliberately a *scope*, not a mode: a signed-in maintainer who opens
// a public link sees the same page a stranger does, which is the only way the
// public view can be trusted to look the same to everyone.

let publicProject: string | null = null;
const listeners = new Set<(project: string | null) => void>();

/** The identifier of the project the public view is showing, or null. */
export function getPublicProject(): string | null {
  return publicProject;
}

export function inPublicScope(): boolean {
  return publicProject !== null;
}

/** Set by the router. Notifies the caches that key on audience (read models,
 *  role store) so private state never bleeds into the public view or back. */
export function setPublicProject(project: string | null): void {
  const next = project === null ? null : project.toUpperCase();
  if (next === publicProject) return;
  publicProject = next;
  for (const listener of listeners) listener(next);
}

/** Subscribe to scope transitions. Returns an unsubscribe. */
export function onPublicScopeChange(
  listener: (project: string | null) => void,
): () => void {
  listeners.add(listener);
  return () => listeners.delete(listener);
}

/** Prefix an in-app route (`/LIF/issues/LIF-42`) for the current scope. A
 *  route that is already public, or an auth route, is left alone. */
export function scopedRoute(route: string): string {
  if (publicProject === null) return route;
  if (route.startsWith("/public/")) return route;
  const normalized = route.startsWith("/") ? route : `/${route}`;
  // A project's overview is signed-in only; in the public view the project's
  // "home" is its issue list, which is where the project crumb should land.
  const overview = normalized.match(/^\/([A-Za-z][A-Za-z0-9_-]*)\/(?:overview|settings)$/);
  if (overview) return `/public/${overview[1]}/issues`;
  return `/public${normalized}`;
}

/** `scopedRoute` for an `href` that already carries the hash. */
export function scopedHref(href: string): string {
  return href.startsWith("#/") ? `#${scopedRoute(href.slice(1))}` : href;
}

/** The public mirror of a private `/api` path (the part after `/api`), or
 *  null when the path has no public counterpart.
 *
 *  The mirror rule, matching `src/api/public.rs`: the public path is
 *  `/public/api/projects/{P}` followed by the private path minus `/api`, with
 *  the project-scoping query parameters dropped because the project is already
 *  in the path. Anything not listed here is refused by the caller rather than
 *  sent to `/api` with a credential. */
export function publicMirror(path: string): string | null {
  const project = publicProject;
  if (project === null) return null;
  const base = `/public/api/projects/${encodeURIComponent(project)}`;
  const [pathname, search = ""] = path.split("?");
  const query = new URLSearchParams(search);

  // The project list collapses to the one published project; the caller
  // wraps the single object back into an array.
  if (pathname === "/projects") return base;

  let m: RegExpMatchArray | null;
  if ((m = pathname.match(/^\/projects\/\d+\/(index|changes)$/))) {
    return `${base}/${m[1]}${search ? `?${search}` : ""}`;
  }
  if (/^\/(modules|labels|folders)$/.test(pathname) && query.has("project_id")) {
    return `${base}${pathname}`;
  }
  if ((m = pathname.match(/^\/issues\/resolve\/([A-Za-z0-9_-]+)$/))) {
    return `${base}/issues/resolve/${m[1]}`;
  }
  if ((m = pathname.match(/^\/issues\/(\d+)(\/comments)?$/))) {
    return `${base}/issues/${m[1]}${m[2] ?? ""}${search ? `?${search}` : ""}`;
  }
  if ((m = pathname.match(/^\/pages\/(\d+)(\/comments)?$/))) {
    return `${base}/pages/${m[1]}${m[2] ?? ""}${search ? `?${search}` : ""}`;
  }
  if (pathname === "/attachments" && query.has("entity_type") && query.has("entity_id")) {
    return `${base}/attachments?${search}`;
  }
  if ((m = pathname.match(/^\/attachments\/(\d+)(\/thumbnail|\/preview)?$/))) {
    return `${base}/attachments/${m[1]}${m[2] ?? ""}`;
  }
  return null;
}

/** Reads the public view answers locally instead of asking the server: the
 *  shape the components expect, with nothing behind it. `undefined` means
 *  "not synthesized here"; the caller then tries `publicMirror`. */
export function publicSynthetic(path: string): { status: number; body: unknown } | undefined {
  const pathname = path.split("?")[0];
  // No account. Components treat a 401 here as "not signed in".
  if (pathname === "/auth/me") return { status: 401, body: { error: "not signed in" } };
  // A stranger is below Viewer with enforcement on: nothing may be changed
  // and nothing may be said.
  if (/^\/projects\/\d+\/my-role$/.test(pathname)) {
    return { status: 200, body: { role: null, enforced: true, is_admin: false } };
  }
  // History is not public. The timeline simply has nothing in it.
  if (/^\/(issues|pages)\/\d+\/activity$/.test(pathname)) {
    return { status: 200, body: { items: [], has_more: false } };
  }
  if (/^\/projects\/\d+\/mention-candidates$/.test(pathname)) return { status: 200, body: [] };
  if (/^\/projects\/\d+\/views$/.test(pathname)) return { status: 200, body: [] };
  return undefined;
}

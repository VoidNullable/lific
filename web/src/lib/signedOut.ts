// LIF-583: reading a published project without an account.
//
// A published project is readable at its ordinary links by anyone; the same
// Layout and routes serve a signed-out reader, with every way of changing
// something hidden. This module holds the one fact those components need:
// is this tab signed out? `App.svelte` sets it from the session check once
// bootstrap (including single-user auto-login) has settled, and again on
// every session change.
//
// A plain module variable rather than a rune so plain TS (the read-model
// registry, the role store, Markdown's sanitizer hooks) can read it at call
// time. Subscribers hear about transitions so caches keyed on the audience
// can drop what they hold.

import { commentTargetFromHash, routeWithCommentTarget, splitResourcePath } from "./commentLinks";
import { loginRouteFor } from "./loginRedirect";

let signedOut = false;
const listeners = new Set<(signedOut: boolean) => void>();

export function isSignedOut(): boolean {
  return signedOut;
}

export function setSignedOut(next: boolean): void {
  if (next === signedOut) return;
  signedOut = next;
  for (const listener of listeners) listener(next);
}

/** Subscribe to transitions. Returns an unsubscribe. */
export function onSignedOutChange(listener: (signedOut: boolean) => void): () => void {
  listeners.add(listener);
  return () => listeners.delete(listener);
}

/** The project pages a signed-out reader may open. Everything else (the
 *  workspace views, settings, creation) goes to login. */
export const SIGNED_OUT_PAGES: ReadonlySet<string> = new Set([
  "project-overview",
  "issues",
  "board",
  "graph",
  "issue-detail",
  "pages",
  "page-detail",
  "plans",
  "plan-detail",
  "modules",
  "module-detail",
  "activity",
  "insights",
  "files",
]);

/** Where an old public-view route now lives, or null when `route` is not
 *  one. The separate `/public/...` view is gone; a published project is read
 *  at its normal links, so `/public/LIF` opens the issue list, the LIF-465
 *  short form `/public/LIF/LIF-42` opens that issue, and anything else just
 *  loses the prefix. A query string (`?comment=7`) is kept. */
export function rewritePublicRoute(route: string): string | null {
  const queryStart = route.indexOf("?");
  const path = queryStart < 0 ? route : route.slice(0, queryStart);
  const query = queryStart < 0 ? "" : route.slice(queryStart);
  const match = path.match(/^\/public\/([A-Za-z][A-Za-z0-9_-]*)(\/.*)?$/);
  if (!match) return null;
  const [, project, rest = ""] = match;
  if (rest === "" || rest === "/") return `/${project}/issues${query}`;
  const short = rest.match(/^\/([A-Za-z][A-Za-z0-9_-]*-\d+)\/?$/);
  if (short) return `/${project}/issues/${short[1]}${query}`;
  return `/${project}${rest}${query}`;
}

/** The route this tab is showing, read from the address bar. A path-style
 *  link keeps its route in the path and a comment target in the fragment
 *  (see App.svelte), so both are folded back together. */
export function currentRoute(loc: { pathname: string; hash: string } = window.location): string {
  if (loc.hash.startsWith("#/")) return loc.hash.slice(1);
  const route = splitResourcePath(loc.pathname)?.route ?? "/";
  const target = commentTargetFromHash(loc.hash);
  return target ? routeWithCommentTarget(route, target) : route;
}

/** The login route that comes back to the current page afterwards. */
export function signInRoute(loc?: { pathname: string; hash: string }): string {
  return loginRouteFor(currentRoute(loc));
}

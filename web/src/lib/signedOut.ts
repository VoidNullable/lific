// LIF-583: reading a published project without an account.
//
// A published project is readable at its ordinary links by anyone; the same
// Layout and routes serve a signed-out reader, with every way of changing
// something hidden. This module holds the one fact those components need:
// is this tab signed out? Its answer comes from the account confirmed by
// /api/auth/me. App.svelte waits for that check, including after a cookie
// changes in another tab, before rendering the new reader's routes.
//
// Plain functions let the read-model registry, role store and Markdown hooks
// read the same account state. Subscribers hear about audience transitions.

import { commentTargetFromHash, routeWithCommentTarget, splitResourcePath } from "./commentLinks";
import { loginRouteFor } from "./loginRedirect";
import { onReadModelAccountChange, readModelAccount, setReadModelAccount } from "./sync/account";

const listeners = new Set<(signedOut: boolean) => void>();

export function isSignedOut(): boolean {
  return readModelAccount() === null;
}

export function setSignedOut(next: boolean): void {
  if (next) setReadModelAccount(null);
  else if (isSignedOut()) setReadModelAccount(undefined);
}

let previous = isSignedOut();
onReadModelAccountChange(() => {
  const next = isSignedOut();
  if (next === previous) return;
  previous = next;
  for (const listener of listeners) listener(next);
});

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

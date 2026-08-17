// LIF-418: line deep links into a text attachment.
//
// The anchor token is `att{id}-L{start}` for one line and
// `att{id}-L{start}-{end}` for a range, e.g. `att12-L340-360`.
//
// Two shapes carry it, because this app is hash-routed and a URL has exactly
// one fragment:
//
//   1. Path style, what "copy link" produces:
//        https://host/LIF/issues/LIF-42#att12-L340-360
//      The server SPA-fallbacks the path (LIF-247) and App.svelte keeps the
//      path as the initial route precisely when a non-route fragment is
//      present, so the fragment survives the cold load. This is the shape the
//      spec asks for and the one that gets pasted into chat.
//
//   2. Hash-route style, what in-app selection writes back:
//        https://host/#/LIF/issues/LIF-42?att=att12-L340-360
//      Overwriting the fragment in place would throw away the route and leave
//      a reload pointing at a 404, so the token rides as a query parameter on
//      the hash route instead. Same convention the comment anchors use
//      (`?comment=42`, see commentLinks.ts).
//
// Both parse back to the same target.

export interface LineTarget {
  attachmentId: number;
  /** 1-based first line of the selection. */
  start: number;
  /** 1-based last line; equals `start` for a single-line target. */
  end: number;
}

const ANCHOR_RE = /^#?att([1-9]\d*)-L([1-9]\d*)(?:-([1-9]\d*))?$/;

/** Build the anchor token for a line or range. Order-insensitive: shift-
 *  selecting upward still produces `L{low}-L{high}`. */
export function formatLineAnchor(
  attachmentId: number,
  start: number,
  end?: number,
): string {
  const lo = end === undefined ? start : Math.min(start, end);
  const hi = end === undefined ? start : Math.max(start, end);
  return hi > lo ? `att${attachmentId}-L${lo}-${hi}` : `att${attachmentId}-L${lo}`;
}

/** Parse a token, with or without its leading `#`. */
export function parseLineAnchor(token: string): LineTarget | null {
  const match = token.match(ANCHOR_RE);
  if (!match) return null;
  const start = Number(match[2]);
  const end = match[3] === undefined ? start : Number(match[3]);
  return {
    attachmentId: Number(match[1]),
    start: Math.min(start, end),
    end: Math.max(start, end),
  };
}

/** Read a target out of `window.location.hash`, in either shape. */
export function lineTargetFromHash(hash: string): LineTarget | null {
  if (!hash.startsWith("#/")) return parseLineAnchor(hash);
  const queryStart = hash.indexOf("?");
  if (queryStart < 0) return null;
  const token = new URLSearchParams(hash.slice(queryStart + 1)).get("att");
  return token ? parseLineAnchor(token) : null;
}

function splitRoute(route: string): { path: string; query: URLSearchParams } {
  const queryStart = route.indexOf("?");
  return {
    path: queryStart < 0 ? route : route.slice(0, queryStart),
    query: new URLSearchParams(queryStart < 0 ? "" : route.slice(queryStart + 1)),
  };
}

function joinRoute(path: string, query: URLSearchParams): string {
  const rest = query.toString();
  return rest ? `${path}?${rest}` : path;
}

/** Attach the token to a hash route as `?att=`, preserving other params. */
export function routeWithLineTarget(route: string, token: string): string {
  const { path, query } = splitRoute(route);
  query.set("att", token);
  return joinRoute(path, query);
}

/** Drop the `att` parameter from a hash route. */
export function routeWithoutLineTarget(route: string): string {
  const { path, query } = splitRoute(route);
  query.delete("att");
  return joinRoute(path, query);
}

/** The value to hand `history.replaceState` for the current location, so the
 *  URL carries `token` without losing the route. Pass `null` to clear. */
export function hashWithLineTarget(hash: string, token: string | null): string {
  if (hash.startsWith("#/")) {
    const route = hash.slice(1);
    return `#${token ? routeWithLineTarget(route, token) : routeWithoutLineTarget(route)}`;
  }
  return token ? `#${token}` : "";
}

export interface LinkLocation {
  origin: string;
  pathname: string;
  search: string;
  hash: string;
}

/** Absolute, pasteable URL for a target: always the path style, because that
 *  is the shape that survives being opened in a fresh tab. */
export function fullLineLink(token: string, loc: LinkLocation): string {
  if (loc.hash.startsWith("#/")) {
    const route = routeWithoutLineTarget(loc.hash.slice(1));
    const base = loc.pathname === "/" ? "" : loc.pathname.replace(/\/+$/, "");
    return `${loc.origin}${base}${route}#${token}`;
  }
  const path = loc.pathname === "" ? "/" : loc.pathname;
  return `${loc.origin}${path}${loc.search}#${token}`;
}

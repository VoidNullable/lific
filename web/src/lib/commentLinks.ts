const COMMENT_ANCHOR = /^#comment-([1-9]\d*)$/;
const COMMENT_TARGET = /^comment-([1-9]\d*)$/;
const RESOURCE_PATH =
  /^(.*)(\/[A-Za-z][A-Za-z0-9_-]*\/(?:overview|issues\/[A-Za-z][A-Za-z0-9_-]*-\d+|pages\/\d+|plans\/\d+|modules\/\d+))\/?$/i;

export function splitResourcePath(
  pathname: string,
): { basePath: string; route: string } | null {
  const match = pathname.match(RESOURCE_PATH);
  return match ? { basePath: match[1], route: match[2] } : null;
}

export function commentTargetFromHash(hash: string): string | null {
  const anchor = hash.match(COMMENT_ANCHOR);
  if (anchor) return `comment-${anchor[1]}`;
  if (!hash.startsWith("#/")) return null;

  const queryStart = hash.indexOf("?");
  if (queryStart < 0) return null;
  const id = new URLSearchParams(hash.slice(queryStart + 1)).get("comment");
  return id && /^[1-9]\d*$/.test(id) ? `comment-${id}` : null;
}

export function routeForCommentHash(hash: string, currentRoute: string): string {
  return hash.startsWith("#/") ? hash.slice(1) || "/" : currentRoute;
}

export function routeWithCommentTarget(route: string, target: string): string {
  const match = target.match(COMMENT_TARGET);
  if (!match) return route;

  const queryStart = route.indexOf("?");
  const path = queryStart < 0 ? route : route.slice(0, queryStart);
  const query = new URLSearchParams(queryStart < 0 ? "" : route.slice(queryStart + 1));
  query.set("comment", match[1]);
  return `${path}?${query}`;
}

/** Remembering where a signed-out visitor was going.
 *
 *  A link into the app opened without a session (a bookmark, a link from a
 *  chat, a tab whose session expired) used to land on `/login` and then on
 *  Home, dropping the page that was asked for. The login route now carries it
 *  as `?next=`, and signing in (or signing up) continues there.
 *
 *  Routes are hash routes, so `next` can only ever move the app between its
 *  own views. It is still checked: only an in-app path is accepted, never the
 *  auth screens themselves, so a crafted link cannot loop or smuggle in
 *  anything that is not a route.
 */

const AUTH_PATHS = new Set(["/login", "/signup"]);

/** The `next` value a route should be remembered as, or null when there is
 *  nothing worth remembering (Home). An auth screen passes its own `next` on,
 *  so a late bounce to login (a request from the page being left that 401s
 *  after the redirect) does not wipe what the first one remembered. */
function rememberable(route: string): string | null {
  const path = route.split("?")[0];
  if (path === "/" || path === "") return null;
  if (AUTH_PATHS.has(path)) {
    const next = nextRouteFrom(route);
    return next === "/" ? null : next;
  }
  return safeNextRoute(route);
}

/** Accept `next` only when it is an app route: a single leading slash, no
 *  scheme or protocol-relative form, no backslashes or control characters,
 *  and not one of the auth screens. */
export function safeNextRoute(next: string | null | undefined): string | null {
  if (!next || next.length > 2048) return null;
  if (!next.startsWith("/") || next.startsWith("//")) return null;
  if (/[\\\u0000-\u001f\u007f]/.test(next)) return null;
  if (AUTH_PATHS.has(next.split("?")[0])) return null;
  return next;
}

/** The auth route that returns to `route` afterwards. */
export function authRouteFor(
  page: "login" | "signup",
  route: string | null | undefined,
): string {
  const next = route ? rememberable(route) : null;
  return next ? `/${page}?${new URLSearchParams({ next })}` : `/${page}`;
}

/** The login route that returns to `route` once signed in. */
export function loginRouteFor(route: string | null | undefined): string {
  return authRouteFor("login", route);
}

/** Where to go after signing in from `authRoute` (`/login?next=...`). */
export function nextRouteFrom(authRoute: string): string {
  const queryStart = authRoute.indexOf("?");
  if (queryStart < 0) return "/";
  const next = new URLSearchParams(authRoute.slice(queryStart + 1)).get("next");
  return safeNextRoute(next) ?? "/";
}

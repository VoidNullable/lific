// Document tab title ("browser tab / bookmark / history entry") formatting
// and the override slot the per-route components register into.
//
// The format lives only here: parts are joined with " · ", a falsy part is
// dropped, the app name always closes the string, and any single part is
// truncated so one long issue title cannot push the identifier (the most
// scannable token, per the front-load rule) out of a clipped tab.
//
// Callers never learn any of that. A list route says pageTitle("Issues",
// project); a detail route says pageTitle(identifier, title, project) once
// its data lands — through the `lific:title` context App.svelte provides,
// with the same set-on-load / clear-on-unmount lifecycle as `lific:topbar`.

/** Max characters kept from a single part before it is cut. */
const MAX_PART = 60;
/** Separates the parts of a title. */
const SEPARATOR = " · ";
/** The instance name that always closes the title. */
const APP_NAME = "Lific";

function cap(part: string): string {
  return part.length > MAX_PART ? part.slice(0, MAX_PART - 1).trimEnd() + "…" : part;
}

/** Compose a full document title from ordered parts. Falsy parts drop out,
 *  so callers can pass optional data (`pageTitle(issue?.title, identifier)`)
 *  without branching. */
export function pageTitle(...parts: (string | false | null | undefined)[]): string {
  const kept = parts
    .filter((p): p is string => Boolean(p && p.trim()))
    .map((p) => cap(p.trim()));
  kept.push(APP_NAME);
  return kept.join(SEPARATOR);
}

/** Context key for the per-route title override. App.svelte provides
 *  `{ set(title?: string) }`; a route sets its composed title when its data
 *  arrives. There is nothing to clean up: App.svelte resets the slot on
 *  every route change, so a route never clears and cannot leak its title
 *  into the next page. */
export const PAGE_TITLE_CONTEXT = "lific:title";

// ── Route-derived defaults ───────────────────────────────────
//
// The default title for a route, used until (or unless) the route's own
// component registers a richer one. Lists and top-level pages only ever
// get this; detail pages upgrade themselves. Lives here, not in App.svelte,
// so the whole title vocabulary (format + defaults) is testable through one
// interface.
//
// The input is a structural subset of the router's parsed route — wide
// `page`/`type` fields rather than App.svelte's private discriminated union,
// so App can pass its route without exporting the type. An unrecognized
// page degrades to the bare app name.

export interface RouteTitleInput {
  type: "auth" | "public" | "public-redirect" | "app" | "loading" | "not-found";
  page?: string;
  project?: string;
  identifier?: string;
  /** The `public` wrapper carries the page it shows. */
  inner?: RouteTitleInput;
}

const SECTION_LABELS: Record<string, string> = {
  issues: "Issues",
  board: "Board",
  graph: "Graph",
  pages: "Pages",
  files: "Files",
  plans: "Plans",
  modules: "Modules",
  activity: "Activity",
  insights: "Insights",
};

/** Title for a parsed route. Public routes resolve to their inner page so
 *  a stranger's tab reads the same as a signed-in maintainer's. */
export function routePageTitle(p: RouteTitleInput): string {
  if (p.type === "public" && p.inner) return routePageTitle(p.inner);
  if (p.type === "auth") {
    return pageTitle(p.page === "login" ? "Sign in" : "Create account");
  }
  if (p.type === "not-found") return pageTitle("Not found");
  if (p.type !== "app") return pageTitle();

  const project = p.project;
  switch (p.page) {
    case "home":
      return pageTitle("Home");
    case "settings":
      return pageTitle("Settings");
    case "instance-settings":
      return pageTitle("Instance settings");
    case "project-new":
      return pageTitle("New project");
    case "project-import":
      return pageTitle("Import project");
    case "project-settings":
      return pageTitle("Settings", project);
    case "issue-new":
      return pageTitle("New issue", project);
    case "issue-detail":
      return pageTitle(p.identifier, project);
    case "page-detail":
      return pageTitle("Page", project);
    case "module-detail":
      return pageTitle("Module", project);
    case "plan-detail":
      return pageTitle("Plan", project);
    default: {
      const label = p.page ? SECTION_LABELS[p.page] : undefined;
      return label ? pageTitle(label, project) : pageTitle();
    }
  }
}

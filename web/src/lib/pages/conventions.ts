// LIF-524: a project's conventions page in the web UI.
//
// A project has at most one conventions page, and it lives on the project's
// Conventions tab rather than in the Pages list. Until it holds any text the
// tab offers the templates instead of the page.

import type { ConventionTemplate, ConventionTemplateKey } from "../api";
import { commentTargetFromHash, routeWithCommentTarget, splitResourcePath } from "../commentLinks";

/** The fields of a page row this decision reads. */
export interface ConventionsCandidate {
  id: number;
  preview: string;
  conventions?: boolean;
}

export type ConventionsState<T extends ConventionsCandidate> =
  /** No conventions page: the templates, Blank among them. */
  | { kind: "none" }
  /** A conventions page with no text yet: the templates fill it in. */
  | { kind: "empty"; page: T }
  /** A conventions page with text: show it. */
  | { kind: "page"; page: T };

/** `preview` is the first non-empty line of the body, so an empty preview
 *  means the body is empty or whitespace. */
export function conventionsState<T extends ConventionsCandidate>(rows: readonly T[]): ConventionsState<T> {
  const page = rows.find((row) => row.conventions === true);
  if (!page) return { kind: "none" };
  return page.preview.trim() === "" ? { kind: "empty", page } : { kind: "page", page };
}

/** The page title a conventions page is created with. */
export const CONVENTIONS_TITLE = "Conventions";

/** The project's Conventions tab. */
export function conventionsRoute(projectIdentifier: string): string {
  return `/${projectIdentifier}/conventions`;
}

/** Where a page opens: the conventions page on its tab, any other page in
 *  Pages. */
export function pageRoute(projectIdentifier: string, page: { id: number; conventions?: boolean }): string {
  return page.conventions === true
    ? conventionsRoute(projectIdentifier)
    : `/${projectIdentifier}/pages/${page.id}`;
}

/** The server calls the empty template "Empty"; the web UI offers it as
 *  Blank, the start you write yourself. */
export function templateName(t: Pick<ConventionTemplate, "key" | "name">): string {
  return t.key === "empty" ? "Blank" : t.name;
}

/** A few words on who runs the tracker under each template. Shorter than the
 *  server's summary, which restates the text's opening paragraph; that text
 *  sits right beside the list. */
const TAGLINES: Partial<Record<ConventionTemplateKey, string>> = {
  empty: "Write your own",
  ai_managed: "Agents run the tracker",
  human_managed: "People file, agents close",
  human_owned: "People file and close",
};

export function templateTagline(t: Pick<ConventionTemplate, "key" | "summary">): string {
  return TAGLINES[t.key] ?? t.summary;
}

/** Turn the conventions page's old address into its tab, keeping a comment
 *  target in the form the router reads (`?comment=N`). `hash` may be a hash
 *  route (`#/P/pages/3?comment=42`) or, on a path-style link
 *  (`/P/pages/3#comment-42`), a bare anchor; the path is dropped then, as
 *  App's boot code does. */
export function conventionsRedirect(
  projectIdentifier: string,
  loc: { pathname: string; hash: string },
): { route: string; url: string } {
  const target = commentTargetFromHash(loc.hash);
  const route = target
    ? routeWithCommentTarget(conventionsRoute(projectIdentifier), target)
    : conventionsRoute(projectIdentifier);
  const basePath =
    splitResourcePath(loc.pathname)?.basePath ??
    (loc.hash.startsWith("#/") ? loc.pathname.replace(/\/$/, "") : "");
  return { route, url: `${basePath}/#${route}` };
}

/** The choices when the templates endpoint does not answer: the same keys
 *  and names, without the summaries and text only the server has. */
export const FALLBACK_TEMPLATES: ConventionTemplate[] = [
  { key: "empty", name: "Empty", summary: "", content: "" },
  { key: "ai_managed", name: "Heavily AI managed", summary: "", content: "" },
  { key: "human_managed", name: "Human managed", summary: "", content: "" },
  { key: "human_owned", name: "Human owned", summary: "", content: "" },
];

/** The template a chooser starts on: the first one with text, so the
 *  preview has something to show, else the first one. */
export function defaultTemplateKey(templates: readonly ConventionTemplate[]): ConventionTemplateKey {
  return (templates.find((t) => t.content.trim() !== "") ?? templates[0])?.key ?? "empty";
}

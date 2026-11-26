// LIF-524: what the Pages list shows for a project's conventions page.
//
// A project has at most one conventions page. Until it holds any text the
// list offers the templates instead of the page; a blank start is only on
// offer while no page exists, since an empty page already is one.

import type { ConventionTemplate } from "../api";

/** The fields of a page row this decision reads. */
export interface ConventionsCandidate {
  id: number;
  preview: string;
  conventions?: boolean;
}

export type ConventionsState<T extends ConventionsCandidate> =
  /** No conventions page: templates plus a blank start. */
  | { kind: "none" }
  /** A conventions page with no text yet: templates fill it in. */
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

/** Templates to offer as starting points: everything with a body. The
 *  empty template is the blank start, which the list shows on its own. */
export function startingTemplates(templates: readonly ConventionTemplate[]): ConventionTemplate[] {
  return templates.filter((t) => t.key !== "empty" && t.content.trim() !== "");
}

/** The page title a conventions page is created with. */
export const CONVENTIONS_TITLE = "Conventions";

/** The create-project choices when the templates endpoint does not answer:
 *  the same keys and names, without the summaries only the server has. */
export const FALLBACK_TEMPLATES: ConventionTemplate[] = [
  { key: "empty", name: "Empty", summary: "", content: "" },
  { key: "ai_managed", name: "Heavily AI managed", summary: "", content: "" },
  { key: "human_managed", name: "Human managed", summary: "", content: "" },
  { key: "human_owned", name: "Human owned", summary: "", content: "" },
];

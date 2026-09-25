// LIF-442: skinny sync rows → the `Issue` shape the list/board components
// already speak.
//
// The list surfaces are built around `api.Issue`. Rather than retype every
// row component, card, comparator and grouping helper against a second
// issue type, the read model's rows are adapted into that shape at the one
// boundary where they enter a route.
//
// Three fields are reconstructed:
//
//  * `project_id` — the model knows it; it is the project it replicates.
//  * `sequence`   — the trailing number of the identifier (`LIF-442` → 442),
//                   which is exactly what the server derives it from. The
//                   "number" sort depends on it.
//  * `description` — the row's `preview`, NOT the document. `/index` and
//                   `/changes` ship skinny rows on purpose, so a cold start
//                   costs one round trip proportional to the row count
//                   rather than to every word in the project; what they do
//                   carry is the first non-empty line of the body, capped
//                   at 200 characters. That is exactly what the two
//                   consumers of this field on a list surface want: the
//                   comfortable-density preview line in `IssueRow`, and
//                   `computeSearchResult`'s body scoring and snippets.
//                   Both degrade honestly — a match past the first line is
//                   missed rather than mis-rendered. Anything that needs
//                   the real document (the editor, the peek panel) fetches
//                   the issue.
//
// Adapted objects are memoized per row. A row object is only replaced when
// the server actually sent a newer version of it, so an unchanged issue
// keeps its identity across a delta and every `{#each ... (issue.id)}` and
// `animate:flip` in the list stays still.

import type { Issue } from "../api";
import type { IssueRow } from "./types";

const adapted = new WeakMap<IssueRow, Issue>();

/** The `-42` tail of `PRO-42`. Falls back to 0 for an unparseable
 *  identifier, which only affects the "number" sort's tie-breaking. */
function sequenceOf(identifier: string): number {
  const match = /-(\d+)$/.exec(identifier);
  return match ? Number(match[1]) : 0;
}

export function toIssue(row: IssueRow, projectId: number): Issue {
  const cached = adapted.get(row);
  if (cached) return cached;
  const issue: Issue = {
    id: row.id,
    project_id: projectId,
    sequence: sequenceOf(row.identifier),
    identifier: row.identifier,
    title: row.title,
    // A 200-char first-line preview, not the document. See the note above.
    description: row.preview,
    status: row.status,
    priority: row.priority,
    module_id: row.module_id,
    sort_order: row.sort_order,
    start_date: row.start_date,
    target_date: row.target_date,
    created_at: row.created_at,
    updated_at: row.updated_at,
    labels: row.labels,
    waits: row.waits,
  };
  adapted.set(row, issue);
  return issue;
}

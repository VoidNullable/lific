// Issue-list fuzzy search. Extracted from IssueList.svelte (LIF-99).
//
// LIF-119: fuzzy full-text search across title, identifier, and
// description. The component computes the filtered set AND the per-issue
// score map in a single pass so it never has to write $state from inside a
// $derived (Svelte's state_unsafe_mutation guard). That whole computation
// lives here as a pure function; the component wraps it in one $derived and
// reads `issues` + `scores` off the result.

import type { Issue } from "../api";
import { fuzzyMatch, buildSnippet } from "../fuzzy";

// LIF-119: search tuning, kept identical to the page list (LIF-118) so the
// two list views feel consistent. See web/src/lib/fuzzy.ts for the scorer
// and the rationale on each constant.
export const SCORE_THRESHOLD = 0.25;
export const RESULT_CAP = 50;
export const CONTENT_SCAN_MAX = 4000;
export const CONTENT_WEIGHT = 0.6;
export const IDENTIFIER_WEIGHT = 0.9;

export interface SearchHit {
  score: number;
  snippet: string | null;
}

export interface SearchResult {
  issues: Issue[];
  scores: Map<number, SearchHit>;
}

/** Score `issues` against `query` and return the ranked, capped subset plus
 *  a per-issue score+snippet map. An empty/blank query short-circuits to the
 *  unfiltered input with no scores, so the caller can use one code path. */
export function computeSearchResult(
  query: string,
  issues: Issue[],
): SearchResult {
  const q = query.trim();
  if (!q) return { issues, scores: new Map() };

  const scores = new Map<number, SearchHit>();
  const hits: Array<{ issue: Issue; score: number }> = [];

  for (const issue of issues) {
    const titleHit = fuzzyMatch(q, issue.title);
    const idHit = fuzzyMatch(q, issue.identifier);
    const body = issue.description.slice(0, CONTENT_SCAN_MAX);
    const descHit = fuzzyMatch(q, body);

    const titleScore = titleHit?.score ?? 0;
    const idScore = (idHit?.score ?? 0) * IDENTIFIER_WEIGHT;
    const descScore = (descHit?.score ?? 0) * CONTENT_WEIGHT;

    const best = Math.max(titleScore, idScore, descScore);
    if (best < SCORE_THRESHOLD) continue;

    const snippet =
      descHit && descScore === best && best > 0
        ? buildSnippet(body, descHit.matchStart, descHit.matchEnd)
        : null;

    scores.set(issue.id, { score: best, snippet });
    hits.push({ issue, score: best });
  }

  hits.sort((a, b) => b.score - a.score);
  const capped = hits.slice(0, RESULT_CAP);

  // Drop scores for issues that fell off the result cap so the comparator
  // doesn't grant relevance ordering to invisible rows.
  const capIds = new Set(capped.map((h) => h.issue.id));
  for (const id of [...scores.keys()]) {
    if (!capIds.has(id)) scores.delete(id);
  }

  return { issues: capped.map((h) => h.issue), scores };
}

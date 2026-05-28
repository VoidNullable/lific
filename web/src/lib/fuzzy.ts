// LIF-118: small, dependency-free fuzzy matcher used by the page list.
//
// Two-tier scoring:
//   1. Exact substring (case-insensitive) — fast path, high ceiling.
//      Prefix and word-boundary matches score higher than mid-word.
//   2. Subsequence — all query chars must appear in order. Consecutive
//      runs and word-boundary hits boost the score; loose subsequences
//      score low so the caller's threshold can cut them.
//
// Scores are roughly bounded in [0, 1]:
//   ~0.95 — substring at start of haystack
//   ~0.9  — substring at a word boundary
//   ~0.8  — substring mid-word
//   ~0.5  — clean subsequence with several word-boundary hits
//   ~0.3  — sparse subsequence
//   null  — no subsequence match at all
//
// The subsequence tier is capped below the substring tier so any exact
// substring match always outranks any fuzzy match.

const WORD_BOUNDARY = /[\s\-_/.,()[\]{}<>:;!?"'`]/;

export interface FuzzyMatch {
  /** Roughly 0..1, higher = better. */
  score: number;
  /** Index of the first matched char in the haystack. */
  matchStart: number;
  /** One past the index of the last matched char in the haystack. */
  matchEnd: number;
}

export function fuzzyMatch(query: string, text: string): FuzzyMatch | null {
  if (!query) return null;
  if (!text) return null;
  const q = query.toLowerCase();
  const t = text.toLowerCase();

  // Tier 1: exact substring.
  const direct = t.indexOf(q);
  if (direct !== -1) {
    let score = 0.8;
    if (direct === 0) {
      score = 0.95;
    } else if (WORD_BOUNDARY.test(t[direct - 1])) {
      score = 0.9;
    }
    return { score, matchStart: direct, matchEnd: direct + q.length };
  }

  // Tier 2: subsequence with bonuses.
  let qi = 0;
  let ti = 0;
  let firstMatch = -1;
  let lastMatch = -1;
  let consecutiveCur = 0;
  let consecutiveMax = 0;
  let wordBoundaryHits = 0;

  while (qi < q.length && ti < t.length) {
    if (q[qi] === t[ti]) {
      if (firstMatch === -1) firstMatch = ti;

      if (ti === lastMatch + 1) {
        consecutiveCur++;
      } else {
        consecutiveCur = 1;
        const prev = ti > 0 ? t[ti - 1] : " ";
        if (WORD_BOUNDARY.test(prev)) wordBoundaryHits++;
      }
      if (consecutiveCur > consecutiveMax) consecutiveMax = consecutiveCur;

      lastMatch = ti;
      qi++;
    }
    ti++;
  }

  if (qi < q.length) return null;

  // Component ratios (each 0..1):
  //   density       — how tightly packed the matches are
  //   consecutivity — biggest unbroken run of matches
  //   wordliness    — how many matches landed on word starts
  const span = Math.max(1, lastMatch - firstMatch + 1);
  const density = q.length / span;
  const consecutivity = consecutiveMax / q.length;
  const wordliness = wordBoundaryHits / q.length;

  // Weighted blend. Cap at 0.7 so substring matches always outrank fuzzy.
  const raw = density * 0.4 + consecutivity * 0.4 + wordliness * 0.2;
  const score = Math.min(0.7, raw);

  return { score, matchStart: firstMatch, matchEnd: lastMatch + 1 };
}

/**
 * Build a short snippet centered on the match, useful for content previews.
 * Collapses whitespace and adds ellipses when truncating.
 */
export function buildSnippet(
  text: string,
  matchStart: number,
  matchEnd: number,
  context = 40,
): string {
  const start = Math.max(0, matchStart - context);
  const end = Math.min(text.length, matchEnd + context);
  const slice = text.slice(start, end).replace(/\s+/g, " ").trim();
  const prefix = start > 0 ? "…" : "";
  const suffix = end < text.length ? "…" : "";
  return `${prefix}${slice}${suffix}`;
}

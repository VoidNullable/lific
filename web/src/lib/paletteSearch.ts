// LIF-445 — synchronous local search for the command palette.
//
// The palette used to wait 120ms and then ask the server for everything.
// But when the selected project's read model (LIF-442) is warm, every issue
// and page row it could match is already in memory: identifier, title,
// labels and a bounded preview. Searching that in-process costs microseconds
// and renders on the next frame, so the server round trip becomes a
// *supplement* rather than the source of truth.
//
// This module is deliberately pure and dependency-free: no npm fuzzy
// package, no reactive state, no DOM. It scores documents, it does not know
// what a PaletteResult is. CommandPalette.svelte owns the wiring.
//
// ── Scoring model ──
//
// A query is split on whitespace into terms. Every term must match at least
// one field of a document (AND across terms, OR across fields) — typing more
// words narrows the result set, it never widens it.
//
// Each (term, field) pair yields a *quality* in [0,1]:
//
//   1.00  the field is exactly the term
//   0.90  the field starts with the term          (terms of >= 2 chars)
//   0.80  a word inside the field starts with it  (terms of >= 2 chars)
//   0.60  the term appears somewhere in the field
//   0.40  a word in the field is within the typo budget (terms of >= 4)
//
// A term's score is the best `weight x quality` over all fields, and a
// document's score is the sum over terms, normalized by the theoretical
// maximum so the result lands in (0,1].

/** Field weights. Title dominates; the reference and its bare number are
 *  worth nearly as much because "lif 445" and "LIF-445" are navigation, not
 *  browsing. Labels are a weak signal and the preview is the weakest: it is
 *  a truncated first line, so a hit there is often incidental. */
export const FIELD_WEIGHTS = {
  title: 5,
  ref: 4,
  number: 4,
  label: 2,
  preview: 1,
} as const;

/** The largest single-field weight, used to normalize document scores. */
export const MAX_FIELD_WEIGHT = FIELD_WEIGHTS.title;

/** Prefix matching needs at least this many characters. One letter matches
 *  the start of far too much to be a useful ranking signal. */
export const PREFIX_MIN_TERM = 2;

/** Typo tolerance only engages from this length. Below it, edit distance
 *  turns unrelated short words into matches ("bug" -> "big"). */
export const FUZZY_MIN_TERM = 4;

/** Allowed edit distance as a fraction of the term length, floored, but
 *  never below one — a 4-character term gets exactly one typo. */
export const FUZZY_MAX_RATIO = 0.2;

/** Match qualities, in [0,1]. See the scoring model note above. */
export const QUALITY = {
  exact: 1,
  prefix: 0.9,
  wordPrefix: 0.8,
  substring: 0.6,
  fuzzy: 0.4,
} as const;

/** Below this many local issue+page hits the palette still asks the server.
 *  At or above it the in-memory answer is good enough and the debounced FTS
 *  request is skipped entirely. */
export const LOCAL_HIT_SERVER_THRESHOLD = 5;

/** Highest score the server FTS tier can produce (the palette decays FTS
 *  rank positionally from 1.0). Local hits are mapped strictly above it. */
export const SERVER_SCORE_MAX = 1;

/** Local hits occupy a band above every server hit and below the exact
 *  identifier / compact-ref fast paths (3.0) and exact project matches
 *  (2.6), so navigation intent still wins outright. */
export const LOCAL_SCORE_FLOOR = 1.2;
export const LOCAL_SCORE_CEIL = 2.5;

/** Anything the palette can search locally: one skinny row from the read
 *  model. `IssueRow` and `PageRow` both satisfy this structurally. */
export interface PaletteDoc {
  kind: "issue" | "page";
  id: number;
  identifier: string;
  title: string;
  labels: string[];
  preview: string;
  updated_at: string;
}

export interface LocalHit<T extends PaletteDoc> {
  doc: T;
  /** Normalized to (0,1]. Map with {@link localScoreToPaletteScore}. */
  score: number;
}

const WORD_BOUNDARY = /[\s\-_/.,()[\]{}<>:;!?"'`]/;
const WORD_SPLIT = /[^a-z0-9]+/;

/** Split a query into lowercased terms. Whitespace only; punctuation stays
 *  attached so "LIF-445" is one term and matches the reference exactly. */
export function tokenize(query: string): string[] {
  return query.toLowerCase().split(/\s+/).filter(Boolean);
}

/** Levenshtein distance, abandoned as soon as it cannot come in at or under
 *  `budget`. Returns `budget + 1` to signal "further than allowed". */
export function boundedEditDistance(a: string, b: string, budget: number): number {
  if (a === b) return 0;
  if (Math.abs(a.length - b.length) > budget) return budget + 1;
  if (a.length === 0) return b.length;
  if (b.length === 0) return a.length;

  let prev = new Array<number>(b.length + 1);
  let curr = new Array<number>(b.length + 1);
  for (let j = 0; j <= b.length; j++) prev[j] = j;

  for (let i = 1; i <= a.length; i++) {
    curr[0] = i;
    let rowMin = curr[0];
    for (let j = 1; j <= b.length; j++) {
      const cost = a[i - 1] === b[j - 1] ? 0 : 1;
      curr[j] = Math.min(prev[j] + 1, curr[j - 1] + 1, prev[j - 1] + cost);
      if (curr[j] < rowMin) rowMin = curr[j];
    }
    if (rowMin > budget) return budget + 1;
    const swap = prev;
    prev = curr;
    curr = swap;
  }

  return prev[b.length];
}

/** The typo budget for a term, or 0 when it is too short to earn one. */
export function typoBudget(term: string): number {
  if (term.length < FUZZY_MIN_TERM) return 0;
  return Math.max(1, Math.floor(term.length * FUZZY_MAX_RATIO));
}

/**
 * Quality of one lowercased `term` against one field, in [0,1]. `allowFuzzy`
 * is false for numeric fields, where a one-character edit turns 445 into 45
 * and the result is nonsense rather than a near miss.
 */
export function matchQuality(term: string, text: string, allowFuzzy = true): number {
  if (!term || !text) return 0;
  const t = text.toLowerCase();
  if (t === term) return QUALITY.exact;

  const at = t.indexOf(term);

  // Single characters get no prefix credit: they would rank half the
  // project's issues as strong matches.
  if (term.length < PREFIX_MIN_TERM) {
    return at === -1 ? 0 : QUALITY.substring;
  }

  if (at === 0) return QUALITY.prefix;
  if (at > 0) {
    return WORD_BOUNDARY.test(t[at - 1]) ? QUALITY.wordPrefix : QUALITY.substring;
  }

  if (!allowFuzzy) return 0;
  const budget = typoBudget(term);
  if (budget === 0) return 0;
  for (const word of t.split(WORD_SPLIT)) {
    if (!word) continue;
    if (boundedEditDistance(term, word, budget) <= budget) return QUALITY.fuzzy;
  }
  return 0;
}

/** The trailing number of a reference: "LIF-445" -> "445", "LIF-DOC-3" -> "3". */
export function refNumber(identifier: string): string {
  const m = identifier.match(/(\d+)\s*$/);
  return m ? m[1] : "";
}

/**
 * Score one document against pre-tokenized terms. Returns 0 when any term
 * fails to match every field — that is the AND, and it is what makes a
 * second word narrow the list instead of padding it.
 */
export function scoreDoc(terms: string[], doc: PaletteDoc): number {
  if (terms.length === 0) return 0;
  const number = refNumber(doc.identifier);

  let total = 0;
  for (const term of terms) {
    let best = FIELD_WEIGHTS.title * matchQuality(term, doc.title);

    if (best < MAX_FIELD_WEIGHT) {
      best = Math.max(best, FIELD_WEIGHTS.ref * matchQuality(term, doc.identifier));
      if (number) {
        best = Math.max(best, FIELD_WEIGHTS.number * matchQuality(term, number, false));
      }
      for (const label of doc.labels) {
        best = Math.max(best, FIELD_WEIGHTS.label * matchQuality(term, label));
      }
      if (doc.preview) {
        best = Math.max(best, FIELD_WEIGHTS.preview * matchQuality(term, doc.preview));
      }
    }

    if (best === 0) return 0;
    total += best;
  }

  return total / (terms.length * MAX_FIELD_WEIGHT);
}

/** Newest first. ISO-8601 timestamps compare correctly as strings; the
 *  identifier is a last resort so the order is deterministic in tests. */
function tieBreak(a: PaletteDoc, b: PaletteDoc): number {
  if (a.updated_at !== b.updated_at) return a.updated_at < b.updated_at ? 1 : -1;
  return a.identifier < b.identifier ? -1 : a.identifier > b.identifier ? 1 : 0;
}

/**
 * Rank documents against a raw query string. Synchronous and allocation
 * light: the caller runs this on every keystroke.
 *
 * @param limit maximum hits to return; pass 0 for "all".
 */
export function searchLocalDocs<T extends PaletteDoc>(
  query: string,
  docs: Iterable<T>,
  limit = 16,
): Array<LocalHit<T>> {
  const terms = tokenize(query);
  if (terms.length === 0) return [];

  const hits: Array<LocalHit<T>> = [];
  for (const doc of docs) {
    const score = scoreDoc(terms, doc);
    if (score > 0) hits.push({ doc, score });
  }

  hits.sort((a, b) => b.score - a.score || tieBreak(a.doc, b.doc));
  return limit > 0 ? hits.slice(0, limit) : hits;
}

/**
 * Rank each kind against its own budget, so a project with a hundred
 * matching issues cannot starve its pages. A single global cap looks
 * harmless until every slot is spent before the first page is considered —
 * the palette then swears the page does not exist.
 */
export function searchLocalDocsPerKind<T extends PaletteDoc>(
  query: string,
  docs: Iterable<T>,
  perKind = 8,
): Array<LocalHit<T>> {
  const byKind = new Map<PaletteDoc["kind"], T[]>();
  for (const doc of docs) {
    const bucket = byKind.get(doc.kind);
    if (bucket) bucket.push(doc);
    else byKind.set(doc.kind, [doc]);
  }

  const hits: Array<LocalHit<T>> = [];
  for (const bucket of byKind.values()) {
    hits.push(...searchLocalDocs(query, bucket, perKind));
  }
  hits.sort((a, b) => b.score - a.score || tieBreak(a.doc, b.doc));
  return hits;
}

/** Map a normalized (0,1] local score into the palette's local band. */
export function localScoreToPaletteScore(score: number): number {
  const clamped = Math.min(1, Math.max(0, score));
  return LOCAL_SCORE_FLOOR + clamped * (LOCAL_SCORE_CEIL - LOCAL_SCORE_FLOOR);
}

/**
 * Server hits the local pass has not already produced, in their original
 * order. Identifiers compare case-insensitively; a hit without one is always
 * kept, since there is nothing to dedupe it against.
 */
export function dedupeByIdentifier<T extends { identifier?: string | null }>(
  server: T[],
  taken: Iterable<string | null | undefined>,
): T[] {
  const seen = new Set<string>();
  for (const id of taken) {
    if (id) seen.add(id.toLowerCase());
  }
  const out: T[] = [];
  for (const hit of server) {
    const id = hit.identifier?.toLowerCase();
    if (id) {
      if (seen.has(id)) continue;
      seen.add(id);
    }
    out.push(hit);
  }
  return out;
}

/**
 * First occurrence of each key wins. Run this on the *whole* merged result
 * set, after sorting: the identifier fast path and the local read model
 * genuinely produce the same row for a query like "LIF-445", and two rows
 * sharing a `{#each}` key is a Svelte runtime error, not a cosmetic one.
 * Sorting first is what makes "first wins" mean "the exact reference wins".
 */
export function dedupeByKey<T>(items: T[], keyOf: (item: T) => string): T[] {
  const seen = new Set<string>();
  const out: T[] = [];
  for (const item of items) {
    const key = keyOf(item);
    if (seen.has(key)) continue;
    seen.add(key);
    out.push(item);
  }
  return out;
}

/**
 * Whether a response should be dropped on arrival. A search is issued
 * against a generation counter *and* a project; either moving on invalidates
 * it. Checked after every await, not just when the debounce fires.
 */
export function isStaleSearch(
  issued: { gen: number; projectIdent: string | null },
  current: { gen: number; projectIdent: string | null },
): boolean {
  return issued.gen !== current.gen || issued.projectIdent !== current.projectIdent;
}

/**
 * Keep the keyboard cursor on the item it was on when a late server response
 * lands and reshuffles the list. Falls back to the old index, clamped, when
 * the selected item is gone (or was never identified).
 */
export function preserveSelection(
  previousKey: string | null,
  nextKeys: string[],
  previousIdx: number,
): number {
  if (nextKeys.length === 0) return 0;
  if (previousKey !== null) {
    const at = nextKeys.indexOf(previousKey);
    if (at !== -1) return at;
  }
  return Math.min(Math.max(previousIdx, 0), nextKeys.length - 1);
}

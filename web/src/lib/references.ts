// LIF-239 — shared helpers for issue/page/plan reference auto-linking,
// hover-card data fetching, and editor autocomplete.
//
// This file is intentionally self-contained: Markdown.svelte,
// EditableMarkdown.svelte, and IssueHoverCard.svelte are the only
// callers (see the LIF-239 concurrency contract — two other agents are
// touching api.ts / routes / lib/issues in parallel, so nothing here
// reaches into those).

import {
  resolveIssue,
  getModule,
  listProjects,
  listIssues,
  search as searchApi,
  type Issue,
  type Module,
  type Project,
} from "./api";
import { fuzzyMatch } from "./fuzzy";

// ── Identifier grammar ───────────────────────────────────────
//
// Project codes are an uppercase letter followed by 1-4 more
// uppercase letters/digits (2-5 chars total). This mirrors the
// backend's `validate_identifier` (src/db/queries/projects.rs) except
// the backend also allows a single bare letter — auto-linking requires
// 2+ here to cut down on false positives (a lone capital followed by
// "-123" in prose is far more likely to be a typo or a citation than a
// real reference).
export const PROJECT_CODE_RE = "[A-Z][A-Z0-9]{1,4}";

/** Matches LIF-42 (issue), LIF-DOC-3 (page), LIF-PLAN-7 (plan).
 *  Capture groups: 1 = project code, 2 = "DOC-" | "PLAN-" | undefined,
 *  3 = the trailing number. */
export const IDENTIFIER_RE = new RegExp(
  `\\b(${PROJECT_CODE_RE})-(DOC-|PLAN-)?(\\d+)\\b`,
  "g",
);

export type RefKind = "issue" | "page" | "plan";

export function refKind(kindMarker: string | undefined): RefKind {
  if (kindMarker === "DOC-") return "page";
  if (kindMarker === "PLAN-") return "plan";
  return "issue";
}

/** Issue identifiers carry their project code as the segment before the
 *  first dash (PROJECT_CODE_RE never contains one) — cheaper than
 *  threading a `projectIdentifier` prop through every call site that
 *  already has the identifier string in hand (peek's "open full view",
 *  the LIF-248 context menu's "open in new tab", shift-click handlers). */
export function projectCodeOf(identifier: string): string {
  return identifier.split("-")[0];
}

/** In-app hash route for a matched identifier. Issues deep-link
 *  directly since their route is keyed by the identifier string itself
 *  (`/PROJ/issues/PROJ-42`). Pages/plans are keyed by numeric id, which
 *  would need a network round trip to resolve mid-render — the markdown
 *  pipeline is synchronous, so those link to the project's list view
 *  instead (same tradeoff the pre-LIF-239 code already made for DOC-n). */
export function routeFor(project: string, kind: RefKind, identifier: string): string {
  if (kind === "page") return `#/${project}/pages`;
  if (kind === "plan") return `#/${project}/plans`;
  return `#/${project}/issues/${identifier}`;
}

// ── Issue + module cache (session-lived, module scope) ────────
//
// Hover cards and editor autocomplete both resolve issues by
// identifier. A single shared cache means re-hovering an identifier, or
// hovering one autocomplete already resolved, costs nothing.

export type CachedIssue =
  | { status: "ok"; issue: Issue }
  // 404 (deleted) and 403 (exists but the viewer lacks project access)
  // both render the same quiet "not available" card — see LIF-239.
  | { status: "unavailable" };

const issueCache = new Map<string, CachedIssue>();
const issueInFlight = new Map<string, Promise<CachedIssue>>();

export async function fetchIssueCached(identifier: string): Promise<CachedIssue> {
  const key = identifier.toUpperCase();
  const cached = issueCache.get(key);
  if (cached) return cached;
  const pending = issueInFlight.get(key);
  if (pending) return pending;

  const promise = resolveIssue(key).then((res) => {
    const result: CachedIssue = res.ok
      ? { status: "ok", issue: res.data }
      : { status: "unavailable" };
    issueCache.set(key, result);
    issueInFlight.delete(key);
    return result;
  });
  issueInFlight.set(key, promise);
  return promise;
}

const moduleCache = new Map<number, Module | null>();
const moduleInFlight = new Map<number, Promise<Module | null>>();

export async function fetchModuleCached(id: number): Promise<Module | null> {
  if (moduleCache.has(id)) return moduleCache.get(id) ?? null;
  const pending = moduleInFlight.get(id);
  if (pending) return pending;

  const promise = getModule(id).then((res) => {
    const mod = res.ok ? res.data : null;
    moduleCache.set(id, mod);
    moduleInFlight.delete(id);
    return mod;
  });
  moduleInFlight.set(id, promise);
  return promise;
}

// ── Editor autocomplete: trigger detection ────────────────────

export interface TriggerMatch {
  /** Index into the source text where the token (incl. leading "#")
   *  starts. */
  start: number;
  /** The raw typed token, e.g. "#foo" or "LIF-4". */
  token: string;
  /** Token with any leading "#" stripped. */
  query: string;
  mode: "hash" | "identifier";
}

// Fires on "#…" (hash-mention → cross-project text search) or on a
// project-code-shaped prefix with a dash, e.g. "LIF-" / "lif-4"
// (case-insensitive while typing; canonicalized to uppercase on
// accept). The token must start at a word boundary (start of line or
// after whitespace) so it never fires mid-word, e.g. "seeLIF-4".
const TRIGGER_RE = /(?:^|\s)(#[^\s#]*|[A-Za-z]{2,5}-[0-9]*)$/;

export function findTrigger(text: string, caret: number): TriggerMatch | null {
  const upto = text.slice(0, caret);
  const m = upto.match(TRIGGER_RE);
  if (!m) return null;
  const token = m[1];
  const start = caret - token.length;
  if (token.startsWith("#")) {
    return { start, token, query: token.slice(1), mode: "hash" };
  }
  return { start, token, query: token, mode: "identifier" };
}

// ── Editor autocomplete: suggestions ───────────────────────────

export interface SuggestionHit {
  id: number;
  identifier: string;
  title: string;
  status: string;
}

// Project catalog, cached with a short TTL. Deliberately independent
// from CommandPalette's own catalog cache (LIF-159) — this feature owns
// a separate file surface per the concurrency contract, and the two
// caches are cheap enough that sharing isn't worth the coupling.
let projectCatalog: Project[] = [];
let projectCatalogAt = 0;
const CATALOG_TTL = 60_000;

async function ensureProjectCatalog(): Promise<Project[]> {
  if (Date.now() - projectCatalogAt < CATALOG_TTL && projectCatalog.length > 0) {
    return projectCatalog;
  }
  const res = await listProjects();
  if (res.ok) {
    projectCatalog = res.data;
    projectCatalogAt = Date.now();
  }
  return projectCatalog;
}

// Per-project issue list, cached briefly so typing digits after "LIF-"
// filters locally instead of round-tripping the server on every
// keystroke.
const projectIssuesCache = new Map<number, { at: number; issues: Issue[] }>();
const ISSUES_TTL = 30_000;

async function issuesForProject(projectId: number): Promise<Issue[]> {
  const cached = projectIssuesCache.get(projectId);
  if (cached && Date.now() - cached.at < ISSUES_TTL) return cached.issues;
  const res = await listIssues({ project_id: projectId, limit: 200 });
  const issues = res.ok ? res.data : [];
  projectIssuesCache.set(projectId, { at: Date.now(), issues });
  return issues;
}

/** Resolve a TriggerMatch to a ranked suggestion list (capped to 8).
 *  Hash mode does a cross-project full-text search (title/body) via the
 *  existing `/search` API — an empty query returns [] rather than
 *  hitting the server, since the backend rejects an empty FTS query.
 *  Identifier mode resolves the project from the typed code, then
 *  fuzzy-filters that project's issues by number prefix using
 *  fuzzy.ts's fuzzyMatch (kept to prefix-only hits via matchStart===0,
 *  since digits after the dash are the only thing the trigger grammar
 *  allows the user to type there). */
export async function searchSuggestions(trigger: TriggerMatch): Promise<SuggestionHit[]> {
  if (trigger.mode === "hash") {
    const q = trigger.query.trim();
    if (!q) return [];
    const res = await searchApi(q);
    if (!res.ok) return [];
    return res.data
      .filter((r) => r.result_type === "issue" && r.identifier)
      .slice(0, 8)
      .map((r) => ({ id: r.id, identifier: r.identifier as string, title: r.title, status: "" }));
  }

  const m = trigger.query.match(/^([A-Za-z]{2,5})-([0-9]*)$/);
  if (!m) return [];
  const [, codeRaw, digits] = m;
  const code = codeRaw.toUpperCase();

  const projects = await ensureProjectCatalog();
  const project = projects.find((p) => p.identifier === code);
  if (!project) return [];

  const issues = await issuesForProject(project.id);
  let candidates = issues;
  if (digits) {
    candidates = issues
      .map((issue) => ({ issue, m: fuzzyMatch(digits, String(issue.sequence)) }))
      .filter((x): x is { issue: Issue; m: NonNullable<ReturnType<typeof fuzzyMatch>> } =>
        x.m !== null && x.m.matchStart === 0,
      )
      .sort((a, b) => b.m.score - a.m.score || a.issue.sequence - b.issue.sequence)
      .map((x) => x.issue);
  }
  return candidates.slice(0, 8).map((issue) => ({
    id: issue.id,
    identifier: issue.identifier,
    title: issue.title,
    status: issue.status,
  }));
}

// ── Caret-anchored positioning ────────────────────────────────
//
// Mirror-div technique (the standard approach behind the
// textarea-caret-position package): clone the textarea's box + font
// metrics into an offscreen div, render the text up to the caret plus a
// marker span, then read the span's offset. Kept self-contained here so
// EditableMarkdown doesn't need a new dependency.

const MIRROR_STYLE_PROPS = [
  "boxSizing", "width", "height", "overflowX", "overflowY",
  "borderTopWidth", "borderRightWidth", "borderBottomWidth", "borderLeftWidth",
  "borderStyle", "paddingTop", "paddingRight", "paddingBottom", "paddingLeft",
  "fontStyle", "fontVariant", "fontWeight", "fontStretch", "fontSize",
  "lineHeight", "fontFamily", "textAlign", "textTransform", "textIndent",
  "textDecoration", "letterSpacing", "wordSpacing", "tabSize",
] as const;

export function getCaretCoordinates(
  el: HTMLTextAreaElement,
  position: number,
): { top: number; left: number; height: number } {
  const div = document.createElement("div");
  const style = div.style;
  const computed = window.getComputedStyle(el);

  style.position = "absolute";
  style.visibility = "hidden";
  style.whiteSpace = "pre-wrap";
  style.wordWrap = "break-word";
  style.top = "0";
  style.left = "-9999px";

  const styleRecord = style as unknown as Record<string, string>;
  const computedRecord = computed as unknown as Record<string, string>;
  for (const prop of MIRROR_STYLE_PROPS) {
    styleRecord[prop] = computedRecord[prop];
  }

  document.body.appendChild(div);
  div.textContent = el.value.slice(0, position);

  const span = document.createElement("span");
  span.textContent = el.value.slice(position) || ".";
  div.appendChild(span);

  const top = span.offsetTop;
  const left = span.offsetLeft;
  const lineHeight = parseFloat(computed.lineHeight);
  document.body.removeChild(div);

  return { top, left, height: Number.isFinite(lineHeight) ? lineHeight : span.offsetHeight };
}

// The Pages list's pure logic: folder structure, filters, sorting, grouping
// and the small derived facts a row or the pulse sidebar shows. Everything
// here is a plain function of its inputs so it can be tested without a DOM.

import type { Folder, PageMeta, PageMetaPerson } from "../api";

/** The fields of a page row this module reads. */
export interface ListPage {
  id: number;
  identifier: string;
  title: string;
  status: string;
  folder_id: number | null;
  pinned: boolean;
  created_at: string;
  updated_at: string;
  preview: string;
  labels: string[];
  conventions?: boolean;
}

export const PAGE_SORTS = ["updated", "created", "title", "status"] as const;
export type PageSort = (typeof PAGE_SORTS)[number];

export const PAGE_GROUPS = ["folder", "edited", "label", "status", "none"] as const;
export type PageGroup = (typeof PAGE_GROUPS)[number];

/** Statuses a page can be filtered by. Archived pages live in their own
 *  place in the folder pane and never mix into the others. */
export const LIVE_STATUSES = ["draft", "active", "complete"] as const;

export const STATUS_LABEL: Record<string, string> = {
  draft: "Draft",
  active: "Active",
  complete: "Complete",
  archived: "Archived",
};

const STATUS_ORDER: Record<string, number> = { draft: 0, active: 1, complete: 2, archived: 3 };

/** What the list shows: every live page, one folder, the archive, or the
 *  conventions page. */
export type FolderScope = "all" | "archived" | "conventions" | number;

/** Who last edited a page: anyone, people only, agents only, or one account. */
export type EditorFilter = "" | "people" | "agents" | `u:${number}`;

export interface PageFilters {
  statuses: string[];
  label: string;
  editor: EditorFilter;
  /** Only pages nothing links to. */
  unlinked: boolean;
}

export const NO_FILTERS: PageFilters = { statuses: [], label: "", editor: "", unlinked: false };

export function filterCount(f: PageFilters): number {
  return (f.statuses.length > 0 ? 1 : 0) + (f.label ? 1 : 0) + (f.editor ? 1 : 0) + (f.unlinked ? 1 : 0);
}

// ── Folders ──────────────────────────────────────────────

export interface FolderNode {
  folder: Folder;
  depth: number;
}

/** Depth-first folder order, siblings by name, each with its depth. */
export function orderedFolders(folders: readonly Folder[]): FolderNode[] {
  const byParent = new Map<number | null, Folder[]>();
  const ids = new Set(folders.map((f) => f.id));
  for (const f of folders) {
    // A parent that is not in the list makes the folder a root, so nothing
    // disappears from the tree.
    const parent = f.parent_id != null && ids.has(f.parent_id) ? f.parent_id : null;
    const list = byParent.get(parent) ?? [];
    list.push(f);
    byParent.set(parent, list);
  }
  const out: FolderNode[] = [];
  const seen = new Set<number>();
  const walk = (parent: number | null, depth: number) => {
    const kids = (byParent.get(parent) ?? []).slice().sort((a, b) => a.name.localeCompare(b.name));
    for (const f of kids) {
      if (seen.has(f.id)) continue;
      seen.add(f.id);
      out.push({ folder: f, depth });
      walk(f.id, depth + 1);
    }
  };
  walk(null, 0);
  return out;
}

/** The folder and every folder under it. */
export function descendantIds(folders: readonly Folder[], id: number): Set<number> {
  const out = new Set<number>([id]);
  let grew = true;
  while (grew) {
    grew = false;
    for (const f of folders) {
      if (f.parent_id != null && out.has(f.parent_id) && !out.has(f.id)) {
        out.add(f.id);
        grew = true;
      }
    }
  }
  return out;
}

/** Root-first chain of folders down to `id`. */
export function folderPath(folders: readonly Folder[], id: number | null): Folder[] {
  const out: Folder[] = [];
  let cur = id == null ? undefined : folders.find((f) => f.id === id);
  while (cur && out.length < 32) {
    out.unshift(cur);
    const parent: number | null = cur.parent_id;
    cur = parent == null ? undefined : folders.find((f) => f.id === parent);
  }
  return out;
}

export function folderPathLabel(folders: readonly Folder[], id: number | null): string {
  return folderPath(folders, id).map((f) => f.name).join(" / ");
}

export interface FolderStats {
  /** Pages directly in the folder. */
  direct: number;
  /** Pages in the folder and everything under it. */
  total: number;
  /** Latest `updated_at` over `total`, or null when empty. */
  updated: string | null;
}

/** Page counts per folder, over the pages given. */
export function folderStats(pages: readonly ListPage[], folders: readonly Folder[]): Map<number, FolderStats> {
  const out = new Map<number, FolderStats>();
  for (const f of folders) out.set(f.id, { direct: 0, total: 0, updated: null });
  const parentOf = new Map(folders.map((f) => [f.id, f.parent_id]));
  for (const p of pages) {
    if (p.folder_id == null || !out.has(p.folder_id)) continue;
    out.get(p.folder_id)!.direct += 1;
    let cur: number | null | undefined = p.folder_id;
    let guard = 0;
    while (cur != null && out.has(cur) && guard++ < 32) {
      const s = out.get(cur)!;
      s.total += 1;
      if (!s.updated || p.updated_at > s.updated) s.updated = p.updated_at;
      cur = parentOf.get(cur);
    }
  }
  return out;
}

/** Pages whose folder is gone count as unfiled, so they stay reachable. */
export function isUnfiled(page: ListPage, folderIds: ReadonlySet<number>): boolean {
  return page.folder_id == null || !folderIds.has(page.folder_id);
}

/** Whether a page belongs in a folder scope; the other scopes don't narrow
 *  by folder. */
export function inScope(
  page: ListPage,
  scope: FolderScope,
  folders: readonly Folder[],
  scopeIds: ReadonlySet<number> | null = null,
): boolean {
  if (typeof scope !== "number") return true;
  const ids = scopeIds ?? descendantIds(folders, scope);
  return page.folder_id != null && ids.has(page.folder_id);
}

// ── Editors and links ────────────────────────────────────

export function editorMatches(filter: EditorFilter, person: PageMetaPerson | null | undefined): boolean {
  if (!filter) return true;
  if (!person) return false;
  if (filter === "agents") return person.is_bot;
  if (filter === "people") return !person.is_bot;
  return `u:${person.id}` === filter;
}

export function linkTotal(meta: PageMeta | undefined): number {
  if (!meta) return 0;
  return meta.links.issues + meta.links.pages + meta.links.comments;
}

const plural = (n: number, one: string, many = `${one}s`) => `${n} ${n === 1 ? one : many}`;

/** "Linked from 2 issues and 1 page", or "" when nothing links here. */
export function describeLinks(meta: PageMeta | undefined): string {
  if (!meta) return "";
  const parts: string[] = [];
  if (meta.links.issues) parts.push(plural(meta.links.issues, "issue"));
  if (meta.links.pages) parts.push(plural(meta.links.pages, "page"));
  if (meta.links.comments) parts.push(plural(meta.links.comments, "comment"));
  if (parts.length === 0) return "";
  const last = parts.pop()!;
  return `Linked from ${parts.length ? `${parts.join(", ")} and ${last}` : last}`;
}

export function personLabel(person: PageMetaPerson): string {
  return person.display_name?.trim() || person.username;
}

// ── Filtering, sorting, grouping ─────────────────────────

export function applyFilters(
  pages: readonly ListPage[],
  filters: PageFilters,
  meta: ReadonlyMap<number, PageMeta>,
): ListPage[] {
  return pages.filter((p) => {
    if (filters.statuses.length > 0 && !filters.statuses.includes(p.status)) return false;
    if (filters.label && !p.labels.includes(filters.label)) return false;
    if (filters.editor && !editorMatches(filters.editor, meta.get(p.id)?.updated_by)) return false;
    // Until the links load nothing is known to be unlinked.
    if (filters.unlinked && (!meta.has(p.id) || linkTotal(meta.get(p.id)) > 0)) return false;
    return true;
  });
}

const byUpdated = (a: ListPage, b: ListPage) => b.updated_at.localeCompare(a.updated_at) || b.id - a.id;
const byCreated = (a: ListPage, b: ListPage) => b.created_at.localeCompare(a.created_at) || b.id - a.id;
const byTitle = (a: ListPage, b: ListPage) =>
  a.title.localeCompare(b.title, undefined, { sensitivity: "base", numeric: true }) || a.id - b.id;

export function sortPages(pages: readonly ListPage[], sort: PageSort): ListPage[] {
  const out = pages.slice();
  switch (sort) {
    case "created":
      return out.sort(byCreated);
    case "title":
      return out.sort(byTitle);
    case "status":
      return out.sort((a, b) => (STATUS_ORDER[a.status] ?? 9) - (STATUS_ORDER[b.status] ?? 9) || byUpdated(a, b));
    case "updated":
    default:
      return out.sort(byUpdated);
  }
}

export interface PageGroupBlock {
  key: string;
  /** Header text; "" for the single ungrouped block. */
  label: string;
  /** Folder groups: the folder, or null for unfiled pages. */
  folderId?: number | null;
  depth?: number;
  status?: string;
  labelColor?: string;
  pages: ListPage[];
}

export function groupPages(
  pages: readonly ListPage[],
  group: PageGroup,
  ctx: {
    folders: readonly Folder[];
    labelColors?: ReadonlyMap<string, string>;
    /** Folder groups: the scope's own folder, shown even when empty. */
    scopeFolder?: number | null;
    /** Day groups: the clock to bucket against. */
    now?: Date;
  },
): PageGroupBlock[] {
  if (group === "none") return [{ key: "all", label: "", pages: pages.slice() }];

  if (group === "edited") {
    const sorted = pages.slice().sort((a, b) => b.updated_at.localeCompare(a.updated_at));
    return bucketByDay(sorted, ctx.now).map((b) => ({ key: `d:${b.bucket}`, label: b.bucket, pages: b.rows }));
  }

  if (group === "status") {
    const out: PageGroupBlock[] = [];
    for (const status of ["draft", "active", "complete", "archived"]) {
      const list = pages.filter((p) => p.status === status);
      if (list.length) out.push({ key: `s:${status}`, label: STATUS_LABEL[status], status, pages: list });
    }
    return out;
  }

  if (group === "label") {
    const names = new Set<string>();
    for (const p of pages) for (const l of p.labels) names.add(l);
    const out: PageGroupBlock[] = [...names]
      .sort((a, b) => a.localeCompare(b))
      .map((name) => ({
        key: `l:${name}`,
        label: name,
        labelColor: ctx.labelColors?.get(name),
        pages: pages.filter((p) => p.labels.includes(name)),
      }));
    const none = pages.filter((p) => p.labels.length === 0);
    if (none.length) out.push({ key: "l:", label: "No label", pages: none });
    return out;
  }

  // Folder: tree order, unfiled first, folders with no matching pages left
  // out except the scope's own folder (which then reads as empty).
  const folderIds = new Set(ctx.folders.map((f) => f.id));
  const out: PageGroupBlock[] = [];
  const unfiled = pages.filter((p) => isUnfiled(p, folderIds));
  if (unfiled.length) out.push({ key: "f:root", label: "Unfiled", folderId: null, depth: 0, pages: unfiled });
  const scopeDepth =
    typeof ctx.scopeFolder === "number" ? folderPath(ctx.folders, ctx.scopeFolder).length - 1 : 0;
  for (const node of orderedFolders(ctx.folders)) {
    const list = pages.filter((p) => p.folder_id === node.folder.id);
    if (!list.length && node.folder.id !== ctx.scopeFolder) continue;
    out.push({
      key: `f:${node.folder.id}`,
      label: folderPathLabel(ctx.folders, node.folder.id),
      folderId: node.folder.id,
      depth: Math.max(0, node.depth - scopeDepth),
      pages: list,
    });
  }
  return out;
}

// ── Time ─────────────────────────────────────────────────

/** API timestamps are UTC without a zone; some carry a space separator. */
export function parseStamp(iso: string): Date {
  const s = iso.replace(" ", "T");
  return new Date(/[zZ]|[+-]\d\d:?\d\d$/.test(s) ? s : `${s}Z`);
}

export const DAY_BUCKETS = ["Today", "Yesterday", "Earlier this week", "Earlier this month", "Older"] as const;
export type DayBucket = (typeof DAY_BUCKETS)[number];

function startOfDay(d: Date): number {
  return new Date(d.getFullYear(), d.getMonth(), d.getDate()).getTime();
}

/** Which day group a timestamp falls in, by local calendar day. */
export function dayBucket(iso: string, now: Date = new Date()): DayBucket {
  const days = Math.round((startOfDay(now) - startOfDay(parseStamp(iso))) / 86_400_000);
  if (days <= 0) return "Today";
  if (days === 1) return "Yesterday";
  if (days < 7) return "Earlier this week";
  if (days < 30) return "Earlier this month";
  return "Older";
}

export function bucketByDay<T extends { updated_at: string }>(
  rows: readonly T[],
  now: Date = new Date(),
): { bucket: DayBucket; rows: T[] }[] {
  const map = new Map<DayBucket, T[]>();
  for (const r of rows) {
    const b = dayBucket(r.updated_at, now);
    const list = map.get(b) ?? [];
    list.push(r);
    map.set(b, list);
  }
  return DAY_BUCKETS.filter((b) => map.has(b)).map((bucket) => ({ bucket, rows: map.get(bucket)! }));
}

/** Whole days between a timestamp and now, never negative. */
export function daysSince(iso: string, now: Date = new Date()): number {
  return Math.max(0, Math.floor((now.getTime() - parseStamp(iso).getTime()) / 86_400_000));
}

/** "Draft for 12 days", "Draft since today". */
export function draftAge(createdAt: string, now: Date = new Date()): string {
  const d = daysSince(createdAt, now);
  if (d === 0) return "Started today";
  if (d === 1) return "Draft for 1 day";
  return `Draft for ${d} days`;
}

/** Active pages nobody has touched in this many days count as gone quiet. */
export const QUIET_DAYS = 60;

export function quietPages(pages: readonly ListPage[], now: Date = new Date()): ListPage[] {
  return pages
    .filter((p) => p.status === "active" && daysSince(p.updated_at, now) >= QUIET_DAYS)
    .sort((a, b) => a.updated_at.localeCompare(b.updated_at));
}

export function readingMinutes(words: number): number {
  return Math.max(1, Math.round(words / 230));
}

/** "412 words · 2 min read", or "" for an empty page. */
export function describeLength(words: number): string {
  if (words <= 0) return "";
  return `${words.toLocaleString("en-US")} ${words === 1 ? "word" : "words"} · ${readingMinutes(words)} min read`;
}

/** The row's excerpt. The server's prose excerpt when it has loaded; until
 *  then the first line, unless that line is a heading, which says nothing
 *  a title doesn't. */
export function excerptOf(page: Pick<ListPage, "preview">, meta: PageMeta | undefined): string {
  if (meta) return meta.excerpt;
  const line = page.preview.trim();
  if (!line || /^#{1,6}\s/.test(line) || /^(```|~~~|\||---|<)/.test(line)) return "";
  return line.replace(/^([-*+]|\d+\.)\s+(\[[ xX]\]\s+)?/, "").replace(/^>\s*/, "").replace(/[*_`[\]]/g, "").slice(0, 220);
}


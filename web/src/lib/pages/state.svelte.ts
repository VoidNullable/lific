// The Pages list's view state: which folder, which filters, how it sorts
// and groups, and what the reading pane shows. Remembered per project so a
// trip into a page and back lands on the same view.

import {
  NO_FILTERS,
  PAGE_GROUPS,
  PAGE_SORTS,
  type EditorFilter,
  type FolderScope,
  type PageFilters,
  type PageGroup,
  type PageSort,
} from "./list";

export interface PageViewSnapshot {
  scope: FolderScope;
  filters: PageFilters;
  sort: PageSort;
  group: PageGroup;
  excerpts: boolean;
  pane: boolean;
}

const KEY = (project: string) => `lific:pages-view:${project}`;

export const DEFAULT_VIEW: PageViewSnapshot = {
  scope: "all",
  filters: NO_FILTERS,
  sort: "updated",
  group: "folder",
  excerpts: true,
  pane: true,
};

/** Parse a stored snapshot, keeping only values this build understands. */
export function parseView(raw: string | null): PageViewSnapshot {
  if (!raw) return { ...DEFAULT_VIEW, filters: { ...NO_FILTERS } };
  try {
    const v = JSON.parse(raw) as Partial<PageViewSnapshot> | null;
    if (!v || typeof v !== "object") throw new Error("not an object");
    const f = (v.filters ?? {}) as Partial<PageFilters>;
    const editor = typeof f.editor === "string" && /^(|people|agents|u:\d+)$/.test(f.editor) ? f.editor : "";
    return {
      scope: v.scope === "all" || v.scope === "unfiled" || typeof v.scope === "number" ? v.scope : "all",
      filters: {
        statuses: Array.isArray(f.statuses)
          ? f.statuses.filter((s): s is string => s === "draft" || s === "active" || s === "complete")
          : [],
        label: typeof f.label === "string" ? f.label : "",
        editor: editor as EditorFilter,
        unlinked: f.unlinked === true,
      },
      sort: PAGE_SORTS.includes(v.sort as PageSort) ? (v.sort as PageSort) : DEFAULT_VIEW.sort,
      group: PAGE_GROUPS.includes(v.group as PageGroup) ? (v.group as PageGroup) : DEFAULT_VIEW.group,
      excerpts: typeof v.excerpts === "boolean" ? v.excerpts : true,
      pane: typeof v.pane === "boolean" ? v.pane : true,
    };
  } catch {
    return { ...DEFAULT_VIEW, filters: { ...NO_FILTERS } };
  }
}

export class PageListView {
  scope = $state<FolderScope>("all");
  filters = $state<PageFilters>({ ...NO_FILTERS });
  sort = $state<PageSort>("updated");
  group = $state<PageGroup>("folder");
  excerpts = $state(true);
  pane = $state(true);

  search = $state("");
  searchOpen = $state(false);
  /** The keyboard cursor, and the page the reading pane shows. */
  focusedId = $state<number | null>(null);
  selected = $state<Set<number>>(new Set());

  filterOpen = $state(false);
  displayOpen = $state(false);
  newMenuOpen = $state(false);

  hydrated = false;

  hydrate(project: string) {
    let raw: string | null = null;
    try {
      raw = localStorage.getItem(KEY(project));
    } catch {
      // private mode
    }
    const v = parseView(raw);
    this.scope = v.scope;
    this.filters = v.filters;
    this.sort = v.sort;
    this.group = v.group;
    this.excerpts = v.excerpts;
    this.pane = v.pane;
    this.search = "";
    this.searchOpen = false;
    this.focusedId = null;
    this.selected = new Set();
    this.hydrated = true;
  }

  snapshot(): PageViewSnapshot {
    return {
      scope: this.scope,
      filters: { ...this.filters, statuses: [...this.filters.statuses] },
      sort: this.sort,
      group: this.group,
      excerpts: this.excerpts,
      pane: this.pane,
    };
  }

  save(project: string) {
    if (!this.hydrated) return;
    try {
      localStorage.setItem(KEY(project), JSON.stringify(this.snapshot()));
    } catch {
      // ignore
    }
  }

  closePopovers() {
    this.filterOpen = false;
    this.displayOpen = false;
    this.newMenuOpen = false;
  }

  toggleStatus(status: string) {
    const has = this.filters.statuses.includes(status);
    this.filters = {
      ...this.filters,
      statuses: has ? this.filters.statuses.filter((s) => s !== status) : [...this.filters.statuses, status],
    };
  }

  clearFilters() {
    this.filters = { ...NO_FILTERS };
  }

  toggleSelected(id: number) {
    const next = new Set(this.selected);
    if (next.has(id)) next.delete(id);
    else next.add(id);
    this.selected = next;
  }
}

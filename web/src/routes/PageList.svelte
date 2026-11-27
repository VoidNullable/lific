<script lang="ts">
  // The Pages tab: a folder pane, the page list, and a reading pane (or the
  // project's page activity when nothing is open). Rows come from the
  // project's read model (LIF-442); excerpts, editors and link counts come
  // from `/pages/meta`, which reloads when the rows move.
  import {
    listProjects,
    listFolders,
    listLabels,
    createPage,
    createFolder,
    deleteFolder,
    renameFolder,
    updatePage,
    getPagesMeta,
    saveArchiveDownload,
    type Folder,
    type Label,
    type PageMeta,
    type PageMetaPerson,
    type Project,
  } from "../lib/api";
  import {
    cacheProjects,
    cachedProject,
    ensureProjectModel,
    refreshProjectModel,
    setActiveProject,
    type ProjectReadModel,
  } from "../lib/sync/readModel.svelte";
  import type { PageRow as SyncPageRow } from "../lib/sync/types";
  import {
    ArrowUpRight,
    Copy,
    Eye,
    FileText,
    FolderClosed,
    FolderInput,
    Link as LinkIcon,
    Pin,
    Plus,
    X,
    Archive,
    CircleDot,
    CheckCircle2,
    PenLine,
    RotateCcw,
  } from "lucide-svelte";
  import { getContext, untrack } from "svelte";
  import Select from "../lib/Select.svelte";
  import Mascot from "../lib/Mascot.svelte";
  import ErrorState from "../lib/ErrorState.svelte";
  import Skeleton from "../lib/Skeleton.svelte";
  import TimeAgo from "../lib/TimeAgo.svelte";
  import { fuzzyMatch, buildSnippet } from "../lib/fuzzy";
  import { projectRole, loadProjectRole } from "../lib/projectRole.svelte";
  import { toast } from "../lib/toast/toast.svelte";
  import { copyToClipboard } from "../lib/clipboard";
  import { openContextMenu, type ContextMenuItem } from "../lib/contextMenuState.svelte";
  import { openPagePeek, pagePeekState } from "../lib/pages/pagePeek.svelte";
  import { shortcutsSuppressed } from "../lib/shortcuts";
  import { currentUser } from "../lib/userState";
  import { loadSeen, saveSeen, snapshotOf, isSeen, withSeen, pruneSeen, type SeenMap } from "../lib/seenStore";
  import { selectedIssueExport } from "../lib/issues/export";
  import { safeLabelColor } from "../lib/labelColors";
  import { conventionsState } from "../lib/pages/conventions";
  import FolderTree from "../lib/pages/FolderTree.svelte";
  import PageRowView from "../lib/pages/PageRow.svelte";
  import ReadingPane from "../lib/pages/ReadingPane.svelte";
  import PagesPulse from "../lib/pages/PagesPulse.svelte";
  import PagesTopbar from "../lib/pages/PagesTopbar.svelte";
  import PageBulkBar from "../lib/pages/PageBulkBar.svelte";
  import { PageListView } from "../lib/pages/state.svelte";
  import {
    applyFilters,
    descendantIds,
    filterCount,
    folderPathLabel,
    folderStats,
    groupPages,
    inScope,
    personLabel,
    sortPages,
    STATUS_LABEL,
    type FolderScope,
    type ListPage,
    type PageGroupBlock,
  } from "../lib/pages/list";

  let {
    navigate,
    projectIdentifier,
  }: {
    navigate: (path: string) => void;
    projectIdentifier: string;
  } = $props();

  // LIF-234: pages are content; viewers browse read-only.
  const canEdit = $derived(projectRole.canEdit);

  const topbarCtx = getContext<{ set: (s: import("svelte").Snippet | undefined) => void } | undefined>("lific:topbar");
  $effect(() => {
    topbarCtx?.set(topbarContent);
    return () => topbarCtx?.set(undefined);
  });

  // LIF-118: search tuning. Title hits beat identifier hits beat label and
  // first-line hits.
  const SCORE_THRESHOLD = 0.25;
  const RESULT_CAP = 50;
  const CONTENT_WEIGHT = 0.6;
  const IDENTIFIER_WEIGHT = 0.9;
  const LABEL_WEIGHT = 0.55;

  let project = $state<Project | null>(null);
  let model = $state<ProjectReadModel | null>(null);
  // Local rows, re-seeded from the model whenever no write is in flight, so
  // optimistic changes survive until the server answers.
  let pages = $state<SyncPageRow[]>([]);
  let folders = $state<Folder[]>([]);
  let labels = $state<Label[]>([]);
  let projectLoading = $state(true);
  let loading = $derived(model === null ? projectLoading : model.coldStart);
  let error = $state("");
  let mutationsInFlight = $state(0);
  let dragId = $state<number | null>(null);
  /** A folder, or null for Unfiled (the list's group header). */
  let dropTarget = $state<number | null | undefined>(undefined);

  const view = new PageListView();

  let searchInputEl = $state<HTMLInputElement | null>(null);
  let listEl = $state<HTMLDivElement | null>(null);
  let foldersOpen = $state(false);
  let pulseOpen = $state(false);

  // ── Data ─────────────────────────────────────────────

  $effect(() => {
    const m = model;
    if (!m) return;
    const rows = m.pageList;
    if (dragId !== null || mutationsInFlight > 0) return;
    pages = rows;
  });

  $effect(() => {
    const id = projectIdentifier;
    untrack(() => {
      view.hydrate(id);
      loadData(id);
    });
  });

  $effect(() => {
    void view.snapshot();
    view.save(projectIdentifier);
  });

  async function withMutation<T>(write: () => Promise<T>): Promise<T> {
    mutationsInFlight += 1;
    try {
      return await write();
    } finally {
      await model?.pull().catch(() => {});
      mutationsInFlight -= 1;
    }
  }

  async function loadData(ident: string) {
    projectLoading = true;
    error = "";
    seenFor = null;
    const cached = cachedProject(ident);
    if (cached) {
      project = cached;
      attachModel(cached);
    } else {
      project = null;
      model = null;
      pages = [];
    }

    const projRes = await listProjects();
    if (!projRes.ok) {
      if (!cached) {
        error = projRes.error;
        projectLoading = false;
        return;
      }
    } else {
      cacheProjects(projRes.data);
      const found = projRes.data.find((p: Project) => p.identifier === ident);
      if (!found) {
        error = `Project ${ident} not found`;
        projectLoading = false;
        return;
      }
      if (ident !== projectIdentifier) return;
      project = found;
      attachModel(found);
    }

    const resolved = project;
    if (!resolved) {
      projectLoading = false;
      return;
    }
    loadProjectRole(resolved.id);

    const [fRes, lRes] = await Promise.all([listFolders(resolved.id), listLabels(resolved.id)]);
    if (ident !== projectIdentifier) return;
    if (fRes.ok) folders = fRes.data;
    if (lRes.ok) labels = lRes.data;
    projectLoading = false;
  }

  function attachModel(found: Project) {
    model = ensureProjectModel(found.id);
    setActiveProject(found.id);
    metaMap = metaCache.get(found.id) ?? new Map();
    metaLoaded = metaCache.has(found.id);
  }

  // ── Page metadata ────────────────────────────────────
  // Reloaded a moment after any row moves; cached per project so a revisit
  // paints excerpts and editors on the first frame.
  const metaCache = new Map<number, Map<number, PageMeta>>();
  let metaMap = $state<Map<number, PageMeta>>(new Map());
  let metaLoaded = $state(false);
  let metaTimer: ReturnType<typeof setTimeout> | null = null;

  const pagesStamp = $derived(pages.reduce((n, p) => Math.max(n, p.seq), 0) + ":" + pages.length);

  $effect(() => {
    const pid = project?.id;
    void pagesStamp;
    if (pid == null || loading) return;
    if (metaTimer) clearTimeout(metaTimer);
    const delay = metaCache.has(pid) ? 700 : 0;
    metaTimer = setTimeout(async () => {
      const res = await getPagesMeta(pid);
      if (!res.ok || project?.id !== pid) return;
      const next = new Map(res.data.pages.map((m) => [m.id, m]));
      metaCache.set(pid, next);
      metaMap = next;
      metaLoaded = true;
    }, delay);
    return () => {
      if (metaTimer) clearTimeout(metaTimer);
    };
  });

  const myId = $derived($currentUser?.id ?? null);

  // ── Derived lists ────────────────────────────────────

  let conventions = $derived(conventionsState(pages));
  /** Every page but the conventions page, which has the project's
   *  Conventions tab. Nothing in this view lists, previews or edits it. */
  let treePages = $derived(pages.filter((p) => !p.conventions));
  let livePages = $derived(treePages.filter((p) => p.status !== "archived"));
  let archivedPages = $derived(treePages.filter((p) => p.status === "archived"));
  let folderIds = $derived(new Set(folders.map((f) => f.id)));
  let stats = $derived(folderStats(livePages, folders));
  let scope = $derived<FolderScope>(
    typeof view.scope === "number" && !folderIds.has(view.scope) && !projectLoading ? "all" : view.scope,
  );
  let scopeIds = $derived(typeof scope === "number" ? descendantIds(folders, scope) : null);
  let scopeLabel = $derived(
    scope === "all"
      ? "All pages"
      : scope === "archived"
        ? "Archived"
        : folderPathLabel(folders, scope) || "Folder",
  );
  let conventionsEntry = $derived<"on" | "off" | "missing">(
    conventions.kind === "page" ? (conventions.page.conventions_enabled !== false ? "on" : "off") : "missing",
  );
  let labelColors = $derived(new Map(labels.map((l) => [l.name, safeLabelColor(l.color)])));

  // Archived pages all share one status, so the status filter stands aside.
  let activeFilters = $derived(scope === "archived" ? { ...view.filters, statuses: [] } : view.filters);
  let scopedPages = $derived(
    scope === "archived" ? archivedPages : livePages.filter((p) => inScope(p, scope, folders, scopeIds)),
  );
  let filtered = $derived(applyFilters(scopedPages, activeFilters, metaMap));
  let showPinnedStrip = $derived(scope === "all" && filterCount(activeFilters) === 0);
  let pinnedPages = $derived(
    showPinnedStrip ? filtered.filter((p) => p.pinned).sort((a, b) => b.updated_at.localeCompare(a.updated_at)) : [],
  );
  let groups = $derived.by<PageGroupBlock[]>(() => {
    const rows = showPinnedStrip ? filtered.filter((p) => !p.pinned) : filtered;
    return groupPages(sortPages(rows, view.sort), view.group, {
      folders,
      labelColors,
      scopeFolder: typeof scope === "number" ? scope : undefined,
    });
  });

  interface SearchHit {
    page: SyncPageRow;
    score: number;
    snippet: string | null;
  }

  let searchHits = $derived.by<SearchHit[]>(() => {
    const q = view.search.trim();
    if (!q) return [];
    const hits: SearchHit[] = [];
    for (const page of treePages) {
      const body = metaMap.get(page.id)?.excerpt || page.preview;
      const titleHit = fuzzyMatch(q, page.title);
      const idHit = fuzzyMatch(q, page.identifier);
      const contentHit = fuzzyMatch(q, body);
      const labelHit = page.labels.length > 0 ? fuzzyMatch(q, page.labels.join(" ")) : null;
      const titleScore = titleHit?.score ?? 0;
      const idScore = (idHit?.score ?? 0) * IDENTIFIER_WEIGHT;
      const contentScore = (contentHit?.score ?? 0) * CONTENT_WEIGHT;
      const labelScore = (labelHit?.score ?? 0) * LABEL_WEIGHT;
      const best = Math.max(titleScore, idScore, contentScore, labelScore);
      if (best < SCORE_THRESHOLD) continue;
      const snippet =
        contentHit && contentScore === best && best > 0 ? buildSnippet(body, contentHit.matchStart, contentHit.matchEnd) : null;
      hits.push({ page, score: best, snippet });
    }
    hits.sort((a, b) => b.score - a.score);
    return hits.slice(0, RESULT_CAP);
  });

  let searching = $derived(view.search.trim() !== "");

  /** The rows on screen, in order: what J/K walks and Ctrl+A selects. */
  let flatRows = $derived.by<ListPage[]>(() => {
    if (searching) return searchHits.map((h) => h.page);
    return [...pinnedPages, ...groups.flatMap((g) => g.pages)];
  });

  // Label groups can list a page twice; count it once.
  let shownCount = $derived(
    loading ? null : searching ? flatRows.length : new Set(flatRows.map((p) => p.id)).size,
  );

  let editors = $derived.by<PageMetaPerson[]>(() => {
    const seen = new Map<number, PageMetaPerson>();
    for (const m of metaMap.values()) if (m.updated_by) seen.set(m.updated_by.id, m.updated_by);
    return [...seen.values()].sort((a, b) => Number(a.is_bot) - Number(b.is_bot) || personLabel(a).localeCompare(personLabel(b)));
  });

  // ── Reading pane ─────────────────────────────────────

  let wide = $state(false);
  $effect(() => {
    const mq = window.matchMedia("(min-width: 1280px)");
    wide = mq.matches;
    const on = () => (wide = mq.matches);
    mq.addEventListener("change", on);
    return () => mq.removeEventListener("change", on);
  });

  let paneMode = $derived(wide && view.pane);
  let paneRow = $derived(paneMode && view.focusedId != null ? (treePages.find((p) => p.id === view.focusedId) ?? null) : null);

  const pagePath = (id: number) => `/${projectIdentifier}/pages/${id}`;

  function resolveRef(identifier: string): string | null {
    const page = pages.find((p) => p.identifier.toLowerCase() === identifier.toLowerCase());
    if (page) return pagePath(page.id);
    if (/-DOC-\d+$/i.test(identifier) || /-PLAN-\d+$/i.test(identifier)) return null;
    return `/${identifier.split("-")[0]}/issues/${identifier}`;
  }

  function open(page: ListPage) {
    markSeen(page);
    navigate(pagePath(page.id));
  }

  function activate(page: ListPage) {
    if (paneMode) {
      view.focusedId = page.id;
      markSeen(page);
    } else {
      open(page);
    }
  }

  function peek(page: ListPage) {
    markSeen(page);
    openPagePeek(page.id, pagePath(page.id));
  }

  // ── Changed since the last visit ─────────────────────

  let seenMap = $state<SeenMap>({});
  let seenFor = $state<string | null>(null);
  const seenKey = (ident: string) => `pages:${ident}`;

  $effect(() => {
    const ident = projectIdentifier;
    if (loading || model?.status !== "ready" || seenFor === ident) return;
    untrack(() => {
      const stored = loadSeen(seenKey(ident));
      seenMap = stored ?? snapshotOf(pages);
      if (!stored) saveSeen(seenKey(ident), seenMap);
      seenFor = ident;
    });
  });

  $effect(() => {
    const ident = projectIdentifier;
    return () => {
      if (seenFor !== ident) return;
      saveSeen(seenKey(ident), model?.status === "ready" ? pruneSeen(seenMap, pages) : seenMap);
    };
  });

  function markSeen(page: ListPage) {
    if (seenFor !== projectIdentifier) return;
    const next = withSeen(seenMap, page);
    if (next === seenMap) return;
    seenMap = next;
    saveSeen(seenKey(projectIdentifier), next);
  }

  let changedIds = $derived.by(() => {
    const out = new Set<number>();
    if (seenFor !== projectIdentifier) return out;
    for (const p of livePages) {
      if (isSeen(seenMap, p)) continue;
      // Your own edits are not news.
      const by = metaMap.get(p.id)?.updated_by;
      if (by && myId != null && by.id === myId) continue;
      out.add(p.id);
    }
    return out;
  });

  let changedCount = $derived(flatRows.reduce((n, p) => n + (changedIds.has(p.id) ? 1 : 0), 0));

  function cycleChanged() {
    const n = flatRows.length;
    if (!n) return;
    const start = flatRows.findIndex((p) => p.id === view.focusedId);
    for (let step = 1; step <= n; step++) {
      const row = flatRows[(start + step + n) % n];
      if (!changedIds.has(row.id)) continue;
      focusRow(row.id);
      markSeen(row);
      return;
    }
  }

  // ── Rows that just changed glow once ─────────────────
  let flashIds = $state<Set<number>>(new Set());
  let prevStamps: Map<number, string> | null = null;
  let stampProject: string | null = null;
  $effect(() => {
    const list = pages;
    const proj = projectIdentifier;
    if (loading) return;
    const next = new Map(list.map((p) => [p.id, p.updated_at]));
    const prev = prevStamps;
    const same = stampProject === proj;
    prevStamps = next;
    stampProject = proj;
    if (!prev || !same) return;
    const changed: number[] = [];
    for (const [id, stamp] of next) if (prev.get(id) !== stamp) changed.push(id);
    if (changed.length === 0 || changed.length > 8) return;
    untrack(() => {
      flashIds = new Set([...flashIds, ...changed]);
      setTimeout(() => {
        const keep = new Set(flashIds);
        for (const id of changed) keep.delete(id);
        flashIds = keep;
      }, 1400);
    });
  });

  // ── Focus and keyboard ───────────────────────────────

  function focusRow(id: number | null) {
    view.focusedId = id;
    if (id == null) return;
    requestAnimationFrame(() => {
      listEl?.querySelector<HTMLElement>(`[data-page-id="${id}"]`)?.scrollIntoView({ block: "nearest" });
    });
  }

  function move(delta: number) {
    const n = flatRows.length;
    if (!n) return;
    const i = flatRows.findIndex((p) => p.id === view.focusedId);
    const next = i < 0 ? (delta > 0 ? 0 : n - 1) : Math.min(n - 1, Math.max(0, i + delta));
    focusRow(flatRows[next].id);
    if (paneMode) markSeen(flatRows[next]);
  }

  function focusedRow(): ListPage | null {
    return flatRows.find((p) => p.id === view.focusedId) ?? null;
  }

  function onKeydown(e: KeyboardEvent) {
    if (shortcutsSuppressed() || pagePeekState.open) return;
    if (e.altKey) return;
    const mod = e.ctrlKey || e.metaKey;
    if (mod && e.key.toLowerCase() === "a" && canEdit && flatRows.length) {
      e.preventDefault();
      view.selected = new Set(flatRows.map((p) => p.id));
      return;
    }
    if (mod) return;
    switch (e.key) {
      case "j":
      case "J":
      case "ArrowDown":
        e.preventDefault();
        move(1);
        return;
      case "k":
      case "K":
      case "ArrowUp":
        e.preventDefault();
        move(-1);
        return;
      case "Home":
        if (flatRows.length) { e.preventDefault(); focusRow(flatRows[0].id); }
        return;
      case "End":
        if (flatRows.length) { e.preventDefault(); focusRow(flatRows[flatRows.length - 1].id); }
        return;
      case "Enter": {
        const row = focusedRow();
        if (row) { e.preventDefault(); open(row); }
        return;
      }
      case " ": {
        const row = focusedRow();
        if (row && !paneMode) { e.preventDefault(); peek(row); }
        return;
      }
      case "x":
      case "X": {
        const row = focusedRow();
        if (row && canEdit) { e.preventDefault(); view.toggleSelected(row.id); }
        return;
      }
      case "p":
      case "P": {
        const row = focusedRow();
        if (row && canEdit) { e.preventDefault(); void togglePin(row); }
        return;
      }
      case "c":
      case "C":
        if (canEdit) { e.preventDefault(); startCreate(); }
        return;
      case "/":
        e.preventDefault();
        view.searchOpen = true;
        requestAnimationFrame(() => searchInputEl?.focus());
        return;
      case "f":
      case "F": {
        e.preventDefault();
        const openNow = !view.filterOpen;
        view.closePopovers();
        view.filterOpen = openNow;
        return;
      }
      case "Escape":
        if (view.filterOpen || view.displayOpen) view.closePopovers();
        else if (foldersOpen || pulseOpen) { foldersOpen = false; pulseOpen = false; }
        else if (view.selected.size) view.selected = new Set();
        else if (view.focusedId != null) view.focusedId = null;
        else return;
        e.preventDefault();
        return;
    }
  }

  function autofocus(node: HTMLInputElement) {
    requestAnimationFrame(() => node.focus());
  }

  $effect(() => {
    window.addEventListener("lific:pages-search-done", onSearchDone);
    return () => window.removeEventListener("lific:pages-search-done", onSearchDone);
  });

  function onSearchDone(e: Event) {
    const key = (e as CustomEvent<string>).detail;
    const first = flatRows[0];
    if (!first) return;
    if (key === "Enter" && searching) open(first);
    else focusRow(first.id);
  }

  // ── Writes ───────────────────────────────────────────

  function patchLocal(ids: Iterable<number>, patch: Partial<SyncPageRow>) {
    const set = new Set(ids);
    pages = pages.map((p) => (set.has(p.id) ? { ...p, ...patch } : p));
  }

  async function togglePin(page: ListPage) {
    const next = !page.pinned;
    patchLocal([page.id], { pinned: next });
    const res = await withMutation(() => updatePage(page.id, { pinned: next }));
    if (!res.ok) {
      patchLocal([page.id], { pinned: !next });
      toast(`Couldn't ${next ? "pin" : "unpin"} ${page.identifier}: ${res.error}`, { kind: "error" });
    }
    refreshProjectModel(project?.id);
  }

  async function setStatus(page: ListPage, status: string) {
    const before = page.status;
    patchLocal([page.id], { status });
    const res = await withMutation(() => updatePage(page.id, { status }));
    if (!res.ok) {
      patchLocal([page.id], { status: before });
      toast(`Couldn't change ${page.identifier}: ${res.error}`, { kind: "error" });
    } else if (status === "archived" || before === "archived") {
      toast(`${page.identifier} ${status === "archived" ? "archived" : "restored"}`, {
        kind: "success",
        duration: 2500,
      });
    }
    refreshProjectModel(project?.id);
  }

  async function moveToFolder(page: ListPage, folderId: number | null): Promise<boolean> {
    const before = page.folder_id ?? null;
    if (before === folderId) return true;
    patchLocal([page.id], { folder_id: folderId });
    const res = await withMutation(() => updatePage(page.id, { folder_id: folderId }));
    refreshProjectModel(project?.id);
    if (!res.ok) {
      patchLocal([page.id], { folder_id: before });
      toast(`Couldn't move ${page.identifier}: ${res.error}`, { kind: "error" });
      return false;
    }
    const where = folderId == null ? "Unfiled" : (folders.find((f) => f.id === folderId)?.name ?? "the folder");
    toast(`Moved ${page.identifier} to ${where}`, { kind: "success", duration: 2500 });
    return true;
  }

  // Bulk: one request per page, applied optimistically, reported once.
  let bulkBusy = $state(false);
  async function bulk(
    patch: (p: SyncPageRow) => Partial<SyncPageRow> | null,
    write: (p: SyncPageRow) => Parameters<typeof updatePage>[1],
    done: string,
  ) {
    const targets = treePages.filter((p) => view.selected.has(p.id));
    const work = targets.map((p) => ({ p, change: patch(p) })).filter((x) => x.change);
    if (!work.length) return;
    bulkBusy = true;
    const before = new Map(work.map(({ p }) => [p.id, p]));
    pages = pages.map((p) => {
      const w = work.find((x) => x.p.id === p.id);
      return w ? { ...p, ...w.change } : p;
    });
    const results = await withMutation(() => Promise.all(work.map(({ p }) => updatePage(p.id, write(p)))));
    const failed = work.filter((_, i) => !results[i].ok);
    if (failed.length) {
      pages = pages.map((p) => (failed.some((f) => f.p.id === p.id) ? before.get(p.id)! : p));
      toast(`${failed.length} of ${work.length} pages didn't change`, { kind: "error" });
    } else {
      toast(`${done} ${work.length} ${work.length === 1 ? "page" : "pages"}`, { kind: "success", duration: 2500 });
      view.selected = new Set();
    }
    bulkBusy = false;
    refreshProjectModel(project?.id);
  }

  async function bulkExport() {
    const ids = treePages.filter((p) => view.selected.has(p.id)).map((p) => p.identifier);
    bulkBusy = true;
    try {
      const blob = await selectedIssueExport(ids, "pages");
      saveArchiveDownload(blob, `${projectIdentifier}-pages-${ids.length}.md`);
    } catch (err) {
      toast(err instanceof Error ? err.message : "Export failed", { kind: "error" });
    } finally {
      bulkBusy = false;
    }
  }

  // ── Create ───────────────────────────────────────────

  let composer = $state<{ status: "draft" | "active"; folderId: number | null } | null>(null);
  let composerTitle = $state("");
  let composerBusy = $state(false);

  function startCreate(folderId: number | null = typeof scope === "number" ? scope : null) {
    if (!canEdit) return;
    // The composer lives in the list; from the archive, go back to every
    // page.
    if (typeof scope !== "number" && scope !== "all") setScope("all");
    view.closePopovers();
    view.search = "";
    composer = { status: "draft", folderId };
    composerTitle = "";
    listEl?.scrollTo({ top: 0 });
  }

  async function commitCreate() {
    if (!project || !composer || !composerTitle.trim() || composerBusy) return;
    composerBusy = true;
    const res = await createPage({
      project_id: project.id,
      title: composerTitle.trim(),
      status: composer.status,
      ...(composer.folderId != null ? { folder_id: composer.folderId } : {}),
    });
    composerBusy = false;
    if (!res.ok) {
      toast(`Couldn't create the page: ${res.error}`, { kind: "error" });
      return;
    }
    composer = null;
    navigate(pagePath(res.data.id));
  }

  // ── Folders ──────────────────────────────────────────

  async function createFolderIn(parentId: number | null, name: string): Promise<boolean> {
    if (!project) return false;
    const res = await createFolder({ project_id: project.id, name, ...(parentId != null ? { parent_id: parentId } : {}) });
    if (!res.ok) {
      toast(`Couldn't create the folder: ${res.error}`, { kind: "error" });
      return false;
    }
    folders = [...folders, res.data];
    return true;
  }

  async function renameFolderTo(id: number, name: string): Promise<boolean> {
    const res = await renameFolder(id, name);
    if (!res.ok) {
      toast(`Couldn't rename the folder: ${res.error}`, { kind: "error" });
      return false;
    }
    folders = folders.map((f) => (f.id === id ? { ...f, name: res.data.name } : f));
    return true;
  }

  async function removeFolder(id: number): Promise<boolean> {
    const name = folders.find((f) => f.id === id)?.name ?? "folder";
    const gone = descendantIds(folders, id);
    const res = await withMutation(() => deleteFolder(id));
    if (!res.ok) {
      toast(`Couldn't delete ${name}: ${res.error}`, { kind: "error" });
      return false;
    }
    folders = folders.filter((f) => !gone.has(f.id));
    pages = pages.map((p) => (p.folder_id != null && gone.has(p.folder_id) ? { ...p, folder_id: null } : p));
    if (typeof view.scope === "number" && gone.has(view.scope)) view.scope = "all";
    refreshProjectModel(project?.id);
    toast(`Deleted ${name}`, { kind: "success", duration: 2500 });
    return true;
  }

  function setScope(next: FolderScope) {
    if (next !== view.scope) {
      view.selected = new Set();
      composer = null;
    }
    view.scope = next;
    view.search = "";
    view.closePopovers();
    foldersOpen = false;
    listEl?.scrollTo({ top: 0 });
  }

  async function dropOn(target: number | null) {
    const id = dragId;
    dragId = null;
    dropTarget = undefined;
    const page = pages.find((p) => p.id === id);
    if (page) await moveToFolder(page, target);
  }

  // ── Move picker (touch, and the context menu) ────────

  let movePage = $state<ListPage | null>(null);
  let moveFolderId = $state<number | null>(null);
  let moveOptions = $derived([
    { value: null, label: "Unfiled" },
    ...folders.map((f) => ({ value: f.id, label: folderPathLabel(folders, f.id) })),
  ]);

  // ── Context menu ─────────────────────────────────────

  function menuFor(page: ListPage, x: number, y: number, trigger: HTMLElement | null) {
    const items: ContextMenuItem[] = [
      { label: "Open", icon: ArrowUpRight, href: `#${pagePath(page.id)}` },
      { label: "Peek", icon: Eye, action: () => peek(page) },
      {
        label: "Copy link",
        icon: LinkIcon,
        action: () => copyToClipboard(`${location.origin}${location.pathname}#${pagePath(page.id)}`, "link"),
      },
      { label: `Copy ${page.identifier}`, icon: Copy, action: () => copyToClipboard(page.identifier) },
    ];
    if (canEdit) {
      items.push(
        { label: page.pinned ? "Unpin" : "Pin", icon: Pin, action: () => togglePin(page) },
        {
          label: "Move to folder…",
          icon: FolderInput,
          action: () => {
            movePage = page;
            moveFolderId = page.folder_id ?? null;
          },
        },
      );
      if (page.status === "archived") {
        items.push({ label: "Restore", icon: RotateCcw, action: () => setStatus(page, "active") });
      } else {
        for (const [s, icon] of [["draft", PenLine], ["active", CircleDot], ["complete", CheckCircle2]] as const) {
          items.push({
            label: `Mark ${STATUS_LABEL[s].toLowerCase()}`,
            icon,
            disabled: page.status === s,
            action: () => setStatus(page, s),
          });
        }
        items.push({ label: "Archive", icon: Archive, action: () => setStatus(page, "archived") });
      }
    }
    openContextMenu(x, y, items, trigger);
  }

  function onRowContextMenu(page: ListPage, e: MouseEvent) {
    e.preventDefault();
    e.stopPropagation();
    menuFor(page, e.clientX, e.clientY, e.currentTarget as HTMLElement);
  }

  function onRowLongPress(page: ListPage, e: PointerEvent) {
    menuFor(page, e.clientX, e.clientY, e.target as HTMLElement);
  }
</script>

<svelte:window
  onkeydown={onKeydown}
  onclick={() => view.closePopovers()}
/>

{#snippet topbarContent()}
  <PagesTopbar
    {view}
    {projectIdentifier}
    count={shownCount}
    {scope}
    {labels}
    {editors}
    {changedCount}
    onCycleChanged={cycleChanged}
    {canEdit}
    {scopeLabel}
    onNewPage={() => startCreate()}
    onOpenFolders={() => (foldersOpen = true)}
    onOpenPulse={() => (pulseOpen = true)}
    bind:searchInputEl
  />
{/snippet}

{#snippet row(page: ListPage, opts: { folder?: boolean; status?: boolean; snippet?: string | null; note?: import("svelte").Snippet } = {})}
  <PageRowView
    {page}
    meta={metaMap.get(page.id)}
    {labels}
    href="#{pagePath(page.id)}"
    folderLabel={opts.folder && page.folder_id != null ? folderPathLabel(folders, page.folder_id) : null}
    excerpts={view.excerpts}
    focused={view.focusedId === page.id}
    selected={view.selected.has(page.id)}
    selecting={view.selected.size > 0}
    changed={changedIds.has(page.id)}
    flashing={flashIds.has(page.id)}
    dragging={dragId === page.id}
    {canEdit}
    showStatus={opts.status ?? true}
    snippet={opts.snippet ?? null}
    note={opts.note}
    onActivate={activate}
    onOpen={open}
    onToggleSelect={(p) => view.toggleSelected(p.id)}
    onTogglePin={togglePin}
    onContextMenu={onRowContextMenu}
    onLongPress={onRowLongPress}
    onDragStart={(p, e) => {
      if (!canEdit) return;
      dragId = p.id;
      if (e.dataTransfer) {
        e.dataTransfer.effectAllowed = "move";
        e.dataTransfer.setData("text/plain", p.identifier);
      }
    }}
    onDragEnd={() => {
      dragId = null;
      dropTarget = undefined;
    }}
  />
{/snippet}

{#snippet emptyState(title: string, body: string, action: { label: string; run: () => void } | null = null)}
  <div class="flex flex-col items-center py-16 gap-3 px-6 max-w-[440px] mx-auto text-center" data-testid="pages-empty">
    <Mascot src="/LizzyReading.png" nativeW={527} nativeH={674} scale={0.16} />
    <p class="text-heading font-medium text-[var(--text)]">{title}</p>
    {#if body}<p class="text-body-sm text-[var(--text-muted)] leading-relaxed">{body}</p>{/if}
    {#if action}
      <button
        class="mt-1 flex items-center gap-1.5 text-body-sm font-medium text-[var(--btn-success-text)] bg-[var(--btn-success)]
               px-3 py-1.5 rounded-md hover:bg-[var(--btn-success-hover)] transition-colors"
        onclick={(e) => { e.stopPropagation(); action.run(); }}
      >
        {action.label}
      </button>
    {/if}
  </div>
{/snippet}

{#snippet sectionHead(text: string, extra: import("svelte").Snippet | null = null)}
  <div
    class="group-head sticky top-0 z-10 flex items-center gap-2 h-9 px-3 sm:px-5 border-b border-[var(--border)]
           text-caption text-[var(--text-muted)]"
  >
    <span class="font-medium text-[var(--text)]">{text}</span>
    {#if extra}{@render extra()}{/if}
  </div>
{/snippet}

<div class="h-full flex min-h-0 relative">
  <!-- Folder pane: docked from lg, a drawer below it. -->
  {#if !error}
    {#if foldersOpen}
      <button class="lg:hidden fixed inset-0 z-40 bg-black/40" aria-label="Close folders" onclick={() => (foldersOpen = false)}></button>
    {/if}
    <aside
      class="w-[272px] lg:w-[232px] shrink-0 overflow-y-auto border-r border-[var(--border)] bg-[var(--bg-subtle)] px-3 py-4
             max-lg:fixed max-lg:inset-y-0 max-lg:left-0 max-lg:z-50 max-lg:shadow-2xl max-lg:transition-transform max-lg:duration-200
             {foldersOpen ? 'max-lg:translate-x-0' : 'max-lg:-translate-x-full'}"
      aria-label="Folders"
    >
      {#if loading}
        <div class="flex flex-col gap-2 px-2">
          {#each [0, 1, 2, 3] as i (i)}<Skeleton variant="bar" class="h-4 w-3/4" />{/each}
        </div>
      {:else}
        <FolderTree
          {folders}
          {stats}
          allCount={livePages.length}
          archivedCount={archivedPages.length}
          {projectIdentifier}
          conventions={conventionsEntry}
          {scope}
          {canEdit}
          {dropTarget}
          dragging={dragId !== null}
          onScope={setScope}
          onCreate={createFolderIn}
          onRename={renameFolderTo}
          onDelete={removeFolder}
          onNewPage={(id) => { setScope(id); startCreate(id); }}
          onDragOver={(t) => (dropTarget = t)}
          onDrop={dropOn}
        />
      {/if}
    </aside>
  {/if}

  <div class="flex-1 min-w-0 flex flex-col min-h-0">
    <div class="flex-1 overflow-y-auto min-h-0 @container" bind:this={listEl} data-testid="pages-list">
      {#if loading}
        <div class="flex flex-col">
          {#each [0, 1, 2, 3, 4, 5] as i (i)}
            <div class="flex items-center gap-3 px-5 min-h-[3.4rem] border-b border-[var(--border)]">
              <Skeleton variant="circle" class="size-4 shrink-0" />
              <div class="flex-1 flex flex-col gap-1.5">
                <Skeleton variant="bar" class="h-3.5 w-full max-w-[280px]" />
                <Skeleton variant="bar" class="h-2.5 w-4/5 max-w-[420px]" />
              </div>
              <Skeleton variant="bar" class="h-3 w-12" />
            </div>
          {/each}
        </div>
      {:else if error}
        <ErrorState title="Couldn't load pages" message={error}>
          <button
            class="text-body-sm font-medium text-[var(--btn-success-text)] bg-[var(--btn-success)] px-3 py-1.5 rounded-md hover:bg-[var(--btn-success-hover)] transition-colors"
            onclick={() => loadData(projectIdentifier)}
          >
            Try again
          </button>
        </ErrorState>
      {:else if searching}
        {#if searchHits.length === 0}
          {@render emptyState(`No pages match "${view.search.trim()}"`, "Search looks at titles, identifiers, labels and the opening text.", {
            label: "Clear search",
            run: () => { view.search = ""; view.searchOpen = false; },
          })}
        {:else}
          {#snippet hitsNote()}
            <span class="text-caption text-[var(--text-faint)]">by relevance{searchHits.length === RESULT_CAP ? `, top ${RESULT_CAP}` : ""}</span>
          {/snippet}
          {@render sectionHead(`${searchHits.length} ${searchHits.length === 1 ? "match" : "matches"}`, hitsNote)}
          {#each searchHits as hit (hit.page.id)}
            {@render row(hit.page, { folder: true, snippet: hit.snippet })}
          {/each}
        {/if}
      {:else}
        {#if scope === "all" && treePages.length === 0 && folders.length === 0 && !composer}
          {@render emptyState(
            "No pages yet",
            "Pages hold the project's docs: specs, notes, decisions. Agents can read and write them too.",
            canEdit ? { label: "New page", run: () => startCreate() } : null,
          )}
        {:else}
          {#if pinnedPages.length > 0}
            <div class="flex items-center gap-2 px-3 sm:px-5 py-2 border-b border-[var(--border)] overflow-x-auto [scrollbar-width:none]" data-testid="pinned-strip">
              <Pin size={12} class="shrink-0 text-[var(--text-faint)]" aria-label="Pinned" />
              {#each pinnedPages as page (page.id)}
                <a
                  href="#{pagePath(page.id)}"
                  class="shrink-0 max-w-[16rem] flex items-center gap-1.5 h-7 px-2.5 rounded-md text-body-sm text-[var(--text)]
                         bg-[var(--surface)] ring-1 ring-[var(--border)] hover:ring-[var(--accent)] transition-shadow
                         {view.focusedId === page.id ? 'ring-[var(--accent)]' : ''}"
                  data-page-id={page.id}
                  onclick={(e) => {
                    if (e.metaKey || e.ctrlKey || e.shiftKey || e.button !== 0) return;
                    e.preventDefault();
                    activate(page);
                  }}
                  oncontextmenu={(e) => onRowContextMenu(page, e)}
                >
                  <FileText size={13} class="shrink-0 text-[var(--text-faint)]" />
                  <span class="truncate">{page.title || "Untitled"}</span>
                  {#if changedIds.has(page.id)}<span class="size-1.5 rounded-full bg-[var(--accent)] shrink-0"></span>{/if}
                </a>
              {/each}
            </div>
          {/if}

          {#if composer}
            <!-- svelte-ignore a11y_click_events_have_key_events, a11y_no_static_element_interactions -->
            <div
              class="composer-in flex flex-wrap items-center gap-2 px-3 sm:px-5 py-2.5 border-b border-[var(--border)]
                     bg-[color-mix(in_srgb,var(--accent-subtle)_45%,transparent)]"
              data-testid="page-composer"
              onclick={(e) => e.stopPropagation()}
            >
              <FileText size={15} class="shrink-0 text-[var(--accent)]" />
              <input
                class="flex-1 min-w-[12rem] h-8 px-2 rounded-md text-body bg-[var(--surface)] text-[var(--text)]
                       border border-[var(--border)] focus:border-[var(--accent)] focus:shadow-[0_0_0_3px_var(--accent-subtle)] outline-none"
                placeholder="Page title"
                aria-label="New page title"
                bind:value={composerTitle}
                disabled={composerBusy}
                use:autofocus
                onkeydown={(e) => {
                  if (e.key === "Enter") { e.preventDefault(); void commitCreate(); }
                  if (e.key === "Escape") { e.preventDefault(); e.stopPropagation(); composer = null; }
                }}
              />
              <div class="inline-flex rounded-md border border-[var(--border)] overflow-hidden" role="group" aria-label="Start as">
                {#each ["draft", "active"] as const as s (s)}
                  <button
                    class="h-8 px-2.5 text-caption font-medium transition-colors
                           {composer.status === s ? 'bg-[var(--surface)] text-[var(--text)]' : 'text-[var(--text-muted)] hover:text-[var(--text)]'}
                           {s === 'active' ? 'border-l border-[var(--border)]' : ''}"
                    aria-pressed={composer.status === s}
                    onclick={() => composer && (composer = { ...composer, status: s })}
                  >
                    {STATUS_LABEL[s]}
                  </button>
                {/each}
              </div>
              <span class="flex items-center gap-1 text-caption text-[var(--text-muted)] min-w-0">
                <FolderClosed size={13} class="shrink-0 text-[var(--text-faint)]" />
                <span class="truncate max-w-[12rem]">{composer.folderId != null ? folderPathLabel(folders, composer.folderId) : "Unfiled"}</span>
              </span>
              <button
                class="h-8 px-3 rounded-md text-body-sm font-medium text-[var(--btn-success-text)] bg-[var(--btn-success)]
                       hover:bg-[var(--btn-success-hover)] disabled:opacity-50 transition-colors"
                disabled={!composerTitle.trim() || composerBusy}
                onclick={commitCreate}
              >
                Create
              </button>
              <button class="size-8 grid place-items-center rounded-md text-[var(--text-faint)] hover:text-[var(--text)]" aria-label="Cancel" onclick={() => (composer = null)}>
                <X size={14} />
              </button>
            </div>
          {/if}

          {#if groups.every((g) => g.pages.length === 0) && pinnedPages.length === 0}
            {#if filterCount(activeFilters) > 0}
              {@render emptyState("No pages match these filters", "", { label: "Clear filters", run: () => view.clearFilters() })}
            {:else if typeof scope === "number"}
              {@render emptyState(
                `No pages in ${scopeLabel} yet`,
                "",
                canEdit ? { label: "New page here", run: () => startCreate(scope as number) } : null,
              )}
            {:else if scope === "archived"}
              {@render emptyState("Nothing archived", "Archived pages are kept here, out of the way of the rest.")}
            {:else if !composer}
              {@render emptyState("No pages yet", "", canEdit ? { label: "New page", run: () => startCreate() } : null)}
            {/if}
          {:else}
            {#each groups as group (group.key)}
              {#if group.label && (view.group !== "folder" || groups.length > 1 || typeof scope !== "number")}
                <div
                  class="group group-head sticky top-0 z-10 flex items-center gap-2 h-9 px-3 sm:px-5 border-b border-[var(--border)]
                         text-caption text-[var(--text-muted)]
                         {dragId !== null && group.folderId !== undefined && dropTarget === group.folderId ? 'outline outline-1 outline-dashed outline-[var(--accent)]' : ''}"
                  role="presentation"
                  data-group={group.key}
                  ondragover={(e) => {
                    if (dragId === null || group.folderId === undefined) return;
                    e.preventDefault();
                    dropTarget = group.folderId;
                  }}
                  ondrop={(e) => {
                    if (group.folderId === undefined) return;
                    e.preventDefault();
                    void dropOn(group.folderId);
                  }}
                >
                  {#if group.folderId !== undefined}
                    <FolderClosed size={13} class="shrink-0 text-[var(--text-faint)]" />
                  {:else if group.labelColor}
                    <span class="size-2 rounded-full shrink-0" style="background: {group.labelColor}"></span>
                  {/if}
                  {#if typeof group.folderId === "number" && group.folderId !== scope}
                    <button class="font-medium text-[var(--text)] hover:underline truncate" onclick={(e) => { e.stopPropagation(); setScope(group.folderId as number); }}>
                      {group.label}
                    </button>
                  {:else}
                    <span class="font-medium text-[var(--text)] truncate">{group.label}</span>
                  {/if}
                  <span class="tabular-nums text-[var(--text-faint)]">{group.pages.length}</span>
                  {#if group.pages.length > 0 && view.group !== "edited"}
                    <span class="hidden sm:inline text-[var(--text-faint)]">
                      · updated <TimeAgo date={group.pages.reduce((m, p) => (p.updated_at > m ? p.updated_at : m), "")} />
                    </span>
                  {/if}
                  {#if canEdit && group.folderId !== undefined}
                    <button
                      class="ml-auto flex items-center gap-1 h-6 px-1.5 rounded text-[var(--text-faint)] hover:text-[var(--text)]
                             hover:bg-[var(--bg-subtle)] opacity-0 group-hover:opacity-100 pointer-coarse:opacity-100 focus-visible:opacity-100 transition-opacity"
                      onclick={(e) => { e.stopPropagation(); startCreate(group.folderId ?? null); }}
                    >
                      <Plus size={12} /> New page here
                    </button>
                  {/if}
                </div>
              {/if}
              {#if group.pages.length === 0}
                <p class="px-5 sm:pl-12 py-3 text-caption text-[var(--text-faint)] border-b border-[var(--border)]">
                  No pages directly in this folder.
                </p>
              {/if}
              {#each group.pages as page (page.id)}
                {#if scope === "archived"}
                  {#snippet restore()}
                    {#if canEdit}
                      <button
                        class="shrink-0 h-6 px-2 rounded-md text-caption font-medium text-[var(--text-muted)] ring-1 ring-[var(--border)]
                               hover:text-[var(--text)] hover:ring-[var(--accent)] transition-shadow"
                        onclick={(e) => { e.stopPropagation(); void setStatus(page, "active"); }}
                      >
                        Restore
                      </button>
                    {/if}
                  {/snippet}
                  {@render row(page, { folder: view.group !== "folder", status: false, note: restore })}
                {:else}
                  {@render row(page, { folder: view.group !== "folder" })}
                {/if}
              {/each}
            {/each}
          {/if}
        {/if}
      {/if}
    </div>

    {#if !loading && !error && view.selected.size === 0}
      {@const focused = focusedRow() !== null}
      <div
        class="hint-bar hidden md:[@media(hover:hover)]:flex shrink-0 items-center gap-4 px-6 h-8 overflow-hidden whitespace-nowrap
               border-t border-[var(--border)] bg-[var(--bg)] text-micro text-[var(--text-faint)]"
        aria-hidden="true"
      >
        {#if composer}
          <span><kbd class="hint-kbd">Enter</kbd> create and open</span>
          <span><kbd class="hint-kbd">Esc</kbd> cancel</span>
        {:else if focused}
          <span><kbd class="hint-kbd">Enter</kbd> open</span>
          {#if !paneMode}<span><kbd class="hint-kbd">Space</kbd> peek</span>{/if}
          {#if canEdit}
            <span><kbd class="hint-kbd">P</kbd> pin</span>
            <span><kbd class="hint-kbd">X</kbd> select</span>
          {/if}
          <span><kbd class="hint-kbd">Esc</kbd> {paneMode ? "close the preview" : "unfocus"}</span>
        {:else}
          <span><kbd class="hint-kbd">J</kbd><kbd class="hint-kbd">K</kbd> move</span>
          {#if canEdit}<span><kbd class="hint-kbd">C</kbd> new page</span>{/if}
          <span><kbd class="hint-kbd">F</kbd> filter</span>
          <span><kbd class="hint-kbd">/</kbd> search</span>
          <span><kbd class="hint-kbd">?</kbd> all shortcuts</span>
        {/if}
        <span class="ml-auto pl-4 tabular-nums hidden lg:inline">
          {flatRows.length} shown
        </span>
      </div>
    {/if}

    {#if view.selected.size > 0 && canEdit}
      <PageBulkBar
        count={view.selected.size}
        busy={bulkBusy}
        {folders}
        {labels}
        onMove={(fid) => bulk((p) => ((p.folder_id ?? null) === fid ? null : { folder_id: fid }), () => ({ folder_id: fid }), "Moved")}
        onStatus={(s) => bulk((p) => (p.status === s ? null : { status: s }), () => ({ status: s }), s === "archived" ? "Archived" : "Updated")}
        onLabel={(name) =>
          bulk(
            (p) => (p.labels.includes(name) ? null : { labels: [...p.labels, name] }),
            (p) => ({ labels: [...p.labels, name] }),
            "Labeled",
          )}
        onPin={(on) => bulk((p) => (p.pinned === on ? null : { pinned: on }), () => ({ pinned: on }), on ? "Pinned" : "Unpinned")}
        onExport={bulkExport}
        onClear={() => (view.selected = new Set())}
      />
    {/if}
  </div>

  <!-- Reading pane, or the page activity while nothing is open. -->
  {#if !error}
    {#if pulseOpen}
      <button class="xl:hidden fixed inset-0 z-40 bg-black/40" aria-label="Close activity" onclick={() => (pulseOpen = false)}></button>
    {/if}
    <aside
      class="w-[min(340px,88vw)] xl:w-[clamp(300px,32%,440px)] shrink-0 border-l border-[var(--border)] bg-[var(--surface)] min-h-0 flex flex-col
             max-xl:fixed max-xl:inset-y-0 max-xl:right-0 max-xl:z-50 max-xl:shadow-2xl max-xl:transition-transform max-xl:duration-200
             {pulseOpen ? 'max-xl:translate-x-0' : 'max-xl:translate-x-full'}"
      aria-label={paneRow ? "Page preview" : "Page activity"}
    >
      {#if paneRow && wide}
        {#key paneRow.id}
          <ReadingPane
            row={paneRow}
            meta={metaMap.get(paneRow.id)}
            {folders}
            {labels}
            href="#{pagePath(paneRow.id)}"
            {canEdit}
            {resolveRef}
            {navigate}
            onOpen={() => paneRow && open(paneRow)}
            onClose={() => (view.focusedId = null)}
            onTogglePin={() => paneRow && togglePin(paneRow)}
            onStatus={(s) => paneRow && setStatus(paneRow, s)}
          />
        {/key}
      {:else if !loading}
        <div class="xl:hidden flex justify-end px-3 pt-3 -mb-3">
          <button class="size-8 grid place-items-center rounded-md text-[var(--text-muted)] hover:bg-[var(--bg-subtle)]" aria-label="Close activity" onclick={() => (pulseOpen = false)}>
            <X size={16} />
          </button>
        </div>
        <PagesPulse
          pages={livePages}
          meta={metaMap}
          {metaLoaded}
          onPick={(p) => { pulseOpen = false; activate(p); }}
          onShowDrafts={() => {
            pulseOpen = false;
            setScope("all");
            view.filters = { ...view.filters, statuses: ["draft"] };
          }}
          onShowUnlinked={() => {
            pulseOpen = false;
            setScope("all");
            view.filters = { ...view.filters, unlinked: true };
          }}
        />
      {/if}
    </aside>
  {/if}
</div>

{#if movePage}
  <!-- svelte-ignore a11y_no_static_element_interactions, a11y_click_events_have_key_events -->
  <div
    class="fixed inset-0 z-[100] bg-black/25 flex items-start justify-center pt-[14dvh] px-2"
    onclick={(e) => { if (e.target === e.currentTarget) movePage = null; }}
    onkeydown={(e) => { if (e.key === "Escape") movePage = null; }}
  >
    <div
      class="w-full sm:max-w-[420px] bg-[var(--surface)] border border-[var(--border)] rounded-xl shadow-[0_16px_48px_rgba(0,0,0,0.28)]"
      role="dialog"
      aria-modal="true"
      aria-label="Move page to folder"
      tabindex="-1"
    >
      <div class="flex items-center gap-3 px-4 py-3 border-b border-[var(--border)]">
        <FolderInput size={16} class="shrink-0 text-[var(--accent)]" />
        <div class="flex-1 min-w-0">
          <h2 class="text-body-lg font-semibold text-[var(--text)]">Move to folder</h2>
          <p class="text-caption text-[var(--text-faint)] truncate">{movePage.title}</p>
        </div>
        <button class="size-7 grid place-items-center rounded-md text-[var(--text-muted)] hover:bg-[var(--bg-subtle)]" aria-label="Close" onclick={() => (movePage = null)}>
          <X size={15} />
        </button>
      </div>
      <div class="px-4 py-4">
        <Select
          options={moveOptions}
          bind:value={moveFolderId}
          onchange={async (o) => {
            const target = movePage;
            if (target && (await moveToFolder(target, o.value as number | null))) movePage = null;
          }}
        />
      </div>
    </div>
  </div>
{/if}

<style>
  .group-head {
    background: color-mix(in srgb, var(--bg-subtle) 55%, var(--bg));
  }
  @keyframes composer-in {
    from {
      opacity: 0;
      transform: translateY(-6px);
    }
  }
  .composer-in {
    animation: composer-in 180ms var(--ease-out-expo);
  }
  .hint-kbd {
    display: inline-grid;
    place-items: center;
    min-width: 1.15rem;
    height: 1.15rem;
    padding: 0 0.3rem;
    margin-right: 0.3rem;
    border-radius: 4px;
    border: 1px solid var(--border);
    background: var(--bg-subtle);
    color: var(--text-muted);
    font-family: var(--font-mono);
    font-size: 0.625rem;
    line-height: 1;
  }
  .hint-kbd + .hint-kbd {
    margin-left: -0.15rem;
  }
  .hint-bar > span {
    flex-shrink: 0;
  }
</style>

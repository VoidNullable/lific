<script lang="ts">
  import {
    listPages,
    listFolders,
    listProjects,
    listLabels,
    createPage,
    createFolder,
    deleteFolder,
    updatePage,
    type Page,
    type Folder,
    type Project,
    type Label,
  } from "../lib/api";
  import {
    FileText,
    FolderOpen,
    FolderClosed,
    Plus,
    ChevronRight,
    ChevronDown,
    Trash2,
    X,
    Search,
    PenLine,
    CircleDot,
    CheckCircle2,
    Archive,
    Pin,
    PinOff,
    FolderPlus,
    ClipboardPaste,
    NotebookPen,
    PanelRight,
  } from "lucide-svelte";
  import Select from "../lib/Select.svelte";
  import SubTabs from "../lib/SubTabs.svelte";
  import Tooltip from "../lib/Tooltip.svelte";
  import TimeAgo from "../lib/TimeAgo.svelte";
  import Mascot from "../lib/Mascot.svelte";
  import { fuzzyMatch, buildSnippet } from "../lib/fuzzy";
  import ErrorState from "../lib/ErrorState.svelte";
  import Skeleton from "../lib/Skeleton.svelte";
  import { getContext } from "svelte";
  import { startAutoRefresh } from "../lib/autoRefresh.svelte";
  import { projectRole, loadProjectRole } from "../lib/projectRole.svelte"; // LIF-234
  import { loadSubTab, saveSubTab } from "../lib/subtab";
  import { toast } from "../lib/toast/toast.svelte";

  // LIF-234: pages are content — create/edit/delete + folder management are
  // maintainer-gated. A viewer browses the tree read-only.
  const canEdit = $derived(projectRole.canEdit);

  // Register the toolbar with Layout's chrome topbar slot so it sits in
  // the same --chrome zone as the sidebar instead of as a banded strip
  // inside the recessed content panel.
  const topbarCtx = getContext<{
    set: (s: import("svelte").Snippet | undefined) => void;
  } | undefined>("lific:topbar");

  $effect(() => {
    topbarCtx?.set(topbarContent);
    return () => topbarCtx?.set(undefined);
  });

  // LIF-118: fuzzy search tuning constants.
  //
  //   SCORE_THRESHOLD  — minimum match score to surface. The scorer caps
  //                      fuzzy (non-substring) matches at 0.7; 0.25 keeps
  //                      reasonable subsequence hits but trims the long
  //                      tail of "technically matched" noise.
  //   RESULT_CAP       — hard cap so a generic 1-char query doesn't blow
  //                      up the DOM. Sorted by score desc, so the user
  //                      always sees the strongest matches.
  //   CONTENT_SCAN_MAX — cap content scanned per page. The scorer is
  //                      linear in haystack length; this keeps worst-case
  //                      cost per keystroke bounded even if a project
  //                      has a few huge pages.
  //   CONTENT_WEIGHT   — content matches discount relative to title.
  //                      Content false positives are more common in long
  //                      bodies, so we want title/identifier hits to win
  //                      ties.
  const SCORE_THRESHOLD = 0.25;
  const RESULT_CAP = 50;
  const CONTENT_SCAN_MAX = 4000;
  const CONTENT_WEIGHT = 0.6;
  const IDENTIFIER_WEIGHT = 0.9;

  let {
    navigate,
    projectIdentifier,
  }: {
    navigate: (path: string) => void;
    projectIdentifier: string;
  } = $props();

  let project = $state<Project | null>(null);
  let pages = $state<Page[]>([]);
  let folders = $state<Folder[]>([]);
  // LIF-105: project labels powering the filter dropdown and chip color
  // lookups in the tree. Empty until the project resolves.
  let labels = $state<Label[]>([]);
  let loading = $state(true);
  let error = $state("");

  // LIF-305: the selected Pages slice persists independently per project.
  const PAGE_SUB_TAB_IDS = ["browse", "recent", "drafts", "archived"] as const;
  type PageSubTab = (typeof PAGE_SUB_TAB_IDS)[number];
  let activeSubTab = $state<PageSubTab>("browse");

  let expandedFolders = $state<Set<number>>(new Set());

  // Drag and drop. Pages only: the API has no endpoint for re-parenting
  // a folder yet, and the old folder-drag path moved the folder
  // optimistically, looked successful, then silently reverted on
  // reload. Until persistence exists, folders aren't draggable.
  let draggedId = $state<{ type: "page"; id: number } | null>(null);
  let dropTarget = $state<number | "root" | null>(null);

  // LIF-280: touch-friendly alternative to native HTML drag-and-drop for
  // moving a page between folders. The picker starts at the page's current
  // location, which Select marks with a check.
  let movePage = $state<Page | null>(null);
  let moveFolderId = $state<number | null>(null);
  let movingPageIds = $state<Set<number>>(new Set());
  let pageMutationGeneration = $state(0);
  let movingPage = $derived(movePage !== null && movingPageIds.has(movePage.id));

  // Inline create. `status` (pages only) lets the "New page as …" menu
  // presets seed the lifecycle status the inline row will commit with.
  let createTarget = $state<{
    type: "page" | "folder";
    parentId: number | null;
    status?: string;
  } | null>(null);
  let createName = $state("");

  // Split "New" button caret menu (folder / paste / quick note / status).
  let newMenuOpen = $state(false);

  // LIF-185: folder focus. When set (via the right sidebar), the main tree
  // shows only that folder's subtree. null = show everything.
  let focusedFolderId = $state<number | null>(null);

  // LIF-105: label filter. Empty string = no filter (mirrors the issue
  // list's filterLabel convention).
  let filterLabel = $state("");

  // LIF-112: lifecycle status. Picker icon + label per value, plus the
  // status filter. The filter's default ("__active") hides archived
  // pages; "" means show All (including archived).
  const PAGE_STATUSES = [
    { value: "draft", label: "Draft", icon: PenLine },
    { value: "active", label: "Active", icon: CircleDot },
    { value: "complete", label: "Complete", icon: CheckCircle2 },
    { value: "archived", label: "Archived", icon: Archive },
  ] as const;

  function statusMeta(value: string) {
    return PAGE_STATUSES.find((s) => s.value === value) ?? PAGE_STATUSES[0];
  }

  // Status → leading-icon color so the page icon actually carries meaning
  // (it used to be an identical FileText on every row).
  function statusColor(status: string): string {
    switch (status) {
      case "active":
        return "var(--accent)";
      case "complete":
        return "var(--success)";
      case "archived":
        return "var(--text-faint)";
      default: // draft
        return "var(--text-muted)";
    }
  }

  // Filter dropdown options. The leading entry is the default "hide
  // archived" view (sentinel "__active"), then explicit "All", then one
  // per concrete status.
  const HIDE_ARCHIVED = "__active";
  const statusFilterOptions = [
    { value: HIDE_ARCHIVED, label: "Active" },
    { value: "", label: "All" },
    ...PAGE_STATUSES.map((s) => ({ value: s.value, label: s.label })),
  ];

  // Default to hiding archived pages.
  let filterStatus = $state(HIDE_ARCHIVED);

  // Browse filters are applied locally so the same complete collection can
  // also power the zero-config LIF-305 flat tabs.
  let visiblePages = $derived(
    pages.filter((p) => {
      if (filterLabel && !p.labels.includes(filterLabel)) return false;
      if (filterStatus === HIDE_ARCHIVED) return p.status !== "archived";
      return !filterStatus || p.status === filterStatus;
    }),
  );

  let recentPages = $derived(
    pages
      .filter((p) => p.status !== "archived")
      .sort((a, b) => b.updated_at.localeCompare(a.updated_at))
      .slice(0, 20),
  );

  let draftPages = $derived(
    pages
      .filter((p) => p.status === "draft")
      .sort((a, b) => b.updated_at.localeCompare(a.updated_at)),
  );

  let archivedPages = $derived(
    pages
      .filter((p) => p.status === "archived")
      .sort((a, b) => b.updated_at.localeCompare(a.updated_at)),
  );

  let pageSubTabs = $derived([
    { id: "browse", label: "Browse" },
    { id: "recent", label: "Recent" },
    { id: "drafts", label: "Drafts", count: draftPages.length },
    { id: "archived", label: "Archived", count: archivedPages.length },
  ]);

  // Search keeps the Browse filter semantics it had when filters were
  // server-backed: "Active" and "All" search every loaded page, while a
  // concrete status or label narrows its candidate set.
  let searchablePages = $derived(
    pages.filter(
      (p) =>
        (!filterLabel || p.labels.includes(filterLabel)) &&
        (filterStatus === HIDE_ARCHIVED || filterStatus === "" || p.status === filterStatus),
    ),
  );

  // LIF-117/118: client-side search. A collapsed icon in the toolbar
  // expands to an input on click, and fuzzy-scores the already-loaded
  // pages across title, identifier, and content. When active, the tree
  // is replaced by a flat ranked result list (folders become irrelevant
  // in a search context).
  let searchQuery = $state("");
  let searchExpanded = $state(false);

  // LIF-231: the overview/folder-navigator sidebar is an off-canvas panel
  // below lg, toggled from the topbar; statically docked at lg+.
  let overviewOpen = $state(false);
  let searchInputEl = $state<HTMLInputElement | null>(null);

  // A search hit carries enough info to render the row: which page, the
  // composite score (for sorting), and an optional snippet pulled from
  // the content body when content was the *reason* the page matched.
  interface SearchHit {
    page: Page;
    score: number;
    snippet: string | null;
  }

  let filteredPages = $derived.by<SearchHit[]>(() => {
    const q = searchQuery.trim();
    if (!q) return [];

    const hits: SearchHit[] = [];
    for (const page of searchablePages) {
      const titleHit = fuzzyMatch(q, page.title);
      const idHit = fuzzyMatch(q, page.identifier);
      // Cap content scan: scorer is O(haystack) and pages can be long.
      const body = page.content.slice(0, CONTENT_SCAN_MAX);
      const contentHit = fuzzyMatch(q, body);

      const titleScore = titleHit?.score ?? 0;
      const idScore = (idHit?.score ?? 0) * IDENTIFIER_WEIGHT;
      const contentScore = (contentHit?.score ?? 0) * CONTENT_WEIGHT;

      const best = Math.max(titleScore, idScore, contentScore);
      if (best < SCORE_THRESHOLD) continue;

      // Snippet only when content was the winning signal — otherwise the
      // title/identifier already explains the match.
      const snippet =
        contentHit && contentScore === best && best > 0
          ? buildSnippet(body, contentHit.matchStart, contentHit.matchEnd)
          : null;

      hits.push({ page, score: best, snippet });
    }

    hits.sort((a, b) => b.score - a.score);
    return hits.slice(0, RESULT_CAP);
  });

  function openSearch() {
    searchExpanded = true;
    requestAnimationFrame(() => searchInputEl?.focus());
  }

  function maybeCollapseSearch() {
    if (!searchQuery) searchExpanded = false;
  }

  let labelOptions = $derived([
    { value: "", label: "Label" },
    ...labels.map((l) => ({ value: l.name, label: l.name, color: l.color })),
  ]);

  let moveFolderOptions = $derived([
    { value: null, label: "No folder / root" },
    ...folders.map((folder) => ({ value: folder.id, label: folder.name })),
  ]);

  $effect(() => {
    const id = projectIdentifier;
    loadData(id);
  });

  async function loadData(ident: string) {
    loading = true;
    error = "";
    const projRes = await listProjects();
    if (!projRes.ok) { error = projRes.error; loading = false; return; }
    const found = projRes.data.find((p: Project) => p.identifier === ident);
    if (!found) { error = `Project ${ident} not found`; loading = false; return; }
    project = found;
    loadProjectRole(found.id); // LIF-234
    activeSubTab = (loadSubTab("pages", String(found.id), PAGE_SUB_TAB_IDS) ?? "browse") as PageSubTab;

    const [pRes, fRes, lRes] = await Promise.all([
      listPages(found.id),
      listFolders(found.id),
      listLabels(found.id),
    ]);
    if (pRes.ok) pages = pRes.data;
    if (fRes.ok) {
      const previousFolderIds = new Set(folders.map((folder) => folder.id));
      const nextFolderIds = new Set(fRes.data.map((folder) => folder.id));
      folders = fRes.data;
      expandedFolders = new Set(
        [...expandedFolders].filter((id) => nextFolderIds.has(id)),
      );
      for (const folder of fRes.data) {
        if (!previousFolderIds.has(folder.id)) expandedFolders.add(folder.id);
      }
    }
    if (lRes.ok) labels = lRes.data;
    loading = false;
  }

  async function reloadPages() {
    if (!project) return;
    const mutationGeneration = pageMutationGeneration;
    const [pagesRes, foldersRes, labelsRes] = await Promise.all([
      listPages(project.id),
      listFolders(project.id),
      listLabels(project.id),
    ]);
    if (
      pagesRes.ok &&
      mutationGeneration === pageMutationGeneration &&
      movingPageIds.size === 0
    ) pages = pagesRes.data;
    if (foldersRes.ok) {
      const previousFolderIds = new Set(folders.map((folder) => folder.id));
      const nextFolderIds = new Set(foldersRes.data.map((folder) => folder.id));
      folders = foldersRes.data;
      expandedFolders = new Set(
        [...expandedFolders].filter((id) => nextFolderIds.has(id)),
      );
      for (const folder of foldersRes.data) {
        if (!previousFolderIds.has(folder.id)) expandedFolders.add(folder.id);
      }
    }
    if (labelsRes.ok) labels = labelsRes.data;
  }

  function selectSubTab(id: string) {
    if (!PAGE_SUB_TAB_IDS.includes(id as PageSubTab)) return;
    activeSubTab = id as PageSubTab;
    if (project) saveSubTab("pages", String(project.id), activeSubTab);
  }

  // ── LIF-129: auto-refresh ────────────────────────────
  // Revalidate on tab focus and relevant realtime events so the page tree
  // picks up pages created/edited/deleted out-of-band. Vetoed while dragging a
  // page/folder, while an inline create input is open, or while the
  // search box is focused — refreshing under any of those would yank the
  // user's interaction. The refresh keeps the full project tree in sync.
  function autoRefreshBusy(): boolean {
    return (
      loading ||
      draggedId !== null ||
      movePage !== null ||
      movingPageIds.size > 0 ||
      createTarget !== null ||
      (searchExpanded && document.activeElement === searchInputEl)
    );
  }

  $effect(() =>
    startAutoRefresh({
      refresh: reloadPages,
      isBusy: autoRefreshBusy,
      shouldRefresh: (event) =>
        event.type === "resync.required" ||
        (typeof event.project_id === "number" && event.project_id === project?.id),
    }),
  );

  // Tree helpers
  function childFolders(parentId: number | null): Folder[] {
    return folders.filter((f) => f.parent_id === parentId);
  }

  function pagesInFolder(folderId: number | null): Page[] {
    // Newest first so a freshly created page lands at the top of its level
    // instead of the bottom (pages share a default sort_order, so the old
    // id-ASC tiebreak buried new docs under everything else).
    return visiblePages
      .filter((p) => (p.folder_id ?? null) === folderId)
      .sort((a, b) => b.created_at.localeCompare(a.created_at));
  }

  function toggleFolder(id: number) {
    const next = new Set(expandedFolders);
    if (next.has(id)) next.delete(id);
    else next.add(id);
    expandedFolders = next;
  }

  function contentPreview(content: string): string {
    const lines = content.split("\n").filter((l) => l.trim() && !l.startsWith("#"));
    return (lines[0] ?? "").replace(/[*_`\[\]]/g, "").slice(0, 140);
  }

  // LIF-183: pinned pages, surfaced in a section above the folder tree.
  // Pins cut across folders, so this is a flat list (most-recent first).
  let pinnedPages = $derived(
    visiblePages
      .filter((p) => p.pinned)
      .sort((a, b) => b.updated_at.localeCompare(a.updated_at)),
  );

  function folderName(folderId: number | null): string | null {
    if (folderId == null) return null;
    return folders.find((f) => f.id === folderId)?.name ?? null;
  }

  // Depth of a folder in the hierarchy (for sidebar indentation).
  function folderDepth(folder: Folder): number {
    let d = 0;
    let cur: Folder | undefined = folder;
    while (cur && cur.parent_id != null) {
      cur = folders.find((f) => f.id === cur!.parent_id);
      d++;
      if (d > 20) break; // cycle guard
    }
    return d;
  }

  // Folders sorted for the sidebar: a stable depth-first ordering so nested
  // folders sit under their parent, each shown with a small indent.
  let sidebarFolders = $derived.by(() => {
    const out: Folder[] = [];
    const walk = (parentId: number | null) => {
      const kids = folders
        .filter((f) => f.parent_id === parentId)
        .sort((a, b) => a.name.localeCompare(b.name));
      for (const f of kids) {
        out.push(f);
        walk(f.id);
      }
    };
    walk(null);
    return out;
  });

  // Direct (non-recursive) page count per folder, for the sidebar.
  function folderPageCount(folderId: number | null): number {
    return visiblePages.filter((p) => (p.folder_id ?? null) === folderId).length;
  }

  // Per-status tallies across visible pages, for the sidebar summary.
  let statusTally = $derived.by(() => {
    const counts: Record<string, number> = { draft: 0, active: 0, complete: 0, archived: 0 };
    for (const p of visiblePages) counts[p.status] = (counts[p.status] ?? 0) + 1;
    return counts;
  });

  function focusFolder(id: number | null) {
    focusedFolderId = id;
    searchQuery = "";
    searchExpanded = false;
    // On mobile, dismiss the off-canvas overview so the filtered list shows.
    overviewOpen = false;
  }

  // Optimistic pin toggle. Flip locally so the row reacts instantly, then
  // persist; on failure, revert.
  async function togglePin(page: Page, e: Event) {
    e.stopPropagation();
    const next = !page.pinned;
    pages = pages.map((p) => (p.id === page.id ? { ...p, pinned: next } : p));
    const res = await updatePage(page.id, { pinned: next });
    if (!res.ok) {
      pages = pages.map((p) => (p.id === page.id ? { ...p, pinned: !next } : p));
    }
  }

  // ── Drag and drop ────────────────────────────────────

  function onDragStartPage(e: DragEvent, pageId: number) {
    draggedId = { type: "page", id: pageId };
    if (e.dataTransfer) {
      e.dataTransfer.effectAllowed = "move";
      e.dataTransfer.setData("text/plain", `page:${pageId}`);
    }
  }

  function onDragEnd() {
    draggedId = null;
    dropTarget = null;
  }

  function onDragOver(e: DragEvent, target: number | "root") {
    if (!draggedId) return;
    e.preventDefault();
    if (e.dataTransfer) e.dataTransfer.dropEffect = "move";
    // Only update if actually changing target — prevents child element
    // dragleave/dragenter flicker from clearing the highlight
    if (dropTarget !== target) dropTarget = target;
  }

  async function movePageToFolder(page: Page, targetFolderId: number | null): Promise<boolean> {
    if (movingPageIds.has(page.id)) return false;

    const previousFolderId = page.folder_id ?? null;
    if (previousFolderId === targetFolderId) return true;

    pageMutationGeneration += 1;
    movingPageIds = new Set([...movingPageIds, page.id]);
    pages = pages.map((p) =>
      p.id === page.id ? { ...p, folder_id: targetFolderId } : p
    );
    try {
      const res = await updatePage(page.id, { folder_id: targetFolderId });
      if (!res.ok) {
        pages = pages.map((p) =>
          p.id === page.id ? { ...p, folder_id: previousFolderId } : p
        );
        toast(`Couldn't move ${page.identifier}: ${res.error}`, { kind: "error" });
        return false;
      }

      pages = pages.map((p) => (p.id === page.id ? res.data : p));
      if (targetFolderId && !expandedFolders.has(targetFolderId)) {
        expandedFolders = new Set([...expandedFolders, targetFolderId]);
      }
      return true;
    } finally {
      const next = new Set(movingPageIds);
      next.delete(page.id);
      movingPageIds = next;
    }
  }

  async function onDrop(e: DragEvent, targetFolderId: number | null) {
    e.preventDefault();
    if (!draggedId) return;
    const dragged = draggedId;
    draggedId = null;
    dropTarget = null;

    const page = pages.find((p) => p.id === dragged.id);
    if (!page) return;
    await movePageToFolder(page, targetFolderId);
  }

  function openMovePicker(page: Page, e: Event) {
    e.stopPropagation();
    movePage = page;
    moveFolderId = page.folder_id ?? null;
  }

  async function chooseMoveFolder(folderId: number | null) {
    if (!movePage) return;
    if (await movePageToFolder(movePage, folderId)) movePage = null;
  }

  // ── Create ───────────────────────────────────────────

  function startCreate(
    type: "page" | "folder",
    parentId: number | null = null,
    status?: string,
  ) {
    if (!canEdit) return; // LIF-234: page/folder creation is maintainer-gated
    newMenuOpen = false;
    createTarget = { type, parentId, status };
    createName = "";
  }

  async function commitCreate() {
    if (!project || !createTarget || !createName.trim()) return;
    const { type, parentId, status } = createTarget;
    createTarget = null;

    if (type === "page") {
      const res = await createPage({
        project_id: project.id,
        title: createName.trim(),
        folder_id: parentId ?? undefined,
        ...(status ? { status } : {}),
      });
      if (res.ok) {
        navigate(`/${projectIdentifier}/pages/${res.data.id}`);
      }
    } else {
      const res = await createFolder({
        project_id: project.id,
        name: createName.trim(),
        parent_id: parentId ?? undefined,
      });
      if (res.ok) {
        folders = [...folders, res.data];
        expandedFolders = new Set([...expandedFolders, res.data.id]);
      }
    }
    createName = "";
  }

  // ── Page-specific quick actions (split-button menu) ──────

  // Enhancement: create a page straight from the clipboard. The first
  // non-empty line becomes the title (stripped of markdown heading/marks),
  // the rest becomes the body — turning a copied block into a doc in one
  // click. Falls back to the normal inline create when the clipboard is
  // empty or unreadable (e.g. permission denied) so the action is never a
  // dead end.
  async function pasteAsNewPage() {
    newMenuOpen = false;
    if (!project) return;
    let text = "";
    try {
      text = await navigator.clipboard.readText();
    } catch {
      // Clipboard blocked/unavailable — degrade to a blank create row.
    }
    if (!text.trim()) {
      startCreate("page");
      return;
    }
    const lines = text.split("\n");
    const firstIdx = lines.findIndex((l) => l.trim());
    const rawTitle = lines[firstIdx] ?? "Pasted page";
    const title =
      rawTitle
        .replace(/^#+\s*/, "")
        .replace(/[*_`>\[\]]/g, "")
        .trim()
        .slice(0, 120) || "Pasted page";
    const body = lines.slice(firstIdx + 1).join("\n").trim();
    const res = await createPage({
      project_id: project.id,
      title,
      ...(focusedFolderId != null ? { folder_id: focusedFolderId } : {}),
      ...(body ? { content: body } : {}),
    });
    if (res.ok) navigate(`/${projectIdentifier}/pages/${res.data.id}`);
  }

  // Enhancement: one-click dated scratch note. Creates an empty draft
  // titled with today's date and drops you straight into the editor — for
  // capturing a thought without naming ceremony.
  async function quickNote() {
    newMenuOpen = false;
    if (!project) return;
    const now = new Date();
    const stamp = now.toLocaleDateString("en-US", {
      month: "short",
      day: "numeric",
      year: "numeric",
    });
    const res = await createPage({
      project_id: project.id,
      title: `Note · ${stamp}`,
      status: "draft",
      ...(focusedFolderId != null ? { folder_id: focusedFolderId } : {}),
    });
    if (res.ok) navigate(`/${projectIdentifier}/pages/${res.data.id}`);
  }

  async function handleDeleteFolder(id: number, e: Event) {
    e.stopPropagation();
    await deleteFolder(id);
    folders = folders.filter((f) => f.id !== id && f.parent_id !== id);
    pages = pages.map((p) => p.folder_id === id ? { ...p, folder_id: null } : p);
  }

  // Count all items (pages + subfolders) in a folder recursively
  function folderItemCount(folderId: number): number {
    const directPages = pagesInFolder(folderId).length;
    const subs = childFolders(folderId);
    return directPages + subs.length + subs.reduce((n, f) => n + folderItemCount(f.id), 0);
  }
</script>

<svelte:window
  onclick={() => { newMenuOpen = false; }}
  onkeydown={(e) => {
    if (e.key === "Escape" && newMenuOpen) newMenuOpen = false;
    else if (e.key === "Escape" && overviewOpen) overviewOpen = false;
  }}
/>

{#snippet topbarContent()}
  <div class="relative flex flex-wrap items-center gap-2 sm:gap-3 px-3 sm:px-6 py-2 w-full">
    <!-- Breadcrumb: Project > Pages. Project segment collapses below sm. -->
    <div class="order-1 flex items-center gap-1.5 shrink-0">
      <button
        class="hidden sm:inline text-body-sm font-mono font-medium text-[var(--text-muted)]
               hover:text-[var(--text)] transition-colors"
        onclick={() => navigate(`/${projectIdentifier}/overview`)}
      >
        {projectIdentifier}
      </button>
      <ChevronRight size={12} class="hidden sm:block text-[var(--text-faint)]" />
      <span class="text-body-sm font-medium text-[var(--text)]">
        Pages
      </span>
      {#if !loading}
        <span
          class="ml-1 text-micro text-[var(--text-faint)] font-medium
                 tabular-nums"
        >
          {visiblePages.length}
        </span>
      {/if}
    </div>

    <!-- LIF-305: the sub-tab strip takes the topbar's bottom row (order-4 +
         basis-full, after every order-3 control) so the breadcrumb and the
         Browse toolbar keep sharing the top row exactly as before. -->
    <div class="order-4 basis-full">
      <SubTabs tabs={pageSubTabs} active={activeSubTab} onselect={selectSubTab} />
    </div>

    {#if activeSubTab === "browse"}
    <!-- LIF-105: label filter. Only shown when the project has labels
         defined — keeps the toolbar clean for label-less projects. -->
    {#if labels.length > 0}
      <div class="order-3 flex items-center gap-1.5">
        <Select
          options={labelOptions}
          bind:value={filterLabel}
          placeholder="Label"
          size="sm"
          class="w-auto"
        >
          {#snippet renderSelected(opt)}
            <span class="flex items-center gap-1.5 text-body-sm">
              {#if opt.value && opt.color}
                <span class="size-2.5 rounded-full shrink-0" style="background: {opt.color}"></span>
                <span class="text-[var(--text)]">{opt.label}</span>
              {:else}
                <span class="text-[var(--text-muted)]">{opt.label}</span>
              {/if}
            </span>
          {/snippet}
          {#snippet renderOption(opt, isSelected)}
            <span class="flex items-center gap-2 text-body-sm {isSelected ? 'font-medium' : ''}">
              {#if opt.value && opt.color}
                <span class="size-2.5 rounded-full shrink-0" style="background: {opt.color}"></span>
                <span class="{isSelected ? 'text-[var(--accent)]' : 'text-[var(--text)]'}">{opt.label}</span>
              {:else}
                <span class="text-[var(--text-muted)]">{opt.label}</span>
              {/if}
            </span>
          {/snippet}
        </Select>
        {#if filterLabel}
          <button
            class="flex items-center gap-1 text-caption text-[var(--text-muted)]
                   hover:text-[var(--text)] px-1.5 py-1 rounded-md
                   hover:bg-[var(--bg-subtle)] transition-colors"
            onclick={() => { filterLabel = ""; }}
            title="Clear label filter"
          >
            <X size={12} />
          </button>
        {/if}
      </div>
    {/if}

    <!-- LIF-112: status filter. Defaults to "Active" (hides archived);
         pick "All" to reveal archived pages, or a concrete status to
         narrow. -->
    <div class="order-3 flex items-center gap-1.5">
      <Select
        options={statusFilterOptions}
        bind:value={filterStatus}
        size="sm"
        class="w-auto"
      >
        {#snippet renderSelected(opt)}
          <span class="flex items-center gap-1.5 text-body-sm text-[var(--text)]">
            {#if opt.value && opt.value !== HIDE_ARCHIVED}
              {@const meta = statusMeta(String(opt.value))}
              <meta.icon size={13} class="shrink-0 text-[var(--text-muted)]" />
            {/if}
            {opt.label}
          </span>
        {/snippet}
        {#snippet renderOption(opt, isSelected)}
          <span class="flex items-center gap-2 text-body-sm {isSelected ? 'font-medium' : ''}">
            {#if opt.value && opt.value !== HIDE_ARCHIVED}
              {@const meta = statusMeta(String(opt.value))}
              <meta.icon size={13} class="shrink-0 {isSelected ? 'text-[var(--accent)]' : 'text-[var(--text-muted)]'}" />
            {/if}
            <span class="{isSelected ? 'text-[var(--accent)]' : 'text-[var(--text)]'}">{opt.label}</span>
          </span>
        {/snippet}
      </Select>
    </div>

    <!-- Right zone: search + create actions -->
    <div class="order-3 ml-auto flex items-center gap-1.5 shrink-0">
      <!-- LIF-231: overview/folder-navigator toggle (below lg, where the
           sidebar is off-canvas). -->
      {#if !loading && !error && (pages.length > 0 || folders.length > 0)}
        <Tooltip content="Overview & folders" placement="bottom">
          <button
            class="lg:hidden size-9 sm:size-7 flex items-center justify-center rounded-md
                   text-[var(--text-muted)] hover:text-[var(--text)]
                   hover:bg-[var(--bg-subtle)] transition-colors"
            aria-label="Show overview and folders"
            aria-expanded={overviewOpen}
            onclick={(e) => { e.stopPropagation(); overviewOpen = true; }}
          >
            <PanelRight size={15} />
          </button>
        </Tooltip>
      {/if}

      <!-- LIF-117: search. Collapsed-to-icon, expands inline on click.
           Below sm the expanded field overlays the whole topbar row instead
           of joining it — a fixed-width input in an already-full shrink-0
           row pushes the row's min width past the viewport (LIF-271). -->
      {#if searchExpanded}
        <div
          class="max-sm:absolute max-sm:inset-0 max-sm:z-20 max-sm:bg-[var(--chrome)]
                 max-sm:flex max-sm:items-center max-sm:px-3 max-sm:py-1.5"
        >
          <div class="relative w-full sm:w-auto">
            <div class="absolute left-2 top-1/2 -translate-y-1/2 pointer-events-none text-[var(--text-faint)]">
              <Search size={12} />
            </div>
            <!-- svelte-ignore a11y_autofocus -->
            <input
              type="text"
              placeholder="Search pages..."
              enterkeyhint="search"
              bind:this={searchInputEl}
              bind:value={searchQuery}
              onblur={maybeCollapseSearch}
              onkeydown={(e) => {
                if (e.key === "Escape") {
                  e.preventDefault();
                  searchQuery = "";
                  searchExpanded = false;
                  (e.currentTarget as HTMLInputElement).blur();
                }
              }}
              class="w-full sm:w-[200px] pl-7 pr-2 py-1 text-body-sm rounded-md
                     border border-[var(--border)] bg-[var(--surface)]
                     text-[var(--text)] placeholder:text-[var(--text-faint)]
                     focus:border-[var(--accent)]
                     focus:shadow-[0_0_0_3px_var(--accent-subtle)]
                     outline-none transition-colors"
            />
          </div>
        </div>
      {:else}
        <Tooltip content="Search" placement="bottom">
          <button
            class="size-7 flex items-center justify-center rounded-md
                   text-[var(--text-muted)] hover:text-[var(--text)]
                   hover:bg-[var(--bg-subtle)] transition-colors"
            onclick={(e) => { e.stopPropagation(); openSearch(); }}
          >
            <Search size={14} />
          </button>
        </Tooltip>
      {/if}

      {#if canEdit}
      <!-- Separator -->
      <div class="w-px h-4 bg-[var(--border)] mx-1.5"></div>

      <!-- Primary action: New page. Split button — the main segment starts
           an inline page create; the caret reveals folder creation and
           page-specific shortcuts (paste-as-page, quick note, status
           presets). Folds the old separate Folder button into the menu.
           Hidden for viewers (LIF-234) — pages are maintainer-gated. -->
      <div class="relative">
        <button
          class="group flex items-center gap-1.5 h-7 pl-2.5 pr-2
                 text-body-sm font-medium text-[var(--btn-success-text)]
                 bg-[var(--btn-success)] hover:bg-[var(--btn-success-hover)]
                 rounded-md shadow-sm transition-colors focus:outline-none
                 focus-visible:ring-2 focus-visible:ring-[var(--btn-success)]
                 focus-visible:ring-offset-1 focus-visible:ring-offset-[var(--chrome)]
                 motion-safe:active:scale-[0.97]"
          aria-haspopup="menu"
          aria-expanded={newMenuOpen}
          onclick={(e) => { e.stopPropagation(); newMenuOpen = !newMenuOpen; }}
        >
          <Plus size={14} class="shrink-0" />
          <span class="hidden sm:inline">New</span>
          <ChevronDown
            size={14}
            class="motion-safe:transition-transform {newMenuOpen ? 'rotate-180' : ''}"
          />
        </button>

        {#if newMenuOpen}
          <!-- svelte-ignore a11y_no_static_element_interactions a11y_click_events_have_key_events -->
          <div
            role="menu"
            tabindex="-1"
            class="absolute right-0 top-full mt-1.5 z-30 w-[228px]
                   bg-[var(--surface)] border border-[var(--border)]
                   rounded-lg shadow-lg py-1.5"
            onclick={(e) => e.stopPropagation()}
          >
            <button
              role="menuitem"
              class="w-full flex items-center gap-2.5 px-3 py-1.5 text-left
                     text-body-sm text-[var(--text)]
                     hover:bg-[var(--bg-subtle)] transition-colors"
              onclick={() => startCreate("page", focusedFolderId)}
            >
              <FileText size={14} class="text-[var(--text-muted)]" />
              <span class="flex-1">New page</span>
            </button>
            <button
              role="menuitem"
              class="w-full flex items-center gap-2.5 px-3 py-1.5 text-left
                     text-body-sm text-[var(--text)]
                     hover:bg-[var(--bg-subtle)] transition-colors"
              onclick={() => startCreate("folder", focusedFolderId)}
            >
              <FolderPlus size={14} class="text-[var(--text-muted)]" />
              <span class="flex-1">New folder</span>
            </button>

            <div class="my-1 h-px bg-[var(--border)]"></div>

            <button
              role="menuitem"
              class="w-full flex items-center gap-2.5 px-3 py-1.5 text-left
                     text-body-sm text-[var(--text)]
                     hover:bg-[var(--bg-subtle)] transition-colors"
              onclick={pasteAsNewPage}
            >
              <ClipboardPaste size={14} class="text-[var(--success)]" />
              <span class="flex-1">Paste as new page</span>
            </button>
            <button
              role="menuitem"
              class="w-full flex items-center gap-2.5 px-3 py-1.5 text-left
                     text-body-sm text-[var(--text)]
                     hover:bg-[var(--bg-subtle)] transition-colors"
              onclick={quickNote}
            >
              <NotebookPen size={14} class="text-[var(--success)]" />
              <span class="flex-1">Quick note</span>
            </button>

            <div class="my-1 h-px bg-[var(--border)]"></div>
            <div class="px-3 pb-1 pt-0.5 text-micro uppercase tracking-widest
                        font-semibold text-[var(--text-faint)]">
              New page as
            </div>
            {#each [["draft", "Draft", PenLine], ["active", "Active", CircleDot], ["complete", "Complete", CheckCircle2]] as [value, label, Icon]}
              {@const IconComp = Icon as typeof PenLine}
              <button
                role="menuitem"
                class="w-full flex items-center gap-2.5 px-3 py-1.5 text-left
                       text-body-sm text-[var(--text)]
                       hover:bg-[var(--bg-subtle)] transition-colors"
                onclick={() => startCreate("page", focusedFolderId, value as string)}
              >
                <IconComp size={14} class="text-[var(--text-muted)]" />
                <span class="flex-1">{label}</span>
              </button>
            {/each}
          </div>
        {/if}
      </div>
      {/if}
    </div>
    {/if}
  </div>
{/snippet}

<div class="h-full flex flex-col">
  <div class="flex-1 flex min-h-0">
  <!-- Content — entire scroll area is the root drop zone -->
  <!-- svelte-ignore a11y_no_static_element_interactions -->
  <div
    class="flex-1 overflow-y-auto min-w-0"
    ondragover={(e) => {
      if (!draggedId) return;
      e.preventDefault();
      if (e.dataTransfer) e.dataTransfer.dropEffect = "move";
      dropTarget = "root";
    }}
    ondrop={(e) => {
      // If dropTarget is still root or null, drop to root
      if (dropTarget === "root" || dropTarget === null) {
        onDrop(e, null);
      }
    }}
  >
    {#if loading}
      <!-- LIF-281: mimics the tree shape (folder rows, each with indented
           page rows beneath). Mirrors the loaded treeLevel markup: the
           px-6 py-4 wrapper; folder rows (gap-1.5 py-1.5 px-1.5 -mx-1.5,
           14px chevron + 18px folder icon + title); the ml-[15px] pl-3
           border-l children rail; and page rows (items-start gap-2 py-1.5
           px-1.5 -mx-1.5, 17px status icon at mt-0.5 + title + preview
           line). Corrects the earlier LIF-246 skeleton, which dropped the
           row padding, mis-sized the status icon, aligned items center
           instead of start, and omitted the preview line. -->
      <div class="px-6 py-4 flex flex-col gap-0.5">
        {#each [3, 2] as pageCount, folder (folder)}
          <div class="flex items-center gap-1.5 py-1.5 px-1.5 -mx-1.5">
            <Skeleton variant="circle" class="size-3.5 shrink-0" />
            <Skeleton variant="circle" class="size-[18px] rounded-md shrink-0" />
            <Skeleton variant="bar" class="h-3.5 w-32" />
          </div>
          <div class="ml-[15px] pl-3 border-l border-[var(--border)] flex flex-col gap-0.5">
            {#each Array(pageCount) as _, page (page)}
              <div class="flex items-start gap-2 py-1.5 px-1.5 -mx-1.5">
                <Skeleton variant="circle" class="size-[17px] mt-0.5 shrink-0" />
                <div class="flex-1 min-w-0 flex flex-col gap-1">
                  <Skeleton variant="bar" class="h-3.5 w-full max-w-[240px]" />
                  <Skeleton variant="bar" class="h-2.5 w-4/5 max-w-[200px]" />
                </div>
              </div>
            {/each}
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
        <button
          class="text-body-sm text-[var(--text-muted)] border border-[var(--border)] px-3 py-1.5 rounded-md hover:bg-[var(--bg-subtle)] transition-colors"
          onclick={() => navigate(`/${projectIdentifier}/overview`)}
        >
          Project overview
        </button>
      </ErrorState>
    {:else if activeSubTab === "recent"}
      <div class="px-6 py-4">
        {@render flatPageList(
          recentPages,
          "A blank page",
          "Pages are your project's docs: specs, notes, decisions. Start the first one and give the ideas a home.",
        )}
      </div>
    {:else if activeSubTab === "drafts"}
      <div class="px-6 py-4">
        {@render flatPageList(draftPages, "No drafts")}
      </div>
    {:else if activeSubTab === "archived"}
      <div class="px-6 py-4">
        {@render flatPageList(archivedPages, "No archived pages")}
      </div>
    {:else if pages.length === 0 && folders.length === 0 && !createTarget}
      <div class="flex flex-col items-center py-16 gap-4 px-6 max-w-[460px] mx-auto text-center">
        <Mascot src="/LizzyReading.png" nativeW={487} nativeH={714} />
        <div class="flex flex-col items-center gap-1.5">
          <p class="text-heading font-medium text-[var(--text)]">A blank page</p>
          <p class="text-body-sm text-[var(--text-muted)] leading-relaxed">
            Pages are your project's docs: specs, notes, decisions. Start the
            first one and give the ideas a home.
          </p>
        </div>
        {#if canEdit}
          <button
            class="flex items-center gap-1.5 mt-1 text-body-sm font-medium
                   text-[var(--btn-success-text)] bg-[var(--btn-success)]
                   px-3 py-1.5 rounded-md hover:bg-[var(--btn-success-hover)]
                   transition-colors"
            onclick={() => startCreate("page")}
          >
            <Plus size={15} />
            Create a page
          </button>
        {/if}
      </div>
    {:else if searchQuery.trim()}
      <!-- LIF-117/118: flat ranked search results. Folders are omitted
           in this mode because the user is looking for a specific page
           by name or content; the tree structure would just be visual
           noise. Results are scored across title/identifier/content and
           sorted by relevance. -->
      <div class="px-6 py-4">
        {#if filteredPages.length === 0}
          <div class="flex flex-col items-center py-16 gap-3">
            <Mascot src="/LizzyReading.png" nativeW={487} nativeH={714} scale={0.16} />
            <p class="text-body-lg text-[var(--text-muted)]">
              No pages match "{searchQuery}"
            </p>
            <button
              class="text-body-sm text-[var(--accent)] hover:underline"
              onclick={() => { searchQuery = ""; searchExpanded = false; }}
            >
              Clear search
            </button>
          </div>
        {:else}
          {#if filteredPages.length === RESULT_CAP}
            <div class="text-micro text-[var(--text-faint)] uppercase tracking-widest font-semibold mb-2 px-1.5">
              Top {RESULT_CAP} matches — narrow the query for fewer results
            </div>
          {/if}
          {#each filteredPages as hit (hit.page.id)}
            {@const hMeta = statusMeta(hit.page.status)}
            <button
              class="w-full flex flex-col items-stretch gap-0.5 py-1.5 px-1.5 -mx-1.5 rounded-md
                     text-left group transition-colors hover:bg-[var(--bg-subtle)]"
              onclick={() => navigate(`/${projectIdentifier}/pages/${hit.page.id}`)}
            >
              <div class="flex items-center gap-2">
                <span class="shrink-0" style="color: {statusColor(hit.page.status)}" title={hMeta.label}>
                  <hMeta.icon size={17} />
                </span>
                <span class="text-body-lg text-[var(--text)] truncate flex-1">
                  {hit.page.title}
                </span>
                <span class="text-caption font-mono text-[var(--text-faint)] shrink-0">
                  {hit.page.identifier}
                </span>
                {#if hit.page.status !== "active"}
                  <span
                    class="flex items-center gap-1 shrink-0 text-micro font-medium
                           px-1.5 py-0.5 rounded-full border border-[var(--border)]
                           text-[var(--text-muted)]"
                    title={hMeta.label}
                  >
                    <hMeta.icon size={11} class="shrink-0" />
                    {hMeta.label}
                  </span>
                {/if}
                {#if hit.page.labels.length > 0}
                  <div class="flex items-center gap-1 shrink-0">
                    {#each hit.page.labels.slice(0, 2) as lbl}
                      {@const labelObj = labels.find((l) => l.name === lbl)}
                      <span
                        class="text-micro font-medium px-1.5 py-0.5 rounded-full
                               border border-[var(--border)]"
                        style={labelObj ? `color: ${labelObj.color}; border-color: ${labelObj.color}40;` : ""}
                      >
                        {lbl}
                      </span>
                    {/each}
                    {#if hit.page.labels.length > 2}
                      <span class="text-micro text-[var(--text-faint)]">
                        +{hit.page.labels.length - 2}
                      </span>
                    {/if}
                  </div>
                {/if}
              </div>
              {#if hit.snippet}
                <!-- LIF-118: content snippet — only shown when content
                     was the winning signal, so title alone wouldn't
                     explain why this page surfaced. -->
                <div class="text-body-sm text-[var(--text-muted)] truncate pl-[26px]">
                  {hit.snippet}
                </div>
              {/if}
            </button>
          {/each}
        {/if}
      </div>
    {:else}
      <div class="px-6 py-4">
        <!-- LIF-185: focus banner when a folder is focused from the sidebar. -->
        {#if focusedFolderId !== null}
          <div class="flex items-center gap-1.5 mb-4 text-body-sm">
            <button
              class="text-[var(--text-muted)] hover:text-[var(--text)] transition-colors"
              onclick={() => focusFolder(null)}
            >
              All pages
            </button>
            <ChevronRight size={13} class="text-[var(--text-faint)]" />
            <span class="flex items-center gap-1.5 font-medium text-[var(--text)]">
              <FolderOpen size={15} class="text-[var(--accent)]" />
              {folderName(focusedFolderId)}
            </span>
          </div>
        {/if}

        <!-- LIF-183: Pinned section. Surfaced above the tree so key docs are
             one glance away regardless of which folder they live in. Hidden
             while focused on a single folder to keep that view clean. -->
        {#if pinnedPages.length > 0 && focusedFolderId === null}
          <section class="mb-6">
            <div class="flex items-center gap-1.5 mb-2 px-1">
              <Pin size={12} class="text-[var(--text-faint)]" />
              <h2 class="text-micro font-semibold uppercase tracking-widest text-[var(--text-muted)]">
                Pinned
              </h2>
              <span class="text-micro text-[var(--text-faint)] tabular-nums">
                {pinnedPages.length}
              </span>
            </div>
            <div class="grid grid-cols-1 sm:grid-cols-2 gap-2">
              {#each pinnedPages as page (page.id)}
                {@const pMeta = statusMeta(page.status)}
                {@const fName = folderName(page.folder_id)}
                {@const prev = contentPreview(page.content)}
                <button
                  class="group text-left rounded-xl bg-[var(--surface)] p-3
                         shadow-[0_1px_2px_rgba(0,0,0,0.06)]
                         hover:shadow-[0_6px_16px_rgba(0,0,0,0.10)]
                         transition motion-safe:hover:-translate-y-0.5"
                  onclick={() => navigate(`/${projectIdentifier}/pages/${page.id}`)}
                >
                  <div class="flex items-start gap-2.5">
                    <span
                      class="shrink-0 mt-0.5"
                      style="color: {statusColor(page.status)}"
                      title={pMeta.label}
                    >
                      <pMeta.icon size={16} />
                    </span>
                    <div class="flex-1 min-w-0">
                      <div class="flex items-center gap-2">
                        <span class="text-body font-medium text-[var(--text)] truncate flex-1">
                          {page.title}
                        </span>
                        {#if canEdit}
                          <span
                            class="shrink-0 text-[var(--text-faint)] opacity-0 group-hover:opacity-100
                                   hover:text-[var(--accent)] transition"
                            role="button"
                            tabindex="0"
                            title="Unpin"
                            onclick={(e) => togglePin(page, e)}
                            onkeydown={(e) => { if (e.key === "Enter") togglePin(page, e); }}
                          >
                            <PinOff size={13} />
                          </span>
                        {/if}
                      </div>
                      {#if prev}
                        <p class="text-caption text-[var(--text-faint)] line-clamp-1 mt-0.5">{prev}</p>
                      {/if}
                      <div class="flex items-center gap-2 mt-1.5 text-micro text-[var(--text-faint)]">
                        <span class="font-mono shrink-0">{page.identifier}</span><span>·</span>
                        {#if fName}<span class="truncate">{fName}</span><span>·</span>{/if}
                        <TimeAgo class="tabular-nums shrink-0" date={page.updated_at} />
                        {#if page.status !== "active"}
                          <span class="flex items-center gap-1 shrink-0">
                            <pMeta.icon size={10} /> {pMeta.label}
                          </span>
                        {/if}
                      </div>
                    </div>
                  </div>
                </button>
              {/each}
            </div>
          </section>
        {/if}
        {@render treeLevel(focusedFolderId)}
      </div>
    {/if}
  </div>

  <!-- LIF-185: right sidebar — project page overview + folder navigator.
       Fills the empty right space and lets you focus a single folder.
       LIF-231: off-canvas drawer below lg, docked at lg+. -->
   {#if activeSubTab === "browse" && !loading && !error && (pages.length > 0 || folders.length > 0)}
    {#if overviewOpen}
      <button
        class="lg:hidden fixed inset-0 z-40 bg-black/40 backdrop-blur-[1px]"
        aria-label="Close overview"
        onclick={() => (overviewOpen = false)}
      ></button>
    {/if}
    <aside
      class="w-[280px] sm:w-[300px] lg:w-[260px] shrink-0 overflow-y-auto
             border-l border-[var(--border)] bg-[var(--bg-subtle)] px-4 py-5
             fixed inset-y-0 right-0 z-50 transition-transform duration-200 ease-out
             {overviewOpen ? 'translate-x-0 shadow-2xl' : 'translate-x-full'}
             lg:static lg:z-auto lg:w-[260px] lg:translate-x-0 lg:shadow-none lg:transition-none"
    >
      <!-- In-drawer close (below lg). -->
      <div class="lg:hidden flex justify-end -mt-2 -mr-1 mb-1">
        <button
          class="size-9 grid place-items-center rounded-md text-[var(--text-muted)]
                 hover:text-[var(--text)] hover:bg-[var(--bg-subtle)] transition-colors"
          aria-label="Close overview"
          onclick={() => (overviewOpen = false)}
        >
          <X size={18} />
        </button>
      </div>
      <!-- Summary -->
      <div class="grid grid-cols-2 gap-3 mb-5">
        <div>
          <p class="text-title font-display tracking-tight tabular-nums text-[var(--text)] leading-none">
            {visiblePages.length}
          </p>
          <p class="text-micro font-semibold uppercase tracking-widest text-[var(--text-faint)] mt-1">
            {visiblePages.length === 1 ? "Page" : "Pages"}
          </p>
        </div>
        <div>
          <p class="text-title font-display tracking-tight tabular-nums text-[var(--text)] leading-none">
            {folders.length}
          </p>
          <p class="text-micro font-semibold uppercase tracking-widest text-[var(--text-faint)] mt-1">
            {folders.length === 1 ? "Folder" : "Folders"}
          </p>
        </div>
      </div>

      <!-- Status breakdown -->
      <div class="flex flex-col gap-1 mb-5">
        {#each PAGE_STATUSES as s}
          {#if statusTally[s.value] > 0}
            <div class="flex items-center gap-2 text-caption">
              <s.icon size={13} style="color: {statusColor(s.value)}" class="shrink-0" />
              <span class="flex-1 text-[var(--text-muted)]">{s.label}</span>
              <span class="tabular-nums text-[var(--text-faint)]">{statusTally[s.value]}</span>
            </div>
          {/if}
        {/each}
      </div>

      <div class="h-px bg-[var(--border)] -mx-4 mb-4"></div>

      <!-- Folder navigator -->
      <p class="text-micro font-semibold uppercase tracking-widest text-[var(--text-faint)] mb-2 px-1">
        Folders
      </p>
      <div class="flex flex-col gap-0.5">
        <button
          class="flex items-center gap-2 px-2 py-1.5 rounded-md text-left text-body-sm
                 transition-colors
                 {focusedFolderId === null
            ? 'bg-[var(--surface)] text-[var(--text)] shadow-[0_1px_2px_rgba(0,0,0,0.06)] font-medium'
            : 'text-[var(--text-muted)] hover:bg-[var(--surface)] hover:text-[var(--text)]'}"
          onclick={() => focusFolder(null)}
        >
          <FileText size={14} class="shrink-0 text-[var(--text-faint)]" />
          <span class="flex-1 truncate">All pages</span>
          <span class="tabular-nums text-micro text-[var(--text-faint)]">{visiblePages.length}</span>
        </button>
        {#each sidebarFolders as folder (folder.id)}
          {@const depth = folderDepth(folder)}
          <button
            class="flex items-center gap-2 px-2 py-1.5 rounded-md text-left text-body-sm
                   transition-colors
                   {focusedFolderId === folder.id
              ? 'bg-[var(--surface)] text-[var(--text)] shadow-[0_1px_2px_rgba(0,0,0,0.06)] font-medium'
              : 'text-[var(--text-muted)] hover:bg-[var(--surface)] hover:text-[var(--text)]'}"
            style="padding-left: {depth * 14 + 8}px;"
            onclick={() => focusFolder(folder.id)}
          >
            {#if focusedFolderId === folder.id}
              <FolderOpen size={14} class="shrink-0 text-[var(--accent)]" />
            {:else}
              <FolderClosed size={14} class="shrink-0 text-[var(--text-faint)]" />
            {/if}
            <span class="flex-1 truncate">{folder.name}</span>
            <span class="tabular-nums text-micro text-[var(--text-faint)]">
              {folderPageCount(folder.id)}
            </span>
          </button>
        {/each}
      </div>
    </aside>
  {/if}
 </div>
</div>

<!-- LIF-280: selecting a folder from this picker provides the same page
     re-parenting path as drag-and-drop, without relying on touch-incompatible
     native HTML5 DnD. -->
{#if movePage}
  <!-- svelte-ignore a11y_no_static_element_interactions a11y_click_events_have_key_events -->
  <div
    class="fixed inset-0 z-[100] bg-black/25 flex items-start justify-center pt-[14dvh] px-2"
    onclick={(e) => {
      if (e.target === e.currentTarget && !movingPage) movePage = null;
    }}
    onkeydown={(e) => {
      if (e.key === "Escape" && !movingPage) movePage = null;
    }}
  >
    <!-- svelte-ignore a11y_no_static_element_interactions a11y_click_events_have_key_events -->
    <div
      class="w-full max-w-[calc(100vw-1rem)] sm:max-w-[420px] bg-[var(--surface)]
             border border-[var(--border)] rounded-xl shadow-[0_16px_48px_rgba(0,0,0,0.28)]"
      role="dialog"
      aria-modal="true"
      aria-label="Move page to folder"
      tabindex="-1"
    >
      <div class="flex items-center gap-3 px-4 py-3 border-b border-[var(--border)]">
        <FolderOpen size={16} class="shrink-0 text-[var(--accent)]" />
        <div class="flex-1 min-w-0">
          <h2 class="text-body-lg font-semibold text-[var(--text)]">Move to folder</h2>
          <p class="text-caption text-[var(--text-faint)] truncate">{movePage.title}</p>
        </div>
        <button
          class="size-7 grid place-items-center rounded-md text-[var(--text-muted)]
                 hover:text-[var(--text)] hover:bg-[var(--bg-subtle)] transition-colors"
          aria-label="Close move folder picker"
          disabled={movingPage}
          onclick={() => (movePage = null)}
        >
          <X size={15} />
        </button>
      </div>
      <div class="px-4 py-4">
        <p class="mb-2 text-micro uppercase tracking-widest font-semibold text-[var(--text-faint)]">
          Folder
        </p>
        <Select
          options={moveFolderOptions}
          bind:value={moveFolderId}
          onchange={(option) => void chooseMoveFolder(option.value as number | null)}
        >
          {#snippet renderOption(option, isSelected)}
            <span class="flex items-center gap-2 text-body-sm {isSelected ? 'font-medium text-[var(--accent)]' : 'text-[var(--text)]'}">
              {#if option.value === null}
                <FileText size={14} class="shrink-0 text-[var(--text-muted)]" />
              {:else}
                <FolderClosed size={14} class="shrink-0 {isSelected ? 'text-[var(--accent)]' : 'text-[var(--text-muted)]'}" />
              {/if}
              <span class="truncate">{option.label}</span>
            </span>
          {/snippet}
        </Select>
      </div>
    </div>
  </div>
{/if}

{#snippet flatPageList(pageList: Page[], emptyTitle: string, emptyDescription: string = "")}
  {#if pageList.length === 0}
    <div class="flex flex-col items-center py-16 gap-3 px-6 max-w-[460px] mx-auto text-center">
      <Mascot src="/LizzyReading.png" nativeW={487} nativeH={714} scale={0.16} />
      <div class="flex flex-col items-center gap-1.5">
        <p class="text-heading font-medium text-[var(--text)]">{emptyTitle}</p>
        {#if emptyDescription}
          <p class="text-body-sm text-[var(--text-muted)] leading-relaxed">{emptyDescription}</p>
        {/if}
      </div>
    </div>
  {:else}
    <div class="flex flex-col">
      {#each pageList as page (page.id)}
        {@const pageMeta = statusMeta(page.status)}
        {@const containingFolder = folderName(page.folder_id)}
        <button
          class="w-full flex items-start gap-2 py-2 px-1.5 -mx-1.5 border-b border-[var(--border)]
                 last:border-b-0 rounded-md text-left transition-colors hover:bg-[var(--bg-subtle)]"
          onclick={() => navigate(`/${projectIdentifier}/pages/${page.id}`)}
        >
          <span
            class="shrink-0 mt-0.5"
            style="color: {statusColor(page.status)}"
            title={pageMeta.label}
          >
            <pageMeta.icon size={17} />
          </span>
          <div class="flex-1 min-w-0">
            <div class="text-body-lg text-[var(--text)] truncate">{page.title}</div>
            <div class="flex items-center gap-2 mt-0.5 text-caption text-[var(--text-faint)]">
              <span class="font-mono shrink-0">{page.identifier}</span>
              {#if containingFolder}
                <span>·</span>
                <span class="truncate">{containingFolder}</span>
              {/if}
              <span>·</span>
              <TimeAgo class="tabular-nums shrink-0" date={page.updated_at} />
            </div>
          </div>
        </button>
      {/each}
    </div>
  {/if}
{/snippet}

<!--
  Recursive tree renderer.
  Each level shows: folders at this depth, then pages at this depth, then inline create if active.
-->
{#snippet treeLevel(parentId: number | null)}
  {@const subFolders = childFolders(parentId)}
  {@const subPages = pagesInFolder(parentId)}

  <!-- Inline create row — pinned to the TOP of the level so a new page is
       visible immediately, even in a project with hundreds of pages. -->
  {#if createTarget && createTarget.parentId === parentId}
    <div class="flex items-center gap-2 py-1">
      {#if createTarget.type === "folder"}
        <FolderOpen size={18} class="text-[var(--btn-success)] shrink-0" />
      {:else}
        <FileText size={18} class="text-[var(--btn-success)] shrink-0" />
      {/if}
      <!-- svelte-ignore a11y_autofocus -->
      <input
        type="text"
        bind:value={createName}
        class="flex-1 px-2 py-1 text-body-lg rounded
               border border-[var(--btn-success)] bg-transparent
               text-[var(--text)] outline-none"
        placeholder={createTarget.type === "folder" ? "Folder name" : "Page title"}
        autofocus
        onkeydown={(e) => {
          if (e.key === "Enter") commitCreate();
          if (e.key === "Escape") { createTarget = null; }
        }}
        onblur={() => { if (!createName.trim()) createTarget = null; }}
      />
    </div>
  {/if}

  {#each subFolders as folder (folder.id)}
    {@const isExpanded = expandedFolders.has(folder.id)}
    {@const isDraggedOver = dropTarget === folder.id}

    <!-- svelte-ignore a11y_no_static_element_interactions -->
    <div
      ondragover={(e) => { e.stopPropagation(); onDragOver(e, folder.id); }}
      ondrop={(e) => { e.stopPropagation(); onDrop(e, folder.id); }}
    >
      <div
        class="flex items-center gap-1.5 py-1.5 px-1.5 -mx-1.5 rounded-md
               cursor-pointer group transition-colors
               {isDraggedOver
          ? 'bg-[var(--accent-subtle)] outline outline-1 outline-dashed outline-[var(--accent)]'
          : 'hover:bg-[var(--bg-subtle)]'}"
        role="button"
        tabindex="0"
        onclick={() => toggleFolder(folder.id)}
        onkeydown={(e) => { if (e.key === "Enter") toggleFolder(folder.id); }}
      >
        <ChevronRight
          size={14}
          class="shrink-0 text-[var(--text-faint)] transition-transform
                 {isExpanded ? 'rotate-90' : ''}"
        />
        {#if isExpanded}
          <FolderOpen size={18} class="shrink-0 text-[var(--accent)]" />
        {:else}
          <FolderClosed size={18} class="shrink-0 text-[var(--text-muted)]" />
        {/if}
        <span class="text-body-lg font-medium text-[var(--text)] flex-1 truncate">
          {folder.name}
        </span>
        <!-- Hover actions (LIF-234: hidden for viewers). LIF-275: always
             visible on touch — hover-reveal never fires there. -->
        {#if canEdit}
        <div class="flex items-center gap-0.5 opacity-0 group-hover:opacity-100 pointer-coarse:opacity-100 transition-opacity">
          <button
            class="size-6 flex items-center justify-center rounded
                   text-[var(--text-faint)] hover:text-[var(--accent)]
                   hover:bg-[var(--accent-subtle)]"
            title="New page"
            onclick={(e) => { e.stopPropagation(); startCreate("page", folder.id); }}
          >
            <FileText size={13} />
          </button>
          <button
            class="size-6 flex items-center justify-center rounded
                   text-[var(--text-faint)] hover:text-[var(--accent)]
                   hover:bg-[var(--accent-subtle)]"
            title="New subfolder"
            onclick={(e) => { e.stopPropagation(); startCreate("folder", folder.id); }}
          >
            <Plus size={13} />
          </button>
          <button
            class="size-6 flex items-center justify-center rounded
                   text-[var(--text-faint)] hover:text-[var(--error)]
                   hover:bg-[var(--error-bg)]"
            title="Delete folder"
            onclick={(e) => handleDeleteFolder(folder.id, e)}
          >
            <Trash2 size={13} />
          </button>
        </div>
        {/if}
      </div>

      <!-- Children (recursive), inset under a vertical guide line so the
           nesting reads clearly. -->
      {#if isExpanded}
        <div class="ml-[15px] pl-3 border-l border-[var(--border)]">
          {@render treeLevel(folder.id)}
        </div>
      {/if}
    </div>
  {/each}

  <!-- Pages at this level -->
  {#each subPages as page (page.id)}
    {@const isDragging = draggedId?.type === "page" && draggedId.id === page.id}
    {@const sMeta = statusMeta(page.status)}
    {@const preview = contentPreview(page.content)}
    <button
      class="w-full flex items-start gap-2 py-1.5 px-1.5 -mx-1.5 rounded-md
             text-left group transition-colors
             {isDragging ? 'opacity-40' : 'hover:bg-[var(--bg-subtle)]'}"
      onclick={() => navigate(`/${projectIdentifier}/pages/${page.id}`)}
      draggable={canEdit}
      ondragstart={(e) => { if (canEdit) onDragStartPage(e, page.id); }}
      ondragend={onDragEnd}
    >
      <span
        class="shrink-0 mt-0.5"
        style="color: {statusColor(page.status)}"
        title={sMeta.label}
      >
        <sMeta.icon size={17} />
      </span>
      <div class="flex-1 min-w-0">
        <div class="flex items-center gap-2">
          <span class="text-body-lg text-[var(--text)] truncate flex-1">
            {page.title}
          </span>

          <!-- Identifier (LIF-DOC-N) — the handle used for cross-references
               and MCP tools; mirrors the search-result rows so it's visible
               without searching. Hidden below sm where row width is scarce. -->
          <span class="hidden sm:inline text-caption font-mono text-[var(--text-faint)] shrink-0">
            {page.identifier}
          </span>

          <!-- LIF-105: label chips. Up to 2 then a "+N" overflow. -->
          {#if page.labels.length > 0}
            <div class="flex items-center gap-1 shrink-0">
              {#each page.labels.slice(0, 2) as lbl}
                {@const labelObj = labels.find((l) => l.name === lbl)}
                <span
                  class="text-micro font-medium px-1.5 py-0.5 rounded-full
                         border border-[var(--border)]"
                  style={labelObj ? `color: ${labelObj.color}; border-color: ${labelObj.color}40;` : ""}
                >
                  {lbl}
                </span>
              {/each}
              {#if page.labels.length > 2}
                <span class="text-micro text-[var(--text-faint)]">
                  +{page.labels.length - 2}
                </span>
              {/if}
            </div>
          {/if}

          <TimeAgo
            class="text-body-sm text-[var(--text-faint)] shrink-0 tabular-nums
                   group-hover:text-[var(--text-muted)] transition-colors"
            date={page.updated_at}
          />

          <!-- LIF-280: hidden until hover on desktop, but always reachable
               on touch devices where native drag-and-drop does not work. -->
          {#if canEdit}
            <span
              class="shrink-0 text-[var(--text-faint)] opacity-0 group-hover:opacity-100
                     pointer-coarse:opacity-100 hover:text-[var(--accent)] transition"
              role="button"
              tabindex="0"
              title="Move to folder…"
              onclick={(e) => openMovePicker(page, e)}
              onkeydown={(e) => { if (e.key === "Enter") openMovePicker(page, e); }}
            >
              <FolderOpen size={13} />
            </span>
          {/if}

          <!-- LIF-183: pin toggle. Always visible (accent) when pinned;
               otherwise revealed on hover. LIF-234: for a viewer, a pinned
               page shows a static pin; the toggle affordance is hidden. -->
          {#if canEdit}
            <span
              class="shrink-0 transition
                     {page.pinned
                ? 'text-[var(--accent)]'
                : 'text-[var(--text-faint)] opacity-0 group-hover:opacity-100 pointer-coarse:opacity-100 hover:text-[var(--accent)]'}"
              role="button"
              tabindex="0"
              title={page.pinned ? "Unpin" : "Pin to top"}
              onclick={(e) => togglePin(page, e)}
              onkeydown={(e) => { if (e.key === "Enter") togglePin(page, e); }}
            >
              {#if page.pinned}
                <Pin size={13} class="fill-current" />
              {:else}
                <Pin size={13} />
              {/if}
            </span>
          {:else if page.pinned}
            <span class="shrink-0 text-[var(--accent)]" title="Pinned">
              <Pin size={13} class="fill-current" />
            </span>
          {/if}
        </div>

        <!-- Content preview — turns the tree into something scannable. -->
        {#if preview}
          <p class="text-caption text-[var(--text-faint)] truncate mt-0.5 pr-6">
            {preview}
          </p>
        {/if}
      </div>
    </button>
  {/each}

{/snippet}

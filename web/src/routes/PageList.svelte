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
    Trash2,
    X,
    Search,
  } from "lucide-svelte";
  import Select from "../lib/Select.svelte";
  import Tooltip from "../lib/Tooltip.svelte";
  import { fuzzyMatch, buildSnippet } from "../lib/fuzzy";

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

  let expandedFolders = $state<Set<number>>(new Set());

  // Drag and drop
  let draggedId = $state<{ type: "page" | "folder"; id: number } | null>(null);
  let dropTarget = $state<number | "root" | null>(null);

  // Inline create
  let createTarget = $state<{ type: "page" | "folder"; parentId: number | null } | null>(null);
  let createName = $state("");

  // LIF-105: server-side label filter. Empty string = no filter (mirrors
  // the issue list's filterLabel convention).
  let filterLabel = $state("");

  // LIF-117/118: client-side search. A collapsed icon in the toolbar
  // expands to an input on click, and fuzzy-scores the already-loaded
  // pages across title, identifier, and content. When active, the tree
  // is replaced by a flat ranked result list (folders become irrelevant
  // in a search context).
  let searchQuery = $state("");
  let searchExpanded = $state(false);
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
    for (const page of pages) {
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

  $effect(() => {
    const id = projectIdentifier;
    loadData(id);
  });

  // Refetch pages when the label filter changes (matches the issue-list
  // pattern of pushing every filter through the server, so it composes
  // cleanly with later filters like folder).
  $effect(() => {
    filterLabel;
    if (project) reloadPages();
  });

  async function loadData(ident: string) {
    loading = true;
    error = "";
    const projRes = await listProjects();
    if (!projRes.ok) { error = projRes.error; loading = false; return; }
    const found = projRes.data.find((p: Project) => p.identifier === ident);
    if (!found) { error = `Project ${ident} not found`; loading = false; return; }
    project = found;

    const [pRes, fRes, lRes] = await Promise.all([
      listPages(found.id, undefined, filterLabel || undefined),
      listFolders(found.id),
      listLabels(found.id),
    ]);
    if (pRes.ok) pages = pRes.data;
    if (fRes.ok) {
      folders = fRes.data;
      expandedFolders = new Set(fRes.data.map((f: Folder) => f.id));
    }
    if (lRes.ok) labels = lRes.data;
    loading = false;
  }

  async function reloadPages() {
    if (!project) return;
    const res = await listPages(project.id, undefined, filterLabel || undefined);
    if (res.ok) pages = res.data;
  }

  // Tree helpers
  function childFolders(parentId: number | null): Folder[] {
    return folders.filter((f) => f.parent_id === parentId);
  }

  function pagesInFolder(folderId: number | null): Page[] {
    return pages.filter((p) => (p.folder_id ?? null) === folderId);
  }

  function toggleFolder(id: number) {
    const next = new Set(expandedFolders);
    if (next.has(id)) next.delete(id);
    else next.add(id);
    expandedFolders = next;
  }

  function contentPreview(content: string): string {
    const lines = content.split("\n").filter((l) => l.trim() && !l.startsWith("#"));
    return (lines[0] ?? "").replace(/[*_`\[\]]/g, "").slice(0, 100) || "Empty page";
  }

  function formatRelative(iso: string): string {
    const d = new Date(iso + "Z");
    const now = new Date();
    const diffDays = Math.floor((now.getTime() - d.getTime()) / 86400000);
    if (diffDays < 1) return "today";
    if (diffDays === 1) return "yesterday";
    if (diffDays < 7) return `${diffDays}d ago`;
    return d.toLocaleDateString("en-US", { month: "short", day: "numeric" });
  }

  // ── Drag and drop ────────────────────────────────────

  function onDragStartPage(e: DragEvent, pageId: number) {
    draggedId = { type: "page", id: pageId };
    if (e.dataTransfer) {
      e.dataTransfer.effectAllowed = "move";
      e.dataTransfer.setData("text/plain", `page:${pageId}`);
    }
  }

  function onDragStartFolder(e: DragEvent, folderId: number) {
    draggedId = { type: "folder", id: folderId };
    if (e.dataTransfer) {
      e.dataTransfer.effectAllowed = "move";
      e.dataTransfer.setData("text/plain", `folder:${folderId}`);
    }
  }

  function onDragEnd() {
    draggedId = null;
    dropTarget = null;
  }

  function onDragOver(e: DragEvent, target: number | "root") {
    if (!draggedId) return;
    // Don't allow dropping a folder into itself
    if (draggedId.type === "folder" && draggedId.id === target) return;
    e.preventDefault();
    if (e.dataTransfer) e.dataTransfer.dropEffect = "move";
    // Only update if actually changing target — prevents child element
    // dragleave/dragenter flicker from clearing the highlight
    if (dropTarget !== target) dropTarget = target;
  }

  async function onDrop(e: DragEvent, targetFolderId: number | null) {
    e.preventDefault();
    if (!draggedId) return;
    const dragged = draggedId;
    draggedId = null;
    dropTarget = null;

    if (dragged.type === "page") {
      const page = pages.find((p) => p.id === dragged.id);
      if (!page || (page.folder_id ?? null) === targetFolderId) return;
      pages = pages.map((p) =>
        p.id === dragged.id ? { ...p, folder_id: targetFolderId } : p
      );
      await updatePage(page.id, { folder_id: targetFolderId } as Record<string, unknown>);
    } else {
      const folder = folders.find((f) => f.id === dragged.id);
      if (!folder || folder.parent_id === targetFolderId) return;
      // Prevent circular nesting
      if (targetFolderId && isDescendant(targetFolderId, dragged.id)) return;
      folders = folders.map((f) =>
        f.id === dragged.id ? { ...f, parent_id: targetFolderId } : f
      );
      // TODO: persist folder parent change when API supports it
    }

    if (targetFolderId && !expandedFolders.has(targetFolderId)) {
      expandedFolders = new Set([...expandedFolders, targetFolderId]);
    }
  }

  function isDescendant(folderId: number, ancestorId: number): boolean {
    const folder = folders.find((f) => f.id === folderId);
    if (!folder) return false;
    if (folder.parent_id === ancestorId) return true;
    if (folder.parent_id) return isDescendant(folder.parent_id, ancestorId);
    return false;
  }

  // ── Create ───────────────────────────────────────────

  function startCreate(type: "page" | "folder", parentId: number | null = null) {
    createTarget = { type, parentId };
    createName = "";
  }

  async function commitCreate() {
    if (!project || !createTarget || !createName.trim()) return;
    const { type, parentId } = createTarget;
    createTarget = null;

    if (type === "page") {
      const res = await createPage({
        project_id: project.id,
        title: createName.trim(),
        folder_id: parentId ?? undefined,
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

<div class="h-full flex flex-col">
  <!-- Toolbar -->
  <div
    class="shrink-0 flex items-center gap-3 px-6 py-2.5
           border-b border-[var(--border)] bg-[var(--surface)]"
  >
    <!-- Breadcrumb: Project > Pages -->
    <div class="flex items-center gap-1.5 shrink-0">
      <button
        class="text-[0.8125rem] font-mono font-medium text-[var(--text-muted)]
               hover:text-[var(--text)] transition-colors"
        onclick={() => navigate(`/${projectIdentifier}/settings`)}
      >
        {projectIdentifier}
      </button>
      <ChevronRight size={12} class="text-[var(--text-faint)]" />
      <span class="text-[0.8125rem] font-medium text-[var(--text)]">
        Pages
      </span>
      {#if !loading}
        <span
          class="text-[0.6875rem] text-[var(--text-faint)] bg-[var(--bg-subtle)]
                 px-1.5 py-0.5 rounded-full font-medium tabular-nums"
        >
          {pages.length}
        </span>
      {/if}
    </div>

    <!-- LIF-105: label filter. Only shown when the project has labels
         defined — keeps the toolbar clean for label-less projects. The
         Select component is the same one IssueList's filter uses so the
         visual vocabulary stays consistent. -->
    {#if labels.length > 0}
      <div class="ml-3 flex items-center gap-1.5">
        <Select
          options={labelOptions}
          bind:value={filterLabel}
          placeholder="Label"
          size="sm"
          class="w-auto"
        >
          {#snippet renderSelected(opt)}
            <span class="flex items-center gap-1.5 text-[0.8125rem]">
              {#if opt.value && opt.color}
                <span class="size-2.5 rounded-full shrink-0" style="background: {opt.color}"></span>
                <span class="text-[var(--text)]">{opt.label}</span>
              {:else}
                <span class="text-[var(--text-muted)]">{opt.label}</span>
              {/if}
            </span>
          {/snippet}
          {#snippet renderOption(opt, isSelected)}
            <span class="flex items-center gap-2 text-[0.8125rem] {isSelected ? 'font-medium' : ''}">
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
            class="flex items-center gap-1 text-[0.75rem] text-[var(--text-muted)]
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

    <!-- Spacer -->
    <div class="flex-1"></div>

    <!-- LIF-117: search. Collapsed-to-icon, expands inline on click. -->
    {#if searchExpanded}
      <div class="relative shrink-0">
        <div class="absolute left-2 top-1/2 -translate-y-1/2 pointer-events-none text-[var(--text-faint)]">
          <Search size={12} />
        </div>
        <!-- svelte-ignore a11y_autofocus -->
        <input
          type="text"
          placeholder="Search pages..."
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
          class="w-[200px] pl-7 pr-2 py-1 text-[0.8125rem] rounded-md
                 border border-[var(--border)] bg-[var(--surface)]
                 text-[var(--text)] placeholder:text-[var(--text-faint)]
                 focus:border-[var(--accent)]
                 focus:shadow-[0_0_0_3px_var(--accent-subtle)]
                 outline-none transition-colors"
        />
      </div>
    {:else}
      <Tooltip content="Search" placement="bottom">
        <button
          class="size-7 flex items-center justify-center rounded-md
                 text-[var(--text-muted)] hover:text-[var(--text)]
                 hover:bg-[var(--bg-subtle)] transition-colors shrink-0"
          onclick={(e) => { e.stopPropagation(); openSearch(); }}
        >
          <Search size={14} />
        </button>
      </Tooltip>
    {/if}

    <!-- Actions -->
    <div class="flex items-center gap-1.5 shrink-0">
      <button
        class="flex items-center gap-1 text-[0.8125rem]
               text-[var(--text-muted)] px-2.5 py-1 rounded-md
               hover:bg-[var(--bg-subtle)] hover:text-[var(--text)]
               transition-colors"
        onclick={() => startCreate("folder")}
      >
        <FolderOpen size={14} />
        Folder
      </button>
      <button
        class="flex items-center gap-1 text-[0.8125rem] font-medium
               text-[var(--accent-text)] bg-[var(--accent)] px-2.5 py-1
               rounded-md hover:bg-[var(--accent-hover)] transition-colors"
        onclick={() => startCreate("page")}
      >
        <Plus size={14} />
        Page
      </button>
    </div>
  </div>

  <!-- Content — entire scroll area is the root drop zone -->
  <!-- svelte-ignore a11y_no_static_element_interactions -->
  <div
    class="flex-1 overflow-y-auto"
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
      <div class="flex items-center justify-center py-20">
        <div
          class="size-6 rounded-full border-2 border-[var(--border)]
                 border-t-[var(--accent)] animate-spin"
        ></div>
      </div>
    {:else if error}
      <div class="flex items-center justify-center py-20">
        <p class="text-[var(--error)] text-[0.875rem]">{error}</p>
      </div>
    {:else if pages.length === 0 && folders.length === 0 && !createTarget}
      <div class="flex flex-col items-center py-16 gap-3">
        <FileText size={32} class="text-[var(--text-faint)]" />
        <p class="text-[0.9375rem] text-[var(--text-muted)]">No pages yet</p>
        <button
          class="text-[0.8125rem] text-[var(--accent)] hover:underline"
          onclick={() => startCreate("page")}
        >
          Create the first page
        </button>
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
            <Search size={28} class="text-[var(--text-faint)]" />
            <p class="text-[0.9375rem] text-[var(--text-muted)]">
              No pages match "{searchQuery}"
            </p>
            <button
              class="text-[0.8125rem] text-[var(--accent)] hover:underline"
              onclick={() => { searchQuery = ""; searchExpanded = false; }}
            >
              Clear search
            </button>
          </div>
        {:else}
          {#if filteredPages.length === RESULT_CAP}
            <div class="text-[0.6875rem] text-[var(--text-faint)] uppercase tracking-widest font-semibold mb-2 px-1.5">
              Top {RESULT_CAP} matches — narrow the query for fewer results
            </div>
          {/if}
          {#each filteredPages as hit (hit.page.id)}
            <button
              class="w-full flex flex-col items-stretch gap-0.5 py-1.5 px-1.5 -mx-1.5 rounded-md
                     text-left group transition-colors hover:bg-[var(--bg-subtle)]"
              onclick={() => navigate(`/${projectIdentifier}/pages/${hit.page.id}`)}
            >
              <div class="flex items-center gap-2">
                <FileText size={18} class="shrink-0 text-[var(--text-faint)] group-hover:text-[var(--accent)]" />
                <span class="text-[0.9375rem] text-[var(--text)] truncate flex-1">
                  {hit.page.title}
                </span>
                <span class="text-[0.75rem] font-mono text-[var(--text-faint)] shrink-0">
                  {hit.page.identifier}
                </span>
                {#if hit.page.labels.length > 0}
                  <div class="flex items-center gap-1 shrink-0">
                    {#each hit.page.labels.slice(0, 2) as lbl}
                      {@const labelObj = labels.find((l) => l.name === lbl)}
                      <span
                        class="text-[0.6875rem] font-medium px-1.5 py-0.5 rounded-full
                               border border-[var(--border)]"
                        style={labelObj ? `color: ${labelObj.color}; border-color: ${labelObj.color}40;` : ""}
                      >
                        {lbl}
                      </span>
                    {/each}
                    {#if hit.page.labels.length > 2}
                      <span class="text-[0.6875rem] text-[var(--text-faint)]">
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
                <div class="text-[0.8125rem] text-[var(--text-muted)] truncate pl-[26px]">
                  {hit.snippet}
                </div>
              {/if}
            </button>
          {/each}
        {/if}
      </div>
    {:else}
      <div class="px-6 py-4">
        {@render treeLevel(null, 0)}
      </div>
    {/if}
  </div>
</div>

<!--
  Recursive tree renderer.
  Each level shows: folders at this depth, then pages at this depth, then inline create if active.
-->
{#snippet treeLevel(parentId: number | null, depth: number)}
  {@const subFolders = childFolders(parentId)}
  {@const subPages = pagesInFolder(parentId)}

  {#each subFolders as folder (folder.id)}
    {@const isExpanded = expandedFolders.has(folder.id)}
    {@const isDraggedOver = dropTarget === folder.id}
    {@const isDragging = draggedId?.type === "folder" && draggedId.id === folder.id}

    <!-- svelte-ignore a11y_no_static_element_interactions -->
    <div
      class="{isDragging ? 'opacity-40' : ''}"
      ondragover={(e) => { e.stopPropagation(); onDragOver(e, folder.id); }}
      ondrop={(e) => { e.stopPropagation(); onDrop(e, folder.id); }}
    >
      <div
        class="flex items-center gap-1.5 py-1.5 px-1.5 -mx-1.5 rounded-md
               cursor-pointer group transition-colors
               {isDraggedOver
          ? 'bg-[var(--accent-subtle)] outline outline-1 outline-dashed outline-[var(--accent)]'
          : 'hover:bg-[var(--bg-subtle)]'}"
        style="padding-left: {depth * 20 + 6}px;"
        role="button"
        tabindex="0"
        draggable="true"
        onclick={() => toggleFolder(folder.id)}
        onkeydown={(e) => { if (e.key === "Enter") toggleFolder(folder.id); }}
        ondragstart={(e) => onDragStartFolder(e, folder.id)}
        ondragend={onDragEnd}
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
        <span class="text-[0.9375rem] font-medium text-[var(--text)] flex-1 truncate">
          {folder.name}
        </span>
        <!-- Hover actions -->
        <div class="flex items-center gap-0.5 opacity-0 group-hover:opacity-100 transition-opacity">
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
      </div>

      <!-- Children (recursive) -->
      {#if isExpanded}
        {@render treeLevel(folder.id, depth + 1)}
      {/if}
    </div>
  {/each}

  <!-- Pages at this level -->
  {#each subPages as page (page.id)}
    {@const isDragging = draggedId?.type === "page" && draggedId.id === page.id}
    <button
      class="w-full flex items-center gap-2 py-1.5 px-1.5 -mx-1.5 rounded-md
             text-left group transition-colors
             {isDragging ? 'opacity-40' : 'hover:bg-[var(--bg-subtle)]'}"
      style="padding-left: {depth * 20 + 6}px;"
      onclick={() => navigate(`/${projectIdentifier}/pages/${page.id}`)}
      draggable="true"
      ondragstart={(e) => onDragStartPage(e, page.id)}
      ondragend={onDragEnd}
    >
      <FileText size={18} class="shrink-0 text-[var(--text-faint)] group-hover:text-[var(--accent)]" />
      <span class="text-[0.9375rem] text-[var(--text)] truncate flex-1">
        {page.title}
      </span>

      <!-- LIF-105: label chips. Up to 2 then a "+N" overflow, matching
           the IssueList row layout so the visual vocabulary stays
           consistent across both list types. -->
      {#if page.labels.length > 0}
        <div class="flex items-center gap-1 shrink-0">
          {#each page.labels.slice(0, 2) as lbl}
            {@const labelObj = labels.find((l) => l.name === lbl)}
            <span
              class="text-[0.6875rem] font-medium px-1.5 py-0.5 rounded-full
                     border border-[var(--border)]"
              style={labelObj ? `color: ${labelObj.color}; border-color: ${labelObj.color}40;` : ""}
            >
              {lbl}
            </span>
          {/each}
          {#if page.labels.length > 2}
            <span class="text-[0.6875rem] text-[var(--text-faint)]">
              +{page.labels.length - 2}
            </span>
          {/if}
        </div>
      {/if}

      <span class="text-[0.8125rem] text-[var(--text-faint)] shrink-0 opacity-0 group-hover:opacity-100 transition-opacity">
        {formatRelative(page.updated_at)}
      </span>
    </button>
  {/each}

  <!-- Inline create form at this level -->
  {#if createTarget && createTarget.parentId === parentId}
    <div
      class="flex items-center gap-2 py-1"
      style="padding-left: {depth * 20 + 6}px;"
    >
      {#if createTarget.type === "folder"}
        <FolderOpen size={18} class="text-[var(--accent)] shrink-0" />
      {:else}
        <FileText size={18} class="text-[var(--accent)] shrink-0" />
      {/if}
      <!-- svelte-ignore a11y_autofocus -->
      <input
        type="text"
        bind:value={createName}
        class="flex-1 px-2 py-1 text-[0.9375rem] rounded
               border border-[var(--accent)] bg-transparent
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
{/snippet}

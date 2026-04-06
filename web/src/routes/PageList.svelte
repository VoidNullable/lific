<script lang="ts">
  import {
    listPages,
    listFolders,
    listProjects,
    createPage,
    createFolder,
    deleteFolder,
    updatePage,
    type Page,
    type Folder,
    type Project,
  } from "../lib/api";
  import {
    FileText,
    FolderOpen,
    FolderClosed,
    Plus,
    ChevronRight,
    Trash2,
  } from "lucide-svelte";

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
  let loading = $state(true);
  let error = $state("");

  let expandedFolders = $state<Set<number>>(new Set());

  // Drag and drop
  let draggedId = $state<{ type: "page" | "folder"; id: number } | null>(null);
  let dropTarget = $state<number | "root" | null>(null);

  // Inline create
  let createTarget = $state<{ type: "page" | "folder"; parentId: number | null } | null>(null);
  let createName = $state("");

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

    const [pRes, fRes] = await Promise.all([
      listPages(found.id),
      listFolders(found.id),
    ]);
    if (pRes.ok) pages = pRes.data;
    if (fRes.ok) {
      folders = fRes.data;
      expandedFolders = new Set(fRes.data.map((f: Folder) => f.id));
    }
    loading = false;
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

    <!-- Spacer -->
    <div class="flex-1"></div>

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
    {:else}
      <div class="max-w-[480px] px-6 py-4">
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

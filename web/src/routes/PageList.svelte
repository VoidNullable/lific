<script lang="ts">
  import {
    listPages,
    listFolders,
    listProjects,
    createPage,
    createFolder,
    deleteFolder,
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

  // Folder expand state
  let expandedFolders = $state<Set<number>>(new Set());

  // New page/folder
  let creatingPage = $state(false);
  let creatingFolder = $state(false);
  let newPageTitle = $state("");
  let newFolderName = $state("");
  let newPageFolderId = $state<number | null>(null);

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
      // Expand all folders by default
      expandedFolders = new Set(fRes.data.map((f: Folder) => f.id));
    }
    loading = false;
  }

  // Pages without a folder
  let loosePages = $derived(pages.filter((p) => !p.folder_id));

  // Pages grouped by folder
  function pagesInFolder(folderId: number): Page[] {
    return pages.filter((p) => p.folder_id === folderId);
  }

  function toggleFolder(id: number) {
    const next = new Set(expandedFolders);
    if (next.has(id)) next.delete(id);
    else next.add(id);
    expandedFolders = next;
  }

  function contentPreview(content: string): string {
    // Strip markdown headers, take first meaningful line
    const lines = content.split("\n").filter((l) => l.trim() && !l.startsWith("#"));
    const first = lines[0] ?? "";
    // Strip markdown formatting
    return first.replace(/[*_`\[\]]/g, "").slice(0, 120) || "Empty page";
  }

  function formatRelative(iso: string): string {
    const d = new Date(iso + "Z");
    const now = new Date();
    const diffMs = now.getTime() - d.getTime();
    const diffDays = Math.floor(diffMs / 86400000);
    if (diffDays < 1) return "today";
    if (diffDays === 1) return "yesterday";
    if (diffDays < 7) return `${diffDays}d ago`;
    return d.toLocaleDateString("en-US", { month: "short", day: "numeric" });
  }

  async function handleCreatePage() {
    if (!project || !newPageTitle.trim()) return;
    creatingPage = false;
    const res = await createPage({
      project_id: project.id,
      title: newPageTitle.trim(),
      folder_id: newPageFolderId ?? undefined,
    });
    if (res.ok) {
      navigate(`/${projectIdentifier}/pages/${res.data.id}`);
    }
    newPageTitle = "";
    newPageFolderId = null;
  }

  async function handleCreateFolder() {
    if (!project || !newFolderName.trim()) return;
    creatingFolder = false;
    const res = await createFolder({
      project_id: project.id,
      name: newFolderName.trim(),
    });
    if (res.ok) {
      folders = [...folders, res.data];
      expandedFolders = new Set([...expandedFolders, res.data.id]);
    }
    newFolderName = "";
  }

  async function handleDeleteFolder(id: number, e: Event) {
    e.stopPropagation();
    await deleteFolder(id);
    folders = folders.filter((f) => f.id !== id);
    // Pages in that folder become loose
    pages = pages.map((p) => p.folder_id === id ? { ...p, folder_id: null } : p);
  }

  function startNewPage(folderId: number | null = null) {
    newPageFolderId = folderId;
    newPageTitle = "";
    creatingPage = true;
  }
</script>

<div class="h-full flex flex-col">
  <!-- Header -->
  <div
    class="shrink-0 flex items-center justify-between px-6 py-3
           border-b border-[var(--border)] bg-[var(--surface)]"
  >
    <h1 class="text-[1.125rem] font-semibold text-[var(--text)] tracking-tight">
      Pages
    </h1>

    <div class="flex items-center gap-2">
      <button
        class="flex items-center gap-1.5 text-[0.8125rem]
               text-[var(--text-muted)] px-2.5 py-1.5 rounded-md
               hover:bg-[var(--bg-subtle)] hover:text-[var(--text)]
               transition-colors"
        onclick={() => { creatingFolder = true; newFolderName = ""; }}
      >
        <FolderOpen size={14} />
        Folder
      </button>
      <button
        class="flex items-center gap-1.5 text-[0.8125rem] font-medium
               text-[var(--accent-text)] bg-[var(--accent)] px-2.5 py-1.5
               rounded-md hover:bg-[var(--accent-hover)] transition-colors"
        onclick={() => startNewPage()}
      >
        <Plus size={14} />
        Page
      </button>
    </div>
  </div>

  <!-- Content -->
  <div class="flex-1 overflow-y-auto">
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
    {:else}
      <div class="max-w-[640px] mx-auto px-6 py-6">

        <!-- New folder inline form -->
        {#if creatingFolder}
          <div class="flex items-center gap-2 mb-4">
            <FolderOpen size={16} class="text-[var(--text-faint)] shrink-0" />
            <input
              type="text"
              bind:value={newFolderName}
              class="flex-1 px-2.5 py-1.5 text-[0.875rem] rounded-md
                     border border-[var(--accent)] bg-[var(--bg-subtle)]
                     text-[var(--text)] outline-none"
              placeholder="Folder name"
              autofocus
              onkeydown={(e) => {
                if (e.key === "Enter") handleCreateFolder();
                if (e.key === "Escape") { creatingFolder = false; }
              }}
              onblur={() => { if (!newFolderName.trim()) creatingFolder = false; }}
            />
          </div>
        {/if}

        <!-- Folders -->
        {#each folders as folder (folder.id)}
          {@const isExpanded = expandedFolders.has(folder.id)}
          {@const folderPages = pagesInFolder(folder.id)}
          <div class="mb-2">
            <!-- Folder header -->
            <!-- svelte-ignore a11y_no_static_element_interactions -->
            <div
              class="w-full flex items-center gap-2 px-2 py-2 -mx-2
                     rounded-md text-left group cursor-pointer
                     hover:bg-[var(--bg-subtle)] transition-colors"
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
                <FolderOpen size={16} class="shrink-0 text-[var(--accent)]" />
              {:else}
                <FolderClosed size={16} class="shrink-0 text-[var(--text-muted)]" />
              {/if}
              <span class="text-[0.875rem] font-medium text-[var(--text)] flex-1">
                {folder.name}
              </span>
              <span class="text-[0.75rem] text-[var(--text-faint)]">
                {folderPages.length}
              </span>
              <!-- Folder actions (visible on hover) -->
              <div
                class="flex items-center gap-1 opacity-0 group-hover:opacity-100
                       transition-opacity"
              >
                <button
                  class="size-6 flex items-center justify-center rounded
                         text-[var(--text-faint)] hover:text-[var(--accent)]
                         hover:bg-[var(--accent-subtle)]"
                  title="New page in folder"
                  onclick={(e) => { e.stopPropagation(); startNewPage(folder.id); }}
                >
                  <Plus size={12} />
                </button>
                <button
                  class="size-6 flex items-center justify-center rounded
                         text-[var(--text-faint)] hover:text-[var(--error)]
                         hover:bg-[var(--error-bg)]"
                  title="Delete folder"
                  onclick={(e) => handleDeleteFolder(folder.id, e)}
                >
                  <Trash2 size={12} />
                </button>
              </div>
            </div>

            <!-- Folder pages -->
            {#if isExpanded}
              <div class="ml-6 border-l border-[var(--border)]">
                {#if folderPages.length === 0}
                  <p class="text-[0.8125rem] text-[var(--text-faint)] py-3 pl-4">
                    No pages yet
                  </p>
                {:else}
                  {#each folderPages as page (page.id)}
                    {@render pageRow(page)}
                  {/each}
                {/if}
              </div>
            {/if}
          </div>
        {/each}

        <!-- Loose pages (no folder) -->
        {#if loosePages.length > 0 && folders.length > 0}
          <div class="mt-4 pt-4 border-t border-[var(--border)]">
            <span
              class="block text-[0.6875rem] font-semibold uppercase tracking-widest
                     text-[var(--text-faint)] mb-2 px-2"
            >
              Ungrouped
            </span>
          </div>
        {/if}
        {#each loosePages as page (page.id)}
          {@render pageRow(page)}
        {/each}

        <!-- New page inline form -->
        {#if creatingPage}
          <div class="flex items-center gap-2 mt-2 {newPageFolderId ? 'ml-6 pl-4 border-l border-[var(--border)]' : ''}">
            <FileText size={16} class="text-[var(--text-faint)] shrink-0" />
            <input
              type="text"
              bind:value={newPageTitle}
              class="flex-1 px-2.5 py-1.5 text-[0.875rem] rounded-md
                     border border-[var(--accent)] bg-[var(--bg-subtle)]
                     text-[var(--text)] outline-none"
              placeholder="Page title"
              autofocus
              onkeydown={(e) => {
                if (e.key === "Enter") handleCreatePage();
                if (e.key === "Escape") { creatingPage = false; }
              }}
              onblur={() => { if (!newPageTitle.trim()) creatingPage = false; }}
            />
          </div>
        {/if}

        <!-- Empty state -->
        {#if pages.length === 0 && folders.length === 0 && !creatingPage && !creatingFolder}
          <div class="flex flex-col items-center py-16 gap-3">
            <FileText size={32} class="text-[var(--text-faint)]" />
            <p class="text-[0.9375rem] text-[var(--text-muted)]">
              No pages yet
            </p>
            <button
              class="text-[0.8125rem] text-[var(--accent)] hover:underline"
              onclick={() => startNewPage()}
            >
              Create the first page
            </button>
          </div>
        {/if}
      </div>
    {/if}
  </div>
</div>

{#snippet pageRow(page: Page)}
  <button
    class="w-full flex items-center gap-3 px-4 py-3 -mx-2
           rounded-md text-left group
           hover:bg-[var(--bg-subtle)] transition-colors"
    onclick={() => navigate(`/${projectIdentifier}/pages/${page.id}`)}
  >
    <FileText size={16} class="shrink-0 text-[var(--text-faint)] group-hover:text-[var(--accent)]" />
    <div class="flex-1 min-w-0">
      <span class="block text-[0.875rem] text-[var(--text)] truncate">
        {page.title}
      </span>
      <span class="block text-[0.75rem] text-[var(--text-faint)] truncate mt-0.5">
        {contentPreview(page.content)}
      </span>
    </div>
    <span class="text-[0.75rem] text-[var(--text-faint)] shrink-0">
      {formatRelative(page.updated_at)}
    </span>
  </button>
{/snippet}

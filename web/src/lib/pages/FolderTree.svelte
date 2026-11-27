<script lang="ts">
  // The Pages folder pane: All pages and Conventions, the folder tree with
  // counts, then Archived. Picking an entry scopes the list; dropping a page
  // on a folder moves it there. Folders are created, renamed and deleted in
  // place.
  import {
    Archive,
    ChevronRight,
    FilePlus2,
    Files,
    FolderClosed,
    FolderOpen,
    FolderPlus,
    MoreHorizontal,
    PenLine,
    ScrollText,
    Trash2,
  } from "lucide-svelte";
  import type { Folder } from "../api";
  import Tooltip from "../Tooltip.svelte";
  import { orderedFolders, type FolderScope, type FolderStats } from "./list";

  let {
    folders,
    stats,
    allCount,
    archivedCount,
    conventions,
    scope,
    canEdit,
    dropTarget,
    dragging,
    onScope,
    onCreate,
    onRename,
    onDelete,
    onNewPage,
    onDragOver,
    onDrop,
  }: {
    folders: Folder[];
    stats: Map<number, FolderStats>;
    allCount: number;
    archivedCount: number;
    /** The conventions page: switched on, written but off, not written
     *  yet, or hidden (a viewer, and nothing written). */
    conventions: "on" | "off" | "missing" | "hidden";
    scope: FolderScope;
    canEdit: boolean;
    /** The folder a dragged page is over; null is Unfiled in the list. */
    dropTarget: number | null | undefined;
    dragging: boolean;
    onScope: (scope: FolderScope) => void;
    onCreate: (parentId: number | null, name: string) => Promise<boolean>;
    onRename: (id: number, name: string) => Promise<boolean>;
    onDelete: (id: number) => Promise<boolean>;
    onNewPage: (folderId: number) => void;
    onDragOver: (target: number | undefined) => void;
    onDrop: (target: number | null) => void;
  } = $props();

  let collapsed = $state<Set<number>>(new Set());
  /** Inline editor: a new folder under `parent`, or a rename of `id`. */
  let editing = $state<{ kind: "new"; parent: number | null } | { kind: "rename"; id: number } | null>(null);
  let draft = $state("");
  let busy = $state(false);
  let menuFor = $state<number | null>(null);
  let confirmDelete = $state<number | null>(null);

  const nodes = $derived(orderedFolders(folders));
  const hasKids = $derived(new Set(folders.map((f) => f.parent_id).filter((p): p is number => p != null)));

  /** A folder is hidden when any ancestor is collapsed. */
  function hidden(f: Folder): boolean {
    let parent = f.parent_id;
    let guard = 0;
    while (parent != null && guard++ < 32) {
      if (collapsed.has(parent)) return true;
      parent = folders.find((x) => x.id === parent)?.parent_id ?? null;
    }
    return false;
  }

  function toggle(id: number) {
    const next = new Set(collapsed);
    if (next.has(id)) next.delete(id);
    else next.add(id);
    collapsed = next;
  }

  export function startNew(parent: number | null = null) {
    if (!canEdit) return;
    menuFor = null;
    if (parent != null && collapsed.has(parent)) toggle(parent);
    editing = { kind: "new", parent };
    draft = "";
  }

  function startRename(f: Folder) {
    menuFor = null;
    editing = { kind: "rename", id: f.id };
    draft = f.name;
  }

  async function commit() {
    const name = draft.trim();
    const e = editing;
    if (!e || busy) return;
    if (!name) {
      editing = null;
      return;
    }
    busy = true;
    const ok = e.kind === "new" ? await onCreate(e.parent, name) : await onRename(e.id, name);
    busy = false;
    if (ok) editing = null;
  }

  function focusInput(node: HTMLInputElement) {
    requestAnimationFrame(() => {
      node.focus();
      node.select();
    });
  }

  const rowBase =
    "w-full flex items-center gap-2 h-8 pr-2 rounded-md text-left text-body-sm transition-colors";
  function rowTone(active: boolean, target: boolean): string {
    if (target) return "bg-[var(--accent-subtle)] text-[var(--text)] outline outline-1 outline-dashed outline-[var(--accent)]";
    if (active) return "bg-[var(--surface)] text-[var(--text)] font-medium shadow-[0_1px_2px_rgba(0,0,0,0.08)]";
    return "text-[var(--text-muted)] hover:bg-[color-mix(in_srgb,var(--surface)_70%,transparent)] hover:text-[var(--text)]";
  }
</script>

<svelte:window onclick={() => { menuFor = null; }} />

{#snippet editor(depth: number)}
  <div class="flex items-center gap-2 h-8 pr-2" style="padding-left: {depth * 14 + 8}px">
    <FolderClosed size={14} class="shrink-0 text-[var(--accent)]" />
    <input
      class="flex-1 min-w-0 h-6 px-1.5 rounded text-body-sm bg-[var(--surface)] text-[var(--text)]
             border border-[var(--accent)] outline-none shadow-[0_0_0_3px_var(--accent-subtle)]"
      placeholder="Folder name"
      aria-label={editing?.kind === "rename" ? "Rename folder" : "New folder name"}
      bind:value={draft}
      disabled={busy}
      use:focusInput
      onkeydown={(e) => {
        if (e.key === "Enter") { e.preventDefault(); void commit(); }
        if (e.key === "Escape") { e.preventDefault(); e.stopPropagation(); editing = null; }
      }}
      onblur={() => { if (!busy) void commit(); }}
    />
  </div>
{/snippet}

<nav class="flex flex-col gap-0.5 text-body-sm" aria-label="Folders" data-testid="folder-tree">
  <!-- svelte-ignore a11y_no_static_element_interactions -->
  <button
    class="{rowBase} pl-2 {rowTone(scope === 'all', false)}"
    aria-current={scope === "all" ? "true" : undefined}
    onclick={() => onScope("all")}
  >
    <Files size={14} class="shrink-0 text-[var(--text-faint)]" />
    <span class="flex-1 truncate">All pages</span>
    <span class="tabular-nums text-caption text-[var(--text-faint)]">{allCount}</span>
  </button>

  {#if conventions !== "hidden"}
    <button
      class="{rowBase} pl-2 {rowTone(scope === 'conventions', false)}"
      aria-current={scope === "conventions" ? "true" : undefined}
      data-testid="conventions-entry"
      data-state={conventions}
      onclick={() => onScope("conventions")}
    >
      <ScrollText size={14} class="shrink-0 {conventions === 'on' ? 'text-[var(--accent)]' : 'text-[var(--text-faint)]'}" />
      <span class="flex-1 truncate">Conventions</span>
      {#if conventions === "missing"}
        <span class="text-caption text-[var(--text-faint)]">Not written</span>
      {:else if conventions === "off"}
        <span class="text-caption text-[var(--text-faint)]">Off</span>
      {/if}
    </button>
  {/if}

  <div class="flex items-center justify-between mt-4 mb-1 pl-2 pr-1">
    <h2 class="text-micro font-semibold uppercase tracking-widest text-[var(--text-faint)]">Folders</h2>
    {#if canEdit}
      <Tooltip content="New folder">
        <button
          class="size-6 grid place-items-center rounded-md text-[var(--text-faint)]
                 hover:text-[var(--text)] hover:bg-[var(--surface)] transition-colors"
          aria-label="New folder"
          onclick={(e) => { e.stopPropagation(); startNew(null); }}
        >
          <FolderPlus size={14} />
        </button>
      </Tooltip>
    {/if}
  </div>

  {#if editing?.kind === "new" && editing.parent === null}
    {@render editor(0)}
  {/if}

  {#if folders.length === 0 && !editing}
    <p class="px-2 py-1 text-caption text-[var(--text-faint)] leading-relaxed">
      {canEdit ? "No folders yet. Group related pages with one." : "No folders."}
    </p>
  {/if}

  {#each nodes as { folder, depth } (folder.id)}
    {#if !hidden(folder)}
      {@const s = stats.get(folder.id)}
      {@const active = scope === folder.id}
      {@const open = !collapsed.has(folder.id)}
      {#if editing?.kind === "rename" && editing.id === folder.id}
        {@render editor(depth)}
      {:else}
        <!-- svelte-ignore a11y_no_static_element_interactions, a11y_click_events_have_key_events -->
        <div
          class="group relative {rowBase} {rowTone(active, dragging && dropTarget === folder.id)}"
          style="padding-left: {depth * 14 + 2}px"
          data-folder-id={folder.id}
          ondragover={(e) => { if (!dragging) return; e.preventDefault(); onDragOver(folder.id); }}
          ondragleave={() => onDragOver(undefined)}
          ondrop={(e) => { e.preventDefault(); onDrop(folder.id); }}
        >
          <button
            class="size-5 shrink-0 grid place-items-center rounded text-[var(--text-faint)]
                   hover:text-[var(--text)] {hasKids.has(folder.id) ? '' : 'invisible'}"
            aria-label={open ? `Collapse ${folder.name}` : `Expand ${folder.name}`}
            aria-expanded={open}
            tabindex={hasKids.has(folder.id) ? 0 : -1}
            onclick={(e) => { e.stopPropagation(); toggle(folder.id); }}
          >
            <ChevronRight size={12} class="transition-transform {open ? 'rotate-90' : ''}" />
          </button>
          <button
            class="flex-1 min-w-0 flex items-center gap-2 h-full text-left"
            aria-current={active ? "true" : undefined}
            onclick={() => onScope(folder.id)}
            ondblclick={() => { if (canEdit) startRename(folder); }}
          >
            {#if active}
              <FolderOpen size={14} class="shrink-0 text-[var(--accent)]" />
            {:else}
              <FolderClosed size={14} class="shrink-0 text-[var(--text-faint)]" />
            {/if}
            <span class="flex-1 truncate">{folder.name}</span>
          </button>
          <span
            class="tabular-nums text-caption text-[var(--text-faint)]
                   {canEdit ? '[@media(hover:hover)]:group-hover:hidden' : ''} {menuFor === folder.id ? 'hidden' : ''}"
          >
            {s?.total ?? 0}
          </span>
          {#if canEdit}
            <button
              class="size-6 -mr-1 shrink-0 place-items-center rounded-md text-[var(--text-faint)]
                     hover:text-[var(--text)] hover:bg-[var(--bg-subtle)]
                     {menuFor === folder.id ? 'grid' : 'hidden [@media(hover:hover)]:group-hover:grid pointer-coarse:grid'}"
              aria-label={`${folder.name} actions`}
              aria-haspopup="menu"
              aria-expanded={menuFor === folder.id}
              onclick={(e) => {
                e.stopPropagation();
                confirmDelete = null;
                menuFor = menuFor === folder.id ? null : folder.id;
              }}
            >
              <MoreHorizontal size={14} />
            </button>
          {/if}

          {#if menuFor === folder.id}
            <!-- svelte-ignore a11y_no_static_element_interactions, a11y_click_events_have_key_events -->
            <div
              role="menu"
              tabindex="-1"
              class="popover-in origin-top-right absolute right-0 top-full mt-1 z-40 w-[220px]
                     bg-[var(--surface)] border border-[var(--border)] rounded-xl shadow-lg p-1 font-normal"
              onclick={(e) => e.stopPropagation()}
            >
              {#if confirmDelete === folder.id}
                <div class="px-2.5 py-2 flex flex-col gap-2">
                  <p class="text-body-sm text-[var(--text)]">
                    Delete {folder.name}{hasKids.has(folder.id) ? " and its subfolders" : ""}?
                  </p>
                  <p class="text-caption text-[var(--text-muted)]">
                    {(s?.total ?? 0) === 0 ? "It holds no pages." : "Its pages move to Unfiled."}
                  </p>
                  <div class="flex gap-1.5">
                    <button
                      class="h-7 px-2.5 rounded-md text-caption font-medium bg-[var(--error)] text-white hover:opacity-90"
                      disabled={busy}
                      onclick={async () => {
                        busy = true;
                        const ok = await onDelete(folder.id);
                        busy = false;
                        if (ok) { menuFor = null; confirmDelete = null; }
                      }}
                    >
                      Delete folder
                    </button>
                    <button
                      class="h-7 px-2.5 rounded-md text-caption text-[var(--text-muted)] hover:bg-[var(--bg-subtle)]"
                      onclick={() => (confirmDelete = null)}
                    >
                      Cancel
                    </button>
                  </div>
                </div>
              {:else}
                {#each [
                  { label: "New page here", icon: FilePlus2, run: () => { menuFor = null; onNewPage(folder.id); } },
                  { label: "New subfolder", icon: FolderPlus, run: () => startNew(folder.id) },
                  { label: "Rename", icon: PenLine, run: () => startRename(folder) },
                ] as item (item.label)}
                  <button
                    role="menuitem"
                    class="w-full flex items-center gap-2.5 px-2.5 h-8 rounded-md text-left text-body-sm
                           text-[var(--text)] hover:bg-[var(--bg-subtle)] transition-colors"
                    onclick={item.run}
                  >
                    <item.icon size={14} class="text-[var(--text-muted)]" />
                    {item.label}
                  </button>
                {/each}
                <div class="my-1 mx-1 h-px bg-[var(--border)]"></div>
                <button
                  role="menuitem"
                  class="w-full flex items-center gap-2.5 px-2.5 h-8 rounded-md text-left text-body-sm
                         text-[var(--error)] hover:bg-[var(--error-bg)] transition-colors"
                  onclick={() => (confirmDelete = folder.id)}
                >
                  <Trash2 size={14} />
                  Delete folder…
                </button>
              {/if}
            </div>
          {/if}
        </div>
      {/if}
      {#if editing?.kind === "new" && editing.parent === folder.id}
        {@render editor(depth + 1)}
      {/if}
    {/if}
  {/each}

  {#if archivedCount > 0 || scope === "archived"}
    <button
      class="{rowBase} pl-2 mt-4 {rowTone(scope === 'archived', false)}"
      aria-current={scope === "archived" ? "true" : undefined}
      data-testid="archived-entry"
      onclick={() => onScope("archived")}
    >
      <Archive size={14} class="shrink-0 text-[var(--text-faint)]" />
      <span class="flex-1 truncate">Archived</span>
      <span class="tabular-nums text-caption text-[var(--text-faint)]">{archivedCount}</span>
    </button>
  {/if}
</nav>

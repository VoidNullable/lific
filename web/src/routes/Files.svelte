<script lang="ts">
  // LIF-418 — the project Files manager.
  //
  // Every file attached anywhere in the project, in one place: filter by type
  // and uploader, sort, see the total the project is carrying, and jump from a
  // file to the issue, page, or comment that uses it. Below it, collapsed, the
  // uploads that never got linked and are counting down to the orphan sweeper.
  //
  // All the decision logic (chip sets, countdown wording, the delete-permission
  // mirror, entity routes) lives in lib/files/files.ts so it can be unit
  // tested; this component fetches and renders.

  import {
    listProjects,
    listProjectAttachments,
    listProjectOrphans,
    getAttachmentLinks,
    deleteAttachment,
    downloadAttachment,
    formatBytes,
    me,
    type AttachmentLinks,
    type LinkedEntity,
    type MimeClass,
    type PendingOrphan,
    type Project,
    type ProjectAttachment,
  } from "../lib/api";
  import {
    MIME_FILTERS,
    SORT_OPTIONS,
    canDeleteAttachment,
    deleteConfirmMessage,
    entityChipLabel,
    entityHref,
    formatSweepCountdown,
    uploaderOptions,
  } from "../lib/files/files";
  import { projectRole, loadProjectRole } from "../lib/projectRole.svelte";
  import { startAutoRefresh } from "../lib/autoRefresh.svelte";
  import TimeAgo from "../lib/TimeAgo.svelte";
  import ErrorState from "../lib/ErrorState.svelte";
  import Skeleton from "../lib/Skeleton.svelte";
  import { toast } from "../lib/toast/toast.svelte";
  import {
    ChevronDown,
    ChevronRight,
    File,
    FileArchive,
    FileText,
    FileType,
    Image,
    Music,
    Paperclip,
    Trash2,
    TriangleAlert,
    Video,
  } from "lucide-svelte";
  import { getContext } from "svelte";

  const PAGE_SIZE = 50;

  let {
    navigate,
    projectIdentifier,
  }: {
    navigate: (path: string) => void;
    projectIdentifier: string;
  } = $props();

  const topbarCtx = getContext<{
    set: (s: import("svelte").Snippet | undefined) => void;
  } | undefined>("lific:topbar");

  $effect(() => {
    topbarCtx?.set(topbarContent);
    return () => topbarCtx?.set(undefined);
  });

  let project = $state<Project | null>(null);
  let items = $state<ProjectAttachment[]>([]);
  let totalCount = $state(0);
  let totalBytes = $state(0);
  let hasMore = $state(false);
  let loading = $state(true);
  let loadingMore = $state(false);
  let error = $state("");

  // Filters + sort.
  let mimeFilter = $state<MimeClass | null>(null);
  let uploaderFilter = $state<string>("");
  let sort = $state<"created_at" | "size" | "filename">("created_at");

  // Row expansion (where-used) and the inline delete confirm.
  let expandedId = $state<number | null>(null);
  let links = $state<Record<number, AttachmentLinks | null>>({});
  let confirmingId = $state<number | null>(null);
  let deletingId = $state<number | null>(null);

  // Orphans: collapsed by default. This is a cleanup surface, not the point
  // of the page, and an empty one is the healthy state.
  let orphansOpen = $state(false);
  let orphans = $state<PendingOrphan[]>([]);
  let orphanBytes = $state(0);

  // The signed-in user, for the client-side half of the delete gate.
  let viewerId = $state<number | null>(null);
  const canEdit = $derived(projectRole.canEdit);
  const isAdmin = $derived(projectRole.isAdmin);

  // Uploader options come from the loaded rows. Narrowing by uploader can
  // therefore only offer names the current filter has seen, which is the
  // honest thing to offer without a separate roster fetch.
  let uploaders = $derived(uploaderOptions(items));

  $effect(() => {
    const ident = projectIdentifier;
    resetView();
    void loadProject(ident);
  });

  // Refetch when a filter or the sort changes. `lastQueryKey` is a plain (non
  // reactive) local, so the initial load kicked off by loadProject can record
  // its own key and this effect won't fire a duplicate request for it.
  let lastQueryKey = "";
  function queryKey(projectId: number): string {
    return [projectId, mimeFilter ?? "", uploaderFilter, sort].join("|");
  }

  $effect(() => {
    const key = project ? queryKey(project.id) : "";
    if (!project || key === lastQueryKey) return;
    void loadPage(project.id, true);
  });

  $effect(() =>
    startAutoRefresh({
      refresh: async () => {
        if (project) await loadPage(project.id, true);
      },
      isBusy: () => loading || loadingMore || deletingId !== null,
      shouldRefresh: (event) =>
        event.type === "resync.required" ||
        (typeof event.project_id === "number" && event.project_id === project?.id),
    }),
  );

  function resetView() {
    lastQueryKey = "";
    items = [];
    totalCount = 0;
    totalBytes = 0;
    expandedId = null;
    confirmingId = null;
    links = {};
    orphans = [];
    orphansOpen = false;
  }

  async function loadProject(ident: string) {
    loading = true;
    error = "";
    const projects = await listProjects();
    if (!projects.ok) {
      error = projects.error;
      loading = false;
      return;
    }
    const found = projects.data.find(
      (p) => p.identifier.toLowerCase() === ident.toLowerCase(),
    );
    if (!found) {
      error = `Project ${ident} not found`;
      loading = false;
      return;
    }
    project = found;
    void loadProjectRole(found.id);
    if (viewerId === null) {
      const who = await me();
      if (who.ok) viewerId = who.data.id;
    }
    await Promise.all([loadPage(found.id, true), loadOrphans(found.id)]);
    loading = false;
  }

  async function loadPage(projectId: number, replace: boolean) {
    const offset = replace ? 0 : items.length;
    lastQueryKey = queryKey(projectId);
    if (!replace) loadingMore = true;
    const res = await listProjectAttachments(projectId, {
      mime_class: mimeFilter,
      uploader: uploaderFilter || null,
      sort,
      limit: PAGE_SIZE,
      offset,
    });
    if (res.ok) {
      items = replace ? res.data.items : [...items, ...res.data.items];
      totalCount = res.data.total_count;
      totalBytes = res.data.total_bytes;
      hasMore = res.data.has_more;
      error = "";
    } else {
      error = res.error;
    }
    loadingMore = false;
  }

  async function loadOrphans(projectId: number) {
    const res = await listProjectOrphans(projectId);
    if (res.ok) {
      orphans = res.data.items;
      orphanBytes = res.data.total_bytes;
    }
  }

  /** Expand a row and fetch its where-used detail.
   *
   *  `/api/attachments/{id}/links` is served by a sibling workstream. When it
   *  isn't there we simply show the entities the listing already carries, so
   *  the expander works either way. */
  async function toggleExpand(row: ProjectAttachment) {
    if (expandedId === row.id) {
      expandedId = null;
      return;
    }
    expandedId = row.id;
    if (links[row.id] !== undefined) return;
    const res = await getAttachmentLinks(row.id);
    links = { ...links, [row.id]: res.ok ? res.data : null };
  }

  /** The entities to show in an expanded row: the richer where-used answer
   *  when we have it, otherwise the project-scoped links from the listing. */
  function shownEntities(row: ProjectAttachment): LinkedEntity[] {
    return links[row.id]?.entities ?? row.entities;
  }

  function goToEntity(entity: LinkedEntity) {
    const href = entityHref(projectIdentifier, entity);
    if (href) navigate(href);
  }

  async function confirmDelete(id: number, referenceCount: number) {
    deletingId = id;
    const res = await deleteAttachment(id);
    deletingId = null;
    confirmingId = null;
    if (!res.ok) {
      toast(`Couldn't delete the file: ${res.error}`, { kind: "error" });
      return;
    }
    toast(
      referenceCount > 0
        ? `File deleted, along with ${referenceCount} reference${referenceCount === 1 ? "" : "s"}.`
        : "File deleted.",
      { kind: "success" },
    );
    // Refresh both lists: a deleted file leaves the listing, and deleting an
    // orphan changes the pending set.
    if (project) {
      await Promise.all([loadPage(project.id, true), loadOrphans(project.id)]);
    }
  }

  function mayDelete(uploaderId: number | null): boolean {
    return canDeleteAttachment({ uploaderId, viewerId, isAdmin, canEdit });
  }
</script>

{#snippet typeIcon(cls: MimeClass, size: number)}
  {#if cls === "image"}
    <Image {size} class="shrink-0 text-[var(--accent)]" />
  {:else if cls === "video"}
    <Video {size} class="shrink-0 text-[var(--accent)]" />
  {:else if cls === "audio"}
    <Music {size} class="shrink-0 text-[var(--accent)]" />
  {:else if cls === "text"}
    <FileText {size} class="shrink-0 text-[var(--text-muted)]" />
  {:else if cls === "pdf"}
    <FileType {size} class="shrink-0 text-[var(--text-muted)]" />
  {:else if cls === "archive"}
    <FileArchive {size} class="shrink-0 text-[var(--text-muted)]" />
  {:else}
    <File {size} class="shrink-0 text-[var(--text-faint)]" />
  {/if}
{/snippet}

{#snippet entityChips(entities: LinkedEntity[])}
  <div class="flex flex-wrap items-center gap-1">
    {#each entities as entity (entity.entity_type + entity.entity_id)}
      <button
        class="text-micro font-mono px-1.5 py-0.5 rounded
               bg-[var(--bg-subtle)] text-[var(--text-muted)]
               hover:text-[var(--accent)] transition-colors"
        title={entity.title}
        onclick={() => goToEntity(entity)}
      >
        {entityChipLabel(entity)}
      </button>
    {/each}
    {#if entities.length === 0}
      <span class="text-micro text-[var(--text-faint)]">no references</span>
    {/if}
  </div>
{/snippet}

{#snippet topbarContent()}
  <div class="flex items-center gap-3 px-6 py-2 w-full">
    <div class="flex items-center gap-1.5 shrink-0">
      <button
        class="text-body-sm font-mono font-medium text-[var(--text-muted)]
               hover:text-[var(--text)] transition-colors"
        onclick={() => navigate(`/${projectIdentifier}/overview`)}
      >
        {projectIdentifier}
      </button>
      <ChevronRight size={12} class="text-[var(--text-faint)]" />
      <span class="text-body-sm font-medium text-[var(--text)]">Files</span>
      {#if !loading}
        <span class="ml-1 text-micro text-[var(--text-faint)] font-medium tabular-nums">
          {totalCount}
        </span>
      {/if}
    </div>
    {#if !loading}
      <span class="text-caption text-[var(--text-faint)] tabular-nums">
        {formatBytes(totalBytes)} total
      </span>
    {/if}
  </div>
{/snippet}

<div class="h-full flex flex-col">
  <div class="flex-1 overflow-y-auto">
    <div class="px-8 py-6 max-w-[1100px] mx-auto">
      {#if loading}
        <div class="flex flex-col gap-2">
          <Skeleton variant="bar" class="h-6 w-64 mb-4" />
          {#each Array(6) as _, i (i)}
            <div class="flex items-center gap-3 px-2.5 py-2">
              <Skeleton variant="circle" class="size-4" />
              <Skeleton variant="bar" class="h-3 flex-1 max-w-[360px]" />
              <Skeleton variant="bar" class="h-2.5 w-12 shrink-0" />
              <Skeleton variant="bar" class="h-2.5 w-16 shrink-0" />
            </div>
          {/each}
        </div>
      {:else if error}
        <ErrorState title="Couldn't load files" message={error}>
          <button
            class="text-body-sm font-medium text-[var(--btn-success-text)] bg-[var(--btn-success)]
                   px-3 py-1.5 rounded-md hover:bg-[var(--btn-success-hover)] transition-colors"
            onclick={() => loadProject(projectIdentifier)}
          >
            Try again
          </button>
        </ErrorState>
      {:else}
        <!-- Filters: type chips, uploader, sort, and what it all adds up to. -->
        <div class="flex flex-wrap items-center gap-2 mb-5">
          <div class="flex flex-wrap items-center gap-1">
            {#each MIME_FILTERS as chip (chip.label)}
              <button
                class="text-caption px-2.5 py-1 rounded-full border transition-colors
                       {mimeFilter === chip.value
                  ? 'border-[var(--accent)] text-[var(--accent)] bg-[var(--accent-subtle)] font-medium'
                  : 'border-[var(--border)] text-[var(--text-muted)] hover:text-[var(--text)] hover:bg-[var(--bg-subtle)]'}"
                onclick={() => (mimeFilter = chip.value)}
              >
                {chip.label}
              </button>
            {/each}
          </div>

          <div class="flex-1"></div>

          <select
            class="h-7 px-2 rounded-md text-caption bg-[var(--bg)]
                   border border-[var(--border)] text-[var(--text-muted)]"
            aria-label="Filter by uploader"
            bind:value={uploaderFilter}
          >
            <option value="">All uploaders</option>
            {#each uploaders as name (name)}
              <option value={name}>{name}</option>
            {/each}
          </select>

          <select
            class="h-7 px-2 rounded-md text-caption bg-[var(--bg)]
                   border border-[var(--border)] text-[var(--text-muted)]"
            aria-label="Sort files"
            bind:value={sort}
          >
            {#each SORT_OPTIONS as option (option.value)}
              <option value={option.value}>{option.label}</option>
            {/each}
          </select>
        </div>

        <div class="flex items-baseline gap-2 mb-3">
          <span class="text-body-sm text-[var(--text)] font-medium tabular-nums">
            {totalCount} file{totalCount === 1 ? "" : "s"}
          </span>
          <span class="text-caption text-[var(--text-faint)] tabular-nums">
            {formatBytes(totalBytes)}
          </span>
        </div>

        {#if items.length === 0}
          <div class="flex flex-col items-center py-20 gap-3 text-center">
            <Paperclip size={32} class="text-[var(--text-faint)]" />
            <p class="text-body-lg text-[var(--text-muted)]">No files here yet</p>
            <p class="text-body-sm text-[var(--text-faint)] max-w-[420px]">
              Files appear once they are attached to an issue, page, or comment
              in this project.
            </p>
          </div>
        {:else}
          <div class="flex flex-col divide-y divide-[var(--border)]">
            {#each items as row (row.id)}
              {@const expanded = expandedId === row.id}
              <div class="py-2">
                <div class="flex items-center gap-3">
                  <button
                    class="size-5 flex items-center justify-center rounded
                           text-[var(--text-faint)] hover:text-[var(--text)]
                           hover:bg-[var(--bg-subtle)] transition-colors shrink-0"
                    title={expanded ? "Hide where this is used" : "Show where this is used"}
                    aria-expanded={expanded}
                    onclick={() => toggleExpand(row)}
                  >
                    {#if expanded}
                      <ChevronDown size={14} />
                    {:else}
                      <ChevronRight size={14} />
                    {/if}
                  </button>

                  {@render typeIcon(row.mime_class, 16)}

                  <button
                    class="min-w-0 flex-1 text-left text-body-sm text-[var(--text)]
                           truncate hover:text-[var(--accent)] transition-colors"
                    title="Download {row.filename}"
                    onclick={() => void downloadAttachment(row.id, row.filename)}
                  >
                    {row.filename}
                  </button>

                  <div class="hidden sm:block shrink-0">
                    {@render entityChips(row.entities)}
                  </div>

                  <span class="text-caption text-[var(--text-faint)] tabular-nums w-16 text-right shrink-0">
                    {formatBytes(row.size_bytes)}
                  </span>
                  <span class="hidden md:block text-caption text-[var(--text-muted)] w-24 truncate shrink-0">
                    {row.uploader ?? "unknown"}
                  </span>
                  <span class="text-caption text-[var(--text-faint)] w-16 text-right shrink-0">
                    <TimeAgo date={row.created_at} />
                  </span>

                  {#if mayDelete(row.uploader_id)}
                    <button
                      class="size-6 flex items-center justify-center rounded shrink-0
                             text-[var(--text-faint)] hover:text-[var(--error)]
                             hover:bg-[var(--bg-subtle)] transition-colors"
                      title="Delete {row.filename}"
                      disabled={deletingId === row.id}
                      onclick={() =>
                        (confirmingId = confirmingId === row.id ? null : row.id)}
                    >
                      <Trash2 size={14} />
                    </button>
                  {/if}
                </div>

                <!-- Inline confirm, not a modal: the row stays in view so it's
                     obvious which file is about to go. -->
                {#if confirmingId === row.id}
                  <div class="flex flex-wrap items-center gap-2 mt-2 ml-8 pl-3 border-l-2 border-[var(--error)]">
                    <span class="text-caption text-[var(--text-muted)]">
                      {deleteConfirmMessage(shownEntities(row).length)}
                    </span>
                    <button
                      class="text-caption font-medium px-2 py-1 rounded-md
                             text-[var(--error-text)] bg-[var(--error)]
                             hover:opacity-90 transition-opacity"
                      disabled={deletingId === row.id}
                      onclick={() => confirmDelete(row.id, shownEntities(row).length)}
                    >
                      {deletingId === row.id ? "Deleting…" : "Delete"}
                    </button>
                    <button
                      class="text-caption text-[var(--text-muted)] px-2 py-1 rounded-md
                             hover:bg-[var(--bg-subtle)] transition-colors"
                      onclick={() => (confirmingId = null)}
                    >
                      Cancel
                    </button>
                  </div>
                {/if}

                {#if expanded}
                  <div class="mt-2 ml-8 flex flex-col gap-2">
                    <div class="flex flex-col gap-1">
                      <span class="text-micro uppercase tracking-widest text-[var(--text-faint)] font-semibold">
                        Used by
                      </span>
                      {@render entityChips(shownEntities(row))}
                    </div>
                    {#if links[row.id]?.duplicates?.length}
                      <div class="flex flex-col gap-1">
                        <span class="text-micro uppercase tracking-widest text-[var(--text-faint)] font-semibold">
                          Identical file also attached to
                        </span>
                        {#each links[row.id]!.duplicates as duplicate (duplicate.id)}
                          <div class="flex items-center gap-2">
                            <span class="text-caption text-[var(--text-muted)] truncate">
                              {duplicate.filename}
                            </span>
                            {@render entityChips(duplicate.entities)}
                          </div>
                        {/each}
                      </div>
                    {/if}
                    <span class="text-micro text-[var(--text-faint)]">
                      {row.mime}
                    </span>
                  </div>
                {/if}
              </div>
            {/each}
          </div>

          {#if hasMore}
            <div class="flex justify-center py-4">
              <button
                class="text-body-sm text-[var(--text-muted)] border border-[var(--border)]
                       px-3 py-1.5 rounded-md hover:bg-[var(--bg-subtle)] transition-colors"
                disabled={loadingMore}
                onclick={() => project && loadPage(project.id, false)}
              >
                {loadingMore ? "Loading…" : "Load more"}
              </button>
            </div>
          {/if}
        {/if}

        <!-- Pending cleanup. Collapsed by default: it is a warning surface,
             and most of the time it is empty. -->
        <section class="mt-10 border-t border-[var(--border)] pt-4">
          <button
            class="w-full flex items-center gap-2 text-left"
            aria-expanded={orphansOpen}
            onclick={() => (orphansOpen = !orphansOpen)}
          >
            {#if orphansOpen}
              <ChevronDown size={14} class="text-[var(--text-faint)]" />
            {:else}
              <ChevronRight size={14} class="text-[var(--text-faint)]" />
            {/if}
            <TriangleAlert size={14} class="text-[var(--error)]" />
            <span class="text-body-sm font-medium text-[var(--text)]">
              Pending cleanup
            </span>
            <span class="text-caption text-[var(--text-faint)] tabular-nums">
              {orphans.length}
              {#if orphans.length > 0}
                · {formatBytes(orphanBytes)}
              {/if}
            </span>
          </button>

          {#if orphansOpen}
            <p class="text-caption text-[var(--text-muted)] mt-2 mb-3 max-w-[560px]">
              Uploads by this project's members that were never attached to
              anything. The server deletes them automatically once their grace
              window runs out.
            </p>
            {#if orphans.length === 0}
              <p class="text-caption text-[var(--text-faint)]">
                Nothing waiting to be swept.
              </p>
            {:else}
              <div class="flex flex-col divide-y divide-[var(--border)]">
                {#each orphans as orphan (orphan.id)}
                  <div class="flex items-center gap-3 py-2">
                    <TriangleAlert
                      size={14}
                      class="shrink-0 text-[var(--error)]"
                    />
                    <span class="min-w-0 flex-1 text-body-sm text-[var(--text-muted)] truncate">
                      {orphan.filename}
                    </span>
                    <span class="text-caption text-[var(--text-faint)] tabular-nums w-16 text-right shrink-0">
                      {formatBytes(orphan.size_bytes)}
                    </span>
                    <span class="hidden md:block text-caption text-[var(--text-muted)] w-24 truncate shrink-0">
                      {orphan.uploader ?? "unknown"}
                    </span>
                    <span class="text-caption w-40 text-right shrink-0 text-[var(--text-muted)]">
                      {formatSweepCountdown(orphan.seconds_until_sweep)}
                    </span>
                    {#if mayDelete(orphan.uploader_id)}
                      <button
                        class="size-6 flex items-center justify-center rounded shrink-0
                               text-[var(--text-faint)] hover:text-[var(--error)]
                               hover:bg-[var(--bg-subtle)] transition-colors"
                        title="Delete {orphan.filename} now"
                        disabled={deletingId === orphan.id}
                        onclick={() =>
                          (confirmingId = confirmingId === orphan.id ? null : orphan.id)}
                      >
                        <Trash2 size={14} />
                      </button>
                    {/if}
                  </div>
                  {#if confirmingId === orphan.id}
                    <div class="flex flex-wrap items-center gap-2 py-2 pl-3 border-l-2 border-[var(--error)]">
                      <span class="text-caption text-[var(--text-muted)]">
                        {deleteConfirmMessage(0)}
                      </span>
                      <button
                        class="text-caption font-medium px-2 py-1 rounded-md
                               text-[var(--error-text)] bg-[var(--error)]
                               hover:opacity-90 transition-opacity"
                        disabled={deletingId === orphan.id}
                        onclick={() => confirmDelete(orphan.id, 0)}
                      >
                        {deletingId === orphan.id ? "Deleting…" : "Delete"}
                      </button>
                      <button
                        class="text-caption text-[var(--text-muted)] px-2 py-1 rounded-md
                               hover:bg-[var(--bg-subtle)] transition-colors"
                        onclick={() => (confirmingId = null)}
                      >
                        Cancel
                      </button>
                    </div>
                  {/if}
                {/each}
              </div>
            {/if}
          {/if}
        </section>
      {/if}
    </div>
  </div>
</div>

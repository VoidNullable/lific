<script lang="ts">
  // The Pages reading pane: the focused page, rendered, beside the list.
  // J/K in the list moves it; Open (or Enter) goes to the full page.
  import { untrack } from "svelte";
  import { ArrowUpRight, Link as LinkIcon, Pin, X } from "lucide-svelte";
  import { getPage, type Folder, type Label, type Page, type PageMeta } from "../api";
  import Markdown from "../Markdown.svelte";
  import Skeleton from "../Skeleton.svelte";
  import TimeAgo from "../TimeAgo.svelte";
  import { copyToClipboard } from "../clipboard";
  import { safeLabelColor } from "../labelColors";
  import EditorMark from "./EditorMark.svelte";
  import { describeLength, describeLinks, folderPathLabel, personLabel, STATUS_LABEL, type ListPage } from "./list";

  let {
    row,
    meta,
    folders,
    labels,
    href,
    canEdit,
    resolveRef,
    navigate,
    onOpen,
    onClose,
    onTogglePin,
    onStatus,
  }: {
    row: ListPage;
    meta: PageMeta | undefined;
    folders: Folder[];
    labels: Label[];
    href: string;
    canEdit: boolean;
    /** In-app path for an identifier that links here, or null. */
    resolveRef: (identifier: string) => string | null;
    navigate: (path: string) => void;
    onOpen: () => void;
    onClose: () => void;
    onTogglePin: () => void;
    onStatus: (status: string) => void;
  } = $props();

  let page = $state<Page | null>(null);
  let loading = $state(false);
  let error = $state("");
  let scroller = $state<HTMLDivElement | null>(null);

  // Refetch when the row's revision moves, so an edit elsewhere shows here.
  $effect(() => {
    const id = row.id;
    void row.updated_at;
    let cancelled = false;
    untrack(() => {
      if (page?.id !== id) {
        page = null;
        scroller?.scrollTo({ top: 0 });
      }
    });
    loading = true;
    error = "";
    (async () => {
      const res = await getPage(id);
      if (cancelled) return;
      if (res.ok) page = res.data;
      else error = res.error;
      loading = false;
    })();
    return () => {
      cancelled = true;
    };
  });

  const folder = $derived(row.folder_id != null ? folderPathLabel(folders, row.folder_id) : "");
  const length = $derived(describeLength(meta?.words ?? 0));
  const linksLine = $derived(describeLinks(meta));
  const labelColor = (name: string) => {
    const l = labels.find((x) => x.name === name);
    return l ? safeLabelColor(l.color) : "var(--text-faint)";
  };
  const STATUSES = ["draft", "active", "complete"] as const;
</script>

<article class="h-full flex flex-col min-h-0" aria-label="{row.identifier} preview" data-testid="page-pane">
  <header class="shrink-0 px-5 pt-4 pb-3 border-b border-[var(--border)]">
    <div class="flex items-center gap-2 text-caption text-[var(--text-faint)] min-w-0">
      <span class="font-mono shrink-0">{row.identifier}</span>
      {#if folder}
        <span aria-hidden="true">·</span>
        <span class="truncate">{folder}</span>
      {/if}
      <span class="flex-1"></span>
      <button
        class="pane-icon"
        title="Copy link"
        aria-label="Copy link to {row.identifier}"
        onclick={() => copyToClipboard(`${location.origin}${location.pathname}${href}`, "link")}
      >
        <LinkIcon size={13} />
      </button>
      {#if canEdit}
        <button
          class="pane-icon {row.pinned ? 'text-[var(--accent)]' : ''}"
          title={row.pinned ? "Unpin" : "Pin"}
          aria-label={row.pinned ? "Unpin" : "Pin"}
          aria-pressed={row.pinned}
          onclick={onTogglePin}
        >
          <Pin size={13} class={row.pinned ? "fill-current" : ""} />
        </button>
      {/if}
      <button class="pane-icon" title="Close preview" aria-label="Close preview" onclick={onClose}>
        <X size={14} />
      </button>
    </div>

    <h2 class="mt-1.5 text-heading font-display tracking-tight leading-snug text-[var(--text)]">
      <a
        {href}
        class="text-[var(--text)] hover:underline decoration-[var(--text-faint)] underline-offset-2"
        onclick={(e) => {
          if (e.metaKey || e.ctrlKey || e.shiftKey || e.button !== 0) return;
          e.preventDefault();
          onOpen();
        }}
      >
        {row.title || "Untitled"}
      </a>
    </h2>

    <div class="mt-2 flex flex-wrap items-center gap-x-3 gap-y-1 text-caption text-[var(--text-muted)]">
      {#if meta?.updated_by}
        <span class="inline-flex items-center gap-1.5 min-w-0">
          <EditorMark person={meta.updated_by} />
          <span class="truncate max-w-[12rem]">{personLabel(meta.updated_by)}</span>
          <TimeAgo date={row.updated_at} class="text-[var(--text-faint)]" />
        </span>
      {:else}
        <span>Updated <TimeAgo date={row.updated_at} /></span>
      {/if}
      {#if length}<span>{length}</span>{/if}
    </div>

    {#if row.labels.length > 0}
      <div class="mt-2 flex flex-wrap items-center gap-x-3 gap-y-1">
        {#each row.labels as name (name)}
          <span class="flex items-center gap-1.5 text-caption text-[var(--text-muted)]">
            <span class="size-2 rounded-full" style="background: {labelColor(name)}"></span>{name}
          </span>
        {/each}
      </div>
    {/if}

    <div class="mt-3 flex flex-wrap items-center gap-1.5">
      <button type="button" class="pane-btn pane-btn-primary" onclick={onOpen}>
        Open <ArrowUpRight size={13} />
      </button>
      {#if canEdit && row.status !== "archived"}
        <div class="inline-flex rounded-[0.4rem] border border-[var(--border)] overflow-hidden" role="group" aria-label="Status">
          {#each STATUSES as s (s)}
            <button
              type="button"
              class="h-7 px-2.5 text-caption font-medium transition-colors
                     {row.status === s
                ? 'bg-[var(--bg-subtle)] text-[var(--text)]'
                : 'bg-[var(--surface)] text-[var(--text-muted)] hover:text-[var(--text)]'}
                     {s !== 'draft' ? 'border-l border-[var(--border)]' : ''}"
              aria-pressed={row.status === s}
              onclick={() => row.status !== s && onStatus(s)}
            >
              {STATUS_LABEL[s]}
            </button>
          {/each}
        </div>
      {:else}
        <span class="text-caption text-[var(--text-muted)] px-1">{STATUS_LABEL[row.status] ?? row.status}</span>
      {/if}
      {#if canEdit && row.status === "archived"}
        <button type="button" class="pane-btn" onclick={() => onStatus("active")}>Restore</button>
      {/if}
    </div>
  </header>

  <div class="flex-1 min-h-0 overflow-y-auto px-5 py-4" bind:this={scroller}>
    {#if loading && !page}
      <div class="flex flex-col gap-2.5">
        <Skeleton variant="bar" class="h-3.5 w-full" />
        <Skeleton variant="bar" class="h-3.5 w-full" />
        <Skeleton variant="bar" class="h-3.5 w-5/6" />
        <Skeleton variant="bar" class="h-3.5 w-2/3" />
      </div>
    {:else if error && !page}
      <p class="text-body-sm text-[var(--text-muted)]">Couldn't load this page: {error}</p>
    {:else if page}
      {#if page.content.trim()}
        <div class="pane-body text-body">
          <Markdown content={page.content} />
        </div>
      {:else}
        <p class="text-body-sm text-[var(--text-faint)] italic">This page is empty.</p>
      {/if}
    {/if}

    {#if meta && meta.linked_from.length > 0}
      <section class="mt-8 pt-4 border-t border-[var(--border)]">
        <h3 class="pane-label mb-2">{linksLine}</h3>
        <div class="flex flex-wrap gap-1.5">
          {#each meta.linked_from as ref (ref)}
            {@const path = resolveRef(ref)}
            {#if path}
              <a
                href="#{path}"
                class="text-caption font-mono px-1.5 py-0.5 rounded border border-[var(--border)]
                       text-[var(--text-muted)] hover:text-[var(--accent)] hover:border-[var(--accent)] transition-colors"
                onclick={(e) => {
                  if (e.metaKey || e.ctrlKey || e.shiftKey || e.button !== 0) return;
                  e.preventDefault();
                  navigate(path);
                }}
              >
                {ref}
              </a>
            {:else}
              <span class="text-caption font-mono px-1.5 py-0.5 rounded border border-[var(--border)] text-[var(--text-faint)]">{ref}</span>
            {/if}
          {/each}
        </div>
      </section>
    {:else if meta && linksLine}
      <p class="mt-8 pt-4 border-t border-[var(--border)] text-caption text-[var(--text-faint)]">{linksLine}</p>
    {/if}
  </div>
</article>

<style>
  .pane-label {
    font-size: var(--text-micro);
    text-transform: uppercase;
    letter-spacing: 0.08em;
    font-weight: 600;
    color: var(--text-faint);
  }
  .pane-btn {
    display: inline-flex;
    align-items: center;
    gap: 0.35rem;
    height: 1.75rem;
    padding: 0 0.6rem;
    border-radius: 0.4rem;
    border: 1px solid var(--border);
    background: var(--surface);
    color: var(--text);
    font-size: var(--text-caption);
    font-weight: 500;
    transition: background-color 120ms, border-color 120ms;
  }
  .pane-btn:hover:not(:disabled) {
    background: var(--bg-subtle);
  }
  .pane-btn-primary {
    background: var(--accent);
    border-color: var(--accent);
    color: var(--accent-text);
  }
  .pane-btn-primary:hover:not(:disabled) {
    background: var(--accent-hover);
  }
  .pane-icon {
    display: grid;
    place-items: center;
    width: 1.5rem;
    height: 1.5rem;
    border-radius: 0.35rem;
    color: var(--text-faint);
  }
  .pane-icon:hover {
    background: var(--bg-subtle);
    color: var(--text);
  }
</style>

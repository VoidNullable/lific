<script lang="ts">
  /*
   * Read-only page preview panel — the pages sibling of issues/PeekPanel.
   * Bottom sheet on mobile (<768px), right side panel on md+, same scrim,
   * transitions, and close affordances as the issue peek so the two read as
   * one mechanism. Deliberately read-only where the issue peek is editable:
   * a page is a document, and the preview's job is reading it; edits live
   * in the full view one tap away.
   */
  import { fade, fly } from "svelte/transition";
  import { X, ArrowUpRight, Pin } from "lucide-svelte";
  import { getPage, type Page } from "../api";
  import { pagePeekState, closePagePeek } from "./pagePeek.svelte";
  import Markdown from "../Markdown.svelte";
  import Skeleton from "../Skeleton.svelte";
  import { motionReduced } from "../theme";

  let { navigate }: { navigate: (path: string) => void } = $props();

  let page = $state<Page | null>(null);
  let loading = $state(false);
  let error = $state("");

  // Fetch fresh on every open/swap — list rows carry content too, but it's
  // as stale as the list load and the peek should read like the real page.
  $effect(() => {
    const id = pagePeekState.pageId;
    if (id === null || !pagePeekState.open) return;
    loading = true;
    error = "";
    let cancelled = false;
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

  function isMobileViewport(): boolean {
    return typeof window !== "undefined" && window.innerWidth < 768;
  }
  function panelInParams() {
    if (motionReduced()) return { duration: 0 };
    return isMobileViewport() ? { y: 480, duration: 240 } : { x: 480, duration: 240 };
  }
  function panelOutParams() {
    if (motionReduced()) return { duration: 0 };
    return isMobileViewport() ? { y: 480, duration: 180 } : { x: 480, duration: 180 };
  }
  function scrimParams() {
    return motionReduced() ? { duration: 0 } : { duration: 180 };
  }

  function openFull() {
    const href = pagePeekState.href;
    closePagePeek();
    if (href) navigate(href);
  }

  function handleKeydown(e: KeyboardEvent) {
    if (!pagePeekState.open) return;
    if (e.key === "Escape") {
      e.preventDefault();
      closePagePeek();
    }
  }

  const STATUS_LABEL: Record<string, string> = {
    draft: "Draft",
    active: "Active",
    complete: "Complete",
    archived: "Archived",
  };
</script>

<svelte:window onkeydown={handleKeydown} />

{#if pagePeekState.open}
  <!-- svelte-ignore a11y_no_static_element_interactions, a11y_click_events_have_key_events -->
  <div
    class="fixed inset-0 z-[90] bg-black/30 backdrop-blur-[1px]"
    onclick={closePagePeek}
    transition:fade={scrimParams()}
  ></div>

  <div
    class="fixed z-[95] flex flex-col bg-[var(--surface)] shadow-2xl
           inset-x-0 bottom-0 h-[85dvh] rounded-t-xl border-t border-[var(--border)]
           pb-[env(safe-area-inset-bottom)]
           md:inset-y-0 md:right-0 md:left-auto md:bottom-auto
           md:h-full md:w-[520px] md:max-w-[92vw]
           md:rounded-none md:border-t-0 md:border-l"
    in:fly={panelInParams()}
    out:fly={panelOutParams()}
    role="dialog"
    aria-modal="true"
    aria-label={page ? `${page.title} preview` : "Page preview"}
  >
    <!-- Drag-handle visual (mobile bottom sheet only, decorative). -->
    <div class="md:hidden flex justify-center pt-2 pb-1 shrink-0">
      <div class="h-1 w-9 rounded-full bg-[var(--border)]"></div>
    </div>

    <div class="shrink-0 flex items-center gap-2 px-4 pt-2 pb-2 md:pt-4 border-b border-[var(--border)]">
      {#if page}
        <span
          class="text-caption font-mono font-semibold px-1.5 py-0.5 rounded
                 border border-[var(--border)] text-[var(--text-muted)]"
        >
          {page.identifier}
        </span>
        <span class="text-caption text-[var(--text-faint)]">
          {STATUS_LABEL[page.status] ?? page.status}
        </span>
        {#if page.pinned}
          <Pin size={12} class="text-[var(--text-faint)]" />
        {/if}
      {/if}
      <div class="flex-1"></div>
      <button
        class="size-7 flex items-center justify-center rounded-md
               text-[var(--text-faint)] hover:text-[var(--text)]
               hover:bg-[var(--bg-subtle)] transition-colors"
        aria-label="Close preview"
        onclick={closePagePeek}
      >
        <X size={16} />
      </button>
    </div>

    <div class="flex-1 overflow-y-auto px-4 py-4">
      {#if loading && !page}
        <div>
          <Skeleton variant="bar" class="h-6 w-3/4 mt-1 mb-5" />
          <div class="flex flex-col gap-2.5">
            <Skeleton variant="bar" class="h-3.5 w-full" />
            <Skeleton variant="bar" class="h-3.5 w-full" />
            <Skeleton variant="bar" class="h-3.5 w-5/6" />
            <Skeleton variant="bar" class="h-3.5 w-2/3" />
          </div>
        </div>
      {:else if error}
        <div class="flex flex-col items-center gap-2 py-16 text-center">
          <p class="text-body-sm text-[var(--text-muted)]">Couldn't load this page.</p>
          <p class="text-caption text-[var(--text-faint)]">{error}</p>
        </div>
      {:else if page}
        <h2 class="text-title font-display tracking-tight text-[var(--text)] mb-3">
          {page.title}
        </h2>
        {#if page.labels.length > 0}
          <div class="flex flex-wrap items-center gap-1.5 mb-4">
            {#each page.labels as label (label)}
              <span
                class="text-caption px-1.5 py-0.5 rounded-md border border-[var(--border)]
                       text-[var(--text-muted)]"
              >
                {label}
              </span>
            {/each}
          </div>
        {/if}
        <div class="border-t border-[var(--border)] -mx-4 mb-4"></div>
        {#if page.content.trim()}
          <Markdown content={page.content} />
        {:else}
          <p class="text-body-sm text-[var(--text-faint)] italic">This page is empty.</p>
        {/if}
      {/if}
    </div>

    <div class="shrink-0 border-t border-[var(--border)] px-4 py-3">
      <button
        class="w-full flex items-center justify-center gap-1.5 h-9 rounded-md
               text-body-sm font-medium text-[var(--btn-success-text)]
               bg-[var(--btn-success)] hover:bg-[var(--btn-success-hover)] transition-colors"
        onclick={openFull}
      >
        Open full page
        <ArrowUpRight size={14} />
      </button>
    </div>
  </div>
{/if}

<script lang="ts">
  // LIF-202 — a proper styled modal for picking an issue, replacing the
  // native window.prompt used by the plan UI. Searches the project's
  // issues (FTS server-side) with debounce + keyboard navigation, and
  // also fast-paths a typed identifier (e.g. "LIF-42" / "42") so the
  // old free-text flow still works for people who know the number.
  //
  // Chrome and interaction model mirror CommandPalette so the two feel
  // like one family: dimmed backdrop, centered card, arrow/enter to
  // pick, Esc to close, mouse hover tracks selection.

  import {
    search as searchApi,
    resolveIssue,
    type Issue,
  } from "./api";
  import StatusIcon from "./StatusIcon.svelte";
  import { Search, CornerDownLeft, X } from "lucide-svelte";
  import { tick } from "svelte";

  type Hit = {
    id: number;
    identifier: string;
    title: string;
    status?: string;
  };

  let {
    open = $bindable(false),
    projectId,
    projectIdentifier,
    title = "Link an issue",
    /** Issue currently selected, so it can be shown / pre-filtered. */
    currentIdentifier = null,
    /** Show a "Clear" action that resolves with null (used by anchor). */
    allowClear = false,
    onSelect,
    onClear,
  }: {
    open?: boolean;
    projectId: number;
    projectIdentifier: string;
    title?: string;
    currentIdentifier?: string | null;
    allowClear?: boolean;
    onSelect: (issue: Issue) => void;
    onClear?: () => void;
  } = $props();

  // Same reasoning as CommandPalette: the long form of this placeholder
  // clips on a phone, where the overlay goes full-screen (LIF-227).
  let innerWidth = $state(1024);
  let narrow = $derived(innerWidth < 640);

  let query = $state("");
  let inputEl = $state<HTMLInputElement | null>(null);
  let listEl = $state<HTMLDivElement | null>(null);
  let selectedIdx = $state(0);
  let hits = $state<Hit[]>([]);
  let searching = $state(false);
  let resolving = $state(false);
  let errorMsg = $state("");
  let searchGen = 0;

  // Open transition: focus the input and seed the list with recent /
  // identifier-shaped results.
  $effect(() => {
    if (open) {
      query = "";
      hits = [];
      selectedIdx = 0;
      errorMsg = "";
      tick().then(() => {
        inputEl?.focus();
        void runSearch("");
      });
    }
  });

  function close() {
    open = false;
  }

  /** Does the query look like a bare issue reference for this project? */
  function identifierShape(q: string): string | null {
    const compact = q.trim();
    // "LIF-42" / "lif 42" / "LIF42"
    const m = compact.match(/^([a-z][a-z0-9_]*?)[\s-]*(\d+)$/i);
    if (m) {
      const proj = m[1].toUpperCase();
      if (proj === projectIdentifier.toUpperCase()) {
        return `${projectIdentifier}-${parseInt(m[2])}`;
      }
      return `${proj}-${parseInt(m[2])}`;
    }
    // bare number → assume current project
    const bare = compact.match(/^(\d+)$/);
    if (bare) return `${projectIdentifier}-${parseInt(bare[1])}`;
    return null;
  }

  async function runSearch(q: string) {
    const gen = ++searchGen;
    const trimmed = q.trim();
    errorMsg = "";

    searching = true;
    const idShape = identifierShape(trimmed);

    const [idHit, ftsRes] = await Promise.all([
      idShape ? resolveIssue(idShape) : Promise.resolve(null),
      trimmed ? searchApi(trimmed, projectId) : Promise.resolve(null),
    ]);
    if (gen !== searchGen) return; // superseded

    const merged: Hit[] = [];
    const seen = new Set<string>();

    if (idHit && idHit.ok) {
      merged.push({
        id: idHit.data.id,
        identifier: idHit.data.identifier,
        title: idHit.data.title,
        status: idHit.data.status,
      });
      seen.add(idHit.data.identifier);
    }

    if (ftsRes && ftsRes.ok) {
      for (const r of ftsRes.data) {
        if (r.result_type !== "issue" || !r.identifier) continue;
        if (seen.has(r.identifier)) continue;
        if (r.project_id !== null && r.project_id !== projectId) continue;
        merged.push({
          id: r.id,
          identifier: r.identifier,
          title: r.title,
        });
        seen.add(r.identifier);
      }
    }

    hits = merged;
    selectedIdx = 0;
    searching = false;
  }

  let debounce: ReturnType<typeof setTimeout> | null = null;
  function onInput() {
    if (debounce) clearTimeout(debounce);
    debounce = setTimeout(() => runSearch(query), 120);
  }

  async function pick(hit: Hit) {
    resolving = true;
    errorMsg = "";
    const res = await resolveIssue(hit.identifier);
    resolving = false;
    if (!res.ok) {
      errorMsg = res.error;
      return;
    }
    onSelect(res.data);
    close();
  }

  function clear() {
    onClear?.();
    close();
  }

  function onKeydown(e: KeyboardEvent) {
    if (e.key === "Escape") {
      e.preventDefault();
      close();
      return;
    }
    if (e.key === "ArrowDown") {
      e.preventDefault();
      selectedIdx = Math.min(selectedIdx + 1, hits.length - 1);
      scrollSelectedIntoView();
    } else if (e.key === "ArrowUp") {
      e.preventDefault();
      selectedIdx = Math.max(selectedIdx - 1, 0);
      scrollSelectedIntoView();
    } else if (e.key === "Enter") {
      e.preventDefault();
      const hit = hits[selectedIdx];
      if (hit) void pick(hit);
    }
  }

  function scrollSelectedIntoView() {
    requestAnimationFrame(() => {
      listEl
        ?.querySelector(`[data-idx="${selectedIdx}"]`)
        ?.scrollIntoView({ block: "nearest" });
    });
  }
</script>

<svelte:window bind:innerWidth />

{#if open}
  <!-- svelte-ignore a11y_no_static_element_interactions a11y_click_events_have_key_events -->
  <div
    class="fixed inset-0 z-[100] bg-black/25 flex items-start justify-center
           sm:pt-[14dvh] sm:px-4"
    onclick={close}
  >
    <!-- svelte-ignore a11y_no_static_element_interactions a11y_click_events_have_key_events -->
    <!-- LIF-227: full-screen on phones for the same reason as the command
         palette — this opens straight into a focused search field, and a
         bottom sheet would sit under the software keyboard. -->
    <div
      class="w-full h-full flex flex-col bg-[var(--surface)]
             border-[var(--border)] shadow-[0_16px_48px_rgba(0,0,0,0.28)] overflow-hidden
             sm:h-auto sm:max-w-[520px] sm:border sm:rounded-xl"
      onclick={(e) => e.stopPropagation()}
    >
      <!-- Header / input row -->
      <div
        class="shrink-0 flex items-center gap-2.5 px-4 py-3 border-b border-[var(--border)]
               pt-[max(0.75rem,env(safe-area-inset-top))] sm:pt-3"
      >
        <Search size={15} class="shrink-0 text-[var(--text-faint)]" />
        <input
          bind:this={inputEl}
          bind:value={query}
          type="text"
          class="flex-1 bg-transparent border-0 outline-none text-body-lg
                 text-[var(--text)] placeholder:text-[var(--text-faint)]"
          placeholder={narrow
            ? `Search ${projectIdentifier} issues…`
            : `Search ${projectIdentifier} issues or type ${projectIdentifier}-42…`}
          oninput={onInput}
          onkeydown={onKeydown}
        />
        <kbd
          class="hidden sm:block px-1.5 py-0.5 rounded border border-[var(--border)]
                 bg-[var(--bg-subtle)] text-[var(--text-faint)]
                 font-mono text-micro leading-none shrink-0"
        >
          esc
        </kbd>
        <button
          class="sm:hidden size-11 -mr-2 shrink-0 grid place-items-center rounded-lg
                 text-[var(--text-muted)] active:bg-[var(--bg-subtle)] transition-colors"
          aria-label="Close issue search"
          onclick={close}
        >
          <X size={20} />
        </button>
      </div>

      <!-- Context line: what this modal acts on -->
      <div
        class="px-4 py-1.5 text-micro uppercase tracking-widest
               font-semibold text-[var(--text-faint)] flex items-center gap-2"
      >
        <span>{title}</span>
        {#if currentIdentifier}
          <span class="font-mono text-[var(--text-muted)] normal-case tracking-normal">
            · current: {currentIdentifier}
          </span>
        {/if}
      </div>

      {#if errorMsg}
        <div class="mx-4 mb-2 text-body-sm text-[var(--error)] bg-[var(--error-bg,var(--bg-subtle))] rounded-md px-3 py-1.5">
          {errorMsg}
        </div>
      {/if}

      <!-- Results -->
      <div
        class="flex-1 min-h-0 overflow-y-auto overscroll-contain py-1.5
               sm:flex-none sm:max-h-[360px]"
        bind:this={listEl}
      >
        {#if hits.length === 0}
          <p class="px-4 py-6 text-center text-body-sm text-[var(--text-faint)]">
            {searching
              ? "Searching…"
              : query.trim()
                ? `No issues match “${query.trim()}”`
                : "Type to search issues…"}
          </p>
        {:else}
          {#each hits as hit, i (hit.identifier)}
            <button
              class="w-full flex items-center gap-2.5 px-4 py-2 text-left
                     transition-colors
                     {i === selectedIdx
                ? 'bg-[var(--accent-subtle)]'
                : 'hover:bg-[var(--bg-subtle)]'}"
              data-idx={i}
              disabled={resolving}
              onclick={() => pick(hit)}
              onmouseenter={() => (selectedIdx = i)}
            >
              <span class="size-5 flex items-center justify-center shrink-0">
                {#if hit.status}
                  <StatusIcon status={hit.status} size={14} />
                {/if}
              </span>
              <span class="flex-1 min-w-0 text-body text-[var(--text)] truncate">
                {hit.title}
              </span>
              <span class="font-mono text-micro text-[var(--text-faint)] shrink-0">
                {hit.identifier}
              </span>
              {#if i === selectedIdx}
                <CornerDownLeft size={12} class="shrink-0 text-[var(--text-faint)]" />
              {/if}
            </button>
          {/each}
        {/if}
      </div>

      <!-- Footer -->
      <div
        class="shrink-0 flex items-center gap-3 px-4 py-2.5 border-t border-[var(--border)]
               text-micro text-[var(--text-faint)]
               pb-[max(0.625rem,env(safe-area-inset-bottom))] sm:pb-2.5"
      >
        <span class="inline-flex items-center gap-1">
          <kbd class="font-mono">↑↓</kbd> navigate
        </span>
        <span class="inline-flex items-center gap-1">
          <kbd class="font-mono">↵</kbd> select
        </span>
        {#if allowClear && currentIdentifier}
          <button
            class="ml-auto inline-flex items-center gap-1 text-[var(--text-muted)]
                   hover:text-[var(--error)] transition-colors"
            onclick={clear}
          >
            <X size={12} /> Clear anchor
          </button>
        {/if}
      </div>
    </div>
  </div>
{/if}

<script lang="ts">
  // The Pages sidebar while no page is open in the reading pane: what was
  // edited lately and by whom, drafts that are waiting, active pages nobody
  // has touched in a while, and which pages are linked most (or not at all).
  import { Link2 } from "lucide-svelte";
  import type { PageMeta } from "../api";
  import TimeAgo from "../TimeAgo.svelte";
  import { now } from "../now.svelte";
  import EditorMark from "./EditorMark.svelte";
  import { draftAge, linkTotal, personLabel, quietPages, QUIET_DAYS, daysSince, type ListPage } from "./list";

  let {
    pages,
    meta,
    metaLoaded,
    onPick,
    onShowDrafts,
    onShowUnlinked,
  }: {
    /** Live (not archived) pages, conventions page excluded. */
    pages: ListPage[];
    meta: Map<number, PageMeta>;
    metaLoaded: boolean;
    onPick: (page: ListPage) => void;
    onShowDrafts: () => void;
    onShowUnlinked: () => void;
  } = $props();

  const clock = $derived(new Date(now()));
  const recent = $derived(pages.slice().sort((a, b) => b.updated_at.localeCompare(a.updated_at)).slice(0, 5));
  const drafts = $derived(
    pages.filter((p) => p.status === "draft").sort((a, b) => a.created_at.localeCompare(b.created_at)),
  );
  const quiet = $derived(quietPages(pages, clock));
  const linked = $derived(
    pages
      .map((p) => ({ page: p, n: linkTotal(meta.get(p.id)) }))
      .filter((x) => x.n > 0)
      .sort((a, b) => b.n - a.n || b.page.updated_at.localeCompare(a.page.updated_at))
      .slice(0, 5),
  );
  const unlinked = $derived(metaLoaded ? pages.filter((p) => meta.has(p.id) && linkTotal(meta.get(p.id)) === 0).length : 0);
</script>

{#snippet item(page: ListPage, trailing: import("svelte").Snippet)}
  <li>
    <button
      class="w-full flex items-center gap-2 h-8 px-2 -mx-2 rounded-md text-left text-body-sm
             text-[var(--text)] hover:bg-[var(--bg-subtle)] transition-colors"
      onclick={() => onPick(page)}
    >
      <span class="flex-1 min-w-0 truncate">{page.title || "Untitled"}</span>
      {@render trailing()}
    </button>
  </li>
{/snippet}

<div class="h-full overflow-y-auto px-5 py-5 flex flex-col gap-7" data-testid="pages-pulse">
  {#if recent.length > 0}
    <section>
      <h3 class="pulse-label">Recently edited</h3>
      <ul class="mt-2 flex flex-col">
        {#each recent as page (page.id)}
          {@const who = meta.get(page.id)?.updated_by}
          {#snippet when()}
            {#if who}
              <EditorMark person={who} tip={personLabel(who)} />
            {/if}
            <TimeAgo date={page.updated_at} class="w-14 text-right shrink-0 text-caption tabular-nums text-[var(--text-faint)]" />
          {/snippet}
          {@render item(page, when)}
        {/each}
      </ul>
    </section>
  {/if}

  {#if drafts.length > 0}
    <section>
      <div class="flex items-baseline justify-between">
        <h3 class="pulse-label">Drafts waiting <span class="tabular-nums">{drafts.length}</span></h3>
        <button class="text-caption text-[var(--accent)] hover:underline" onclick={onShowDrafts}>All drafts</button>
      </div>
      <ul class="mt-2 flex flex-col">
        {#each drafts.slice(0, 3) as page (page.id)}
          {#snippet age()}
            <span class="shrink-0 text-caption text-[var(--text-faint)]">{draftAge(page.created_at, clock).replace("Draft for ", "")}</span>
          {/snippet}
          {@render item(page, age)}
        {/each}
      </ul>
    </section>
  {/if}

  {#if quiet.length > 0}
    <section>
      <h3 class="pulse-label">Quiet for {QUIET_DAYS}+ days <span class="tabular-nums">{quiet.length}</span></h3>
      <p class="mt-1 text-caption text-[var(--text-faint)]">Active pages nobody has edited in a while.</p>
      <ul class="mt-2 flex flex-col">
        {#each quiet.slice(0, 3) as page (page.id)}
          {#snippet idle()}
            <span class="shrink-0 text-caption tabular-nums text-[var(--text-faint)]">{daysSince(page.updated_at, clock)}d</span>
          {/snippet}
          {@render item(page, idle)}
        {/each}
      </ul>
    </section>
  {/if}

  {#if linked.length > 0}
    <section>
      <h3 class="pulse-label">Most linked</h3>
      <ul class="mt-2 flex flex-col">
        {#each linked as { page, n } (page.id)}
          {#snippet count()}
            <span class="shrink-0 flex items-center gap-1 text-caption tabular-nums text-[var(--text-muted)]">
              <Link2 size={12} class="text-[var(--text-faint)]" />{n}
            </span>
          {/snippet}
          {@render item(page, count)}
        {/each}
      </ul>
    </section>
  {/if}

  {#if unlinked > 0}
    <section class="rounded-xl bg-[var(--bg-subtle)] px-3.5 py-3">
      <p class="text-body-sm text-[var(--text)]">
        <span class="tabular-nums font-medium">{unlinked}</span>
        {unlinked === 1 ? "page has" : "pages have"} no links from issues, pages or comments.
      </p>
      <button class="mt-1 text-caption text-[var(--accent)] hover:underline" onclick={onShowUnlinked}>
        Show {unlinked === 1 ? "it" : "them"}
      </button>
    </section>
  {/if}
</div>

<style>
  .pulse-label {
    font-size: var(--text-micro);
    text-transform: uppercase;
    letter-spacing: 0.08em;
    font-weight: 600;
    color: var(--text-faint);
  }
</style>

<script lang="ts">
  // LIF-239 — hover preview for an auto-linked issue identifier.
  //
  // Positioning follows Tooltip.svelte's technique: `position: fixed`
  // with viewport coordinates computed from the trigger's
  // getBoundingClientRect(), so the card escapes any `overflow: hidden`
  // ancestor (a rounded content panel, a scrollable comment list, etc).
  // Unlike Tooltip, the trigger here isn't a Svelte-owned child — it's a
  // raw `<a>` produced by Markdown.svelte's `{@html}` output — so the
  // caller (Markdown.svelte) passes the trigger element in directly
  // instead of this component wrapping it via a snippet.
  //
  // Show/hide timing (the 350ms hover delay and the mouseleave grace
  // period) is owned by Markdown.svelte, since it has to coordinate
  // across every auto-linked identifier in the rendered output, not
  // just one. This component owns: data fetching (via the shared
  // session cache in references.ts), sizing/position/flip, and
  // forwarding its own mouseenter/mouseleave so hovering the card
  // itself (e.g. to read a long title) keeps it open.

  import { tick } from "svelte";
  import { fetchIssueCached, fetchModuleCached, type CachedIssue } from "./references";
  import StatusIcon from "./StatusIcon.svelte";
  import PriorityIcon from "./PriorityIcon.svelte";
  import ProjectIcon from "./ProjectIcon.svelte";
  import { Layers } from "lucide-svelte";
  import type { Module } from "./api";

  let {
    identifier,
    anchorEl,
    onEnter,
    onLeave,
    showDescription = false,
  }: {
    identifier: string;
    anchorEl: HTMLElement;
    onEnter: () => void;
    onLeave: () => void;
    /** LIF-363: also preview the issue's description (markdown stripped to
     *  plain text, clamped). On by the dependency graph, whose nodes already
     *  show status/id/priority/title — the card's value there is the
     *  content. Markdown auto-links keep the compact metadata-only card. */
    showDescription?: boolean;
  } = $props();

  /** Reduce markdown to tooltip-safe plain text: drop code fences, images
   *  and reference syntax, unwrap links/emphasis, strip list/heading/quote
   *  markers. Not a parser — just enough that a preview reads as prose. */
  function stripMarkdown(md: string): string {
    return md
      .replace(/```[\s\S]*?```/g, " ") // fenced code blocks
      .replace(/`([^`]*)`/g, "$1") // inline code
      .replace(/!\[[^\]]*\]\([^)]*\)/g, " ") // images
      .replace(/\[([^\]]*)\]\([^)]*\)/g, "$1") // links → text
      .replace(/^#{1,6}\s+/gm, "") // headings
      .replace(/^\s*(?:[-*+]|\d+\.)\s+(?:\[[ xX]\]\s*)?/gm, "") // list markers
      .replace(/^\s*>\s?/gm, "") // blockquotes
      .replace(/(\*\*|__|\*|_|~~)/g, "") // emphasis
      .replace(/\s+/g, " ")
      .trim();
  }

  let cardEl = $state<HTMLDivElement | null>(null);
  let cached = $state<CachedIssue | null>(null);
  let mod = $state<Module | null>(null);
  // Gates opacity so the first-paint jump to (0,0) before position is
  // computed never flashes — same trick as Tooltip.svelte.
  let positioned = $state(false);
  let coords = $state({ x: 0, y: 0 });

  // Fetch (cached) whenever the identifier changes. A hover card
  // instance is only ever mounted for one identifier at a time (see
  // Markdown.svelte's `{#if hoverIdent}`), but identifiers can still
  // change without an unmount if the user glides the mouse from one
  // link straight to another before the grace-period hide fires.
  $effect(() => {
    const ident = identifier;
    cached = null;
    mod = null;
    let cancelled = false;
    (async () => {
      const result = await fetchIssueCached(ident);
      if (cancelled) return;
      cached = result;
      if (result.status === "ok" && result.issue.module_id != null) {
        const m = await fetchModuleCached(result.issue.module_id);
        if (!cancelled) mod = m;
      }
    })();
    return () => {
      cancelled = true;
    };
  });

  async function computePosition() {
    if (!cardEl) return;
    const a = anchorEl.getBoundingClientRect();
    const c = cardEl.getBoundingClientRect();
    const gap = 8;

    // Prefer below the anchor; flip above when there isn't room.
    let y = a.bottom + gap;
    if (y + c.height > window.innerHeight - 8) {
      y = a.top - c.height - gap;
    }
    y = Math.max(8, y);

    // Left-align to the anchor, clamped so the card never runs off
    // either horizontal edge.
    let x = a.left;
    x = Math.max(8, Math.min(x, window.innerWidth - c.width - 8));

    coords = { x, y };
  }

  // Recompute position whenever the card's content (and thus size)
  // changes — loading vs. loaded vs. "not available" are different
  // heights.
  $effect(() => {
    cached;
    mod;
    positioned = false;
    (async () => {
      await tick();
      await computePosition();
      positioned = true;
    })();
  });

  // Reposition on scroll/resize like Tooltip does, but hide instead of
  // drifting stale — the anchor may have moved out from under fixed
  // coords entirely.
  $effect(() => {
    const onScroll = () => onLeave();
    window.addEventListener("scroll", onScroll, true);
    window.addEventListener("resize", onScroll);
    return () => {
      window.removeEventListener("scroll", onScroll, true);
      window.removeEventListener("resize", onScroll);
    };
  });
</script>

<!-- svelte-ignore a11y_no_static_element_interactions -->
<div
  bind:this={cardEl}
  role="tooltip"
  class="fixed z-[1000] w-[270px] rounded-lg border border-[var(--border)]
         bg-[var(--surface)] shadow-[0_8px_24px_rgba(0,0,0,0.22)]
         px-3 py-2.5 text-left transition-opacity duration-100
         {positioned ? 'opacity-100' : 'opacity-0'}"
  style="left: {coords.x}px; top: {coords.y}px;"
  onmouseenter={onEnter}
  onmouseleave={onLeave}
>
  {#if cached === null}
    <div class="flex items-center gap-2 text-body-sm text-[var(--text-faint)]">
      <span class="hc-spinner" aria-hidden="true"></span> Loading…
    </div>
  {:else if cached.status === "unavailable"}
    <!-- Quiet, not an error: covers both a deleted issue (404) and one
         the current viewer just doesn't have access to (403) — see
         references.ts's CachedIssue. -->
    <p class="text-body-sm text-[var(--text-faint)] italic m-0">
      {identifier} isn't available
    </p>
  {:else}
    {@const issue = cached.issue}
    <div class="flex items-center gap-1.5">
      <StatusIcon status={issue.status} size={14} />
      <span class="font-mono text-micro text-[var(--text-faint)]">{issue.identifier}</span>
      {#if issue.priority !== "none"}
        <span class="ml-auto shrink-0"><PriorityIcon priority={issue.priority} size={13} /></span>
      {/if}
    </div>
    <p class="text-body-sm text-[var(--text)] leading-snug line-clamp-2 mt-1 mb-0">
      {issue.title}
    </p>
    {#if showDescription}
      {@const preview = stripMarkdown(issue.description)}
      {#if preview}
        <p
          class="text-caption text-[var(--text-muted)] leading-snug line-clamp-5
                 mt-1.5 mb-0 pt-1.5 border-t border-[var(--border)]"
        >
          {preview}
        </p>
      {:else}
        <p class="text-caption text-[var(--text-faint)] italic mt-1.5 mb-0 pt-1.5 border-t border-[var(--border)]">
          No description
        </p>
      {/if}
    {/if}
    {#if mod}
      <div class="mt-1.5 flex items-center gap-1 text-micro text-[var(--text-muted)]">
        {#if mod.emoji}
          <ProjectIcon value={mod.emoji} size={11} />
        {:else}
          <Layers size={10} class="text-[var(--text-faint)] shrink-0" />
        {/if}
        <span class="truncate">{mod.name}</span>
      </div>
    {/if}
  {/if}
</div>

<style>
  .hc-spinner {
    width: 12px;
    height: 12px;
    border-radius: 999px;
    border: 2px solid var(--border);
    border-top-color: var(--accent);
    animation: hc-spin 0.6s linear infinite;
  }
  @keyframes hc-spin {
    to {
      transform: rotate(360deg);
    }
  }
  @media (prefers-reduced-motion: reduce) {
    .hc-spinner {
      animation-duration: 1.4s;
    }
  }
</style>

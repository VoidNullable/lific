<script lang="ts" module>
  import type { Component } from "svelte";

  /** One crumb in the trail. The last segment is treated as the current
   *  page: rendered non-linked and in a slightly stronger color. Linked
   *  segments carry an `href` (hash route like "#/LIF/issues"). */
  export interface Crumb {
    label: string;
    /** Hash route, e.g. "#/LIF/issues". Omit for the current (last) crumb. */
    href?: string;
    /** Optional leading icon (a lucide-svelte component). */
    icon?: Component<{ size?: number | string; class?: string }> | undefined;
    /** Render the label in the monospace face (identifiers: LIF, LIF-42). */
    mono?: boolean;
    /** Collapse this crumb (and its leading separator) below the `sm`
     *  breakpoint — matches Topbar hiding the project scope on phones,
     *  where the app header already shows the project. */
    hideBelowSm?: boolean;
  }
</script>

<script lang="ts">
  // LIF-286 — shared breadcrumb trail. Extracted from the hand-rolled
  // `PROJ › Issues/Board` crumb in lib/issues/Topbar.svelte so every detail
  // route (issue / page / module / plan) shows the same trail with the same
  // typography, colors, and truncation behavior.
  //
  // Visual reference is Topbar's crumb: project segment is muted + hover,
  // the `›` separators are faint, the current page reads in --text. Linked
  // segments navigate via plain hash hrefs (no navigate() dependency), so
  // this stays a pure presentational component.

  import { ChevronRight } from "lucide-svelte";

  let { segments }: { segments: Crumb[] } = $props();

  // A separator belongs between two VISIBLE crumbs. Keying it off its own
  // segment's `hideBelowSm` alone left a dangling leading "›" below sm
  // whenever the first crumb was collapsed — visible in every detail
  // topbar as `› ALP-1`, and costing ~20px in a row that had none to
  // spare. So a separator also hides below sm when every crumb before it
  // does: at that point it is not separating anything.
  let sepHiddenBelowSm = $derived(
    segments.map((seg, i) =>
      Boolean(seg.hideBelowSm) || segments.slice(0, i).every((s) => s.hideBelowSm),
    ),
  );
</script>

<nav aria-label="Breadcrumb" class="min-w-0">
  <ol class="flex items-center gap-1.5 min-w-0">
    {#each segments as seg, i (i)}
      {@const isLast = i === segments.length - 1}
      {#if i > 0}
        <li
          aria-hidden="true"
          class="shrink-0 flex items-center {sepHiddenBelowSm[i] ? 'hidden sm:flex' : ''}"
        >
          <ChevronRight size={12} class="text-[var(--text-faint)]" />
        </li>
      {/if}
      <li class="min-w-0 flex items-center {seg.hideBelowSm ? 'hidden sm:flex' : ''}">
        {#if seg.href && !isLast}
          <a
            href={seg.href}
            title={seg.label}
            class="flex items-center gap-1.5 min-w-0 text-body-sm font-medium
                   text-[var(--text-muted)] hover:text-[var(--text)]
                   transition-colors {seg.mono ? 'font-mono' : ''}"
          >
            {#if seg.icon}
              {@const Icon = seg.icon}
              <Icon size={13} class="shrink-0" />
            {/if}
            <span class="truncate max-w-[9rem] sm:max-w-[14rem]">{seg.label}</span>
          </a>
        {:else}
          <span
            title={seg.label}
            aria-current={isLast ? "page" : undefined}
            class="flex items-center gap-1.5 min-w-0 text-body-sm font-medium
                   text-[var(--text)] {seg.mono ? 'font-mono' : ''}"
          >
            {#if seg.icon}
              {@const Icon = seg.icon}
              <Icon size={13} class="shrink-0" />
            {/if}
            <span class="truncate max-w-[9rem] sm:max-w-[14rem]">{seg.label}</span>
          </span>
        {/if}
      </li>
    {/each}
  </ol>
</nav>

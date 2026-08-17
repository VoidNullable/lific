<script lang="ts">
  // LIF-418: one node of the collapsible JSON tree.
  //
  // Recursive, and deliberately lazy: a collapsed object renders none of its
  // children, so a 20 MB API dump costs one row until someone opens it. Only
  // the first CHILD_BUDGET children of any container render at once, with a
  // "show the rest" row after them, which keeps a 50k-element array from
  // freezing the tab the moment it is expanded.

  import { ChevronRight } from "lucide-svelte";
  import JsonNode from "./JsonNode.svelte";

  let {
    label = null,
    value,
    depth = 0,
    open: initialOpen = false,
  }: {
    label?: string | null;
    value: unknown;
    depth?: number;
    open?: boolean;
  } = $props();

  const CHILD_BUDGET = 100;

  // `null` means "never toggled", so the node follows whatever the parent
  // asked for; a click pins it either way.
  let toggled = $state<boolean | null>(null);
  const open = $derived(toggled ?? initialOpen);
  let shown = $state(CHILD_BUDGET);

  const isArray = $derived(Array.isArray(value));
  const isObject = $derived(
    value !== null && typeof value === "object" && !Array.isArray(value),
  );
  const container = $derived(isArray || isObject);

  const entries = $derived.by<[string, unknown][]>(() => {
    if (!open) return []; // lazy: nothing is built while collapsed
    if (isArray) return (value as unknown[]).map((v, i) => [String(i), v]);
    if (isObject) return Object.entries(value as Record<string, unknown>);
    return [];
  });

  const childCount = $derived.by(() => {
    if (isArray) return (value as unknown[]).length;
    if (isObject) return Object.keys(value as Record<string, unknown>).length;
    return 0;
  });

  /** Collapsed summary: `{3 keys}` / `[12 items]`. */
  const summary = $derived(
    isArray
      ? `[${childCount} ${childCount === 1 ? "item" : "items"}]`
      : `{${childCount} ${childCount === 1 ? "key" : "keys"}}`,
  );

  function scalarText(v: unknown): string {
    if (typeof v === "string") return JSON.stringify(v);
    if (v === null) return "null";
    return String(v);
  }

  function scalarClass(v: unknown): string {
    if (typeof v === "string") return "jn__str";
    if (typeof v === "number") return "jn__num";
    if (typeof v === "boolean") return "jn__bool";
    return "jn__null";
  }
</script>

<div class="jn" style="--jn-depth: {depth}">
  {#if container}
    <button
      type="button"
      class="jn__row jn__row--toggle"
      aria-expanded={open}
      onclick={() => (toggled = !open)}
    >
      <span class="jn__caret" class:jn__caret--open={open}><ChevronRight size={12} /></span>
      {#if label !== null}<span class="jn__key">{label}</span><span class="jn__colon">:</span>{/if}
      <span class="jn__summary">{summary}</span>
    </button>
    {#if open}
      {#each entries.slice(0, shown) as [key, child] (key)}
        <JsonNode label={key} value={child} depth={depth + 1} />
      {/each}
      {#if entries.length > shown}
        <button
          type="button"
          class="jn__row jn__more"
          onclick={() => (shown += CHILD_BUDGET)}
        >
          Show {Math.min(CHILD_BUDGET, entries.length - shown)} more of {entries.length}
        </button>
      {/if}
    {/if}
  {:else}
    <div class="jn__row">
      <span class="jn__caret"></span>
      {#if label !== null}<span class="jn__key">{label}</span><span class="jn__colon">:</span>{/if}
      <span class={scalarClass(value)}>{scalarText(value)}</span>
    </div>
  {/if}
</div>

<style>
  .jn {
    font-family: var(--font-mono);
    font-size: var(--text-caption);
    line-height: 1.6;
  }
  .jn__row {
    display: flex;
    align-items: baseline;
    gap: 0.25rem;
    width: 100%;
    padding: 0 0.5rem 0 calc(0.5rem + var(--jn-depth) * 0.875rem);
    border: 0;
    background: transparent;
    color: var(--text);
    font: inherit;
    text-align: left;
  }
  .jn__row--toggle {
    cursor: pointer;
  }
  .jn__row--toggle:hover {
    background: var(--bg-subtle);
  }
  .jn__caret {
    display: inline-flex;
    align-items: center;
    width: 0.875rem;
    flex-shrink: 0;
    color: var(--text-faint);
    transition: transform 0.12s var(--ease-out-expo);
  }
  .jn__caret--open {
    transform: rotate(90deg);
  }
  .jn__key {
    color: var(--text-muted);
  }
  .jn__colon {
    color: var(--text-faint);
    margin-right: 0.125rem;
  }
  .jn__summary {
    color: var(--text-faint);
  }
  .jn__str {
    color: var(--ansi-green);
    word-break: break-word;
  }
  .jn__num {
    color: var(--ansi-blue);
  }
  .jn__bool {
    color: var(--ansi-magenta);
  }
  .jn__null {
    color: var(--text-faint);
  }
  .jn__more {
    color: var(--accent);
    cursor: pointer;
  }
  @media (prefers-reduced-motion: reduce) {
    .jn__caret {
      transition: none;
    }
  }
</style>

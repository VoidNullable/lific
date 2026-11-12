<script lang="ts">
  // LIF-543: newest agent actions in the window, as a vertical timeline.
  // Each item links to its entity through the hash route and navLink, so
  // modified clicks still open a new tab; gone entities render unlinked.

  import { FileText, Pencil, Plus, Check, Eye } from "lucide-svelte";
  import type { AgentActionKind, AgentRecentItem } from "../api";
  import { now } from "../now.svelte";
  import { navLink } from "../navLink";
  import { SERIES, recentVerb, shortAgo } from "./activity";

  let {
    items,
    navigate,
  }: { items: AgentRecentItem[]; navigate: (path: string) => void } = $props();

  const ICON = { edit: Pencil, create: Plus, close: Check } as const;
  const COLOR = Object.fromEntries(SERIES.map((s) => [s.key, s.color])) as Record<AgentActionKind, string>;

  function iconFor(item: AgentRecentItem) {
    if (item.kind === "read") return item.entity_type === "issue" ? Eye : FileText;
    return ICON[item.kind];
  }
</script>

{#snippet body(item: AgentRecentItem)}
  {@const Icon = iconFor(item)}
  <span
    class="relative z-[1] size-8 shrink-0 rounded-lg bg-[var(--aa-tile)] flex items-center justify-center"
    style="color: {item.kind === 'read' ? 'var(--aa-text-2)' : COLOR[item.kind]}"
  >
    <Icon size={16} strokeWidth={2} />
  </span>
  <span class="min-w-0 flex-1 leading-snug pt-0.5">
    <span class="block text-[0.95rem] text-[var(--aa-text)]">
      <span class="font-semibold">{item.agent.display_name}</span>
      {recentVerb(item)}
    </span>
    <span class="block text-[0.95rem] text-[var(--aa-text)] break-words">
      {#if item.identifier}
        <span class="font-medium text-[var(--aa-link)] mr-1">{item.identifier}</span>
      {/if}
      {item.title ?? (item.identifier ? "" : "Untitled")}
    </span>
    <time datetime={item.at} class="block text-[0.85rem] text-[var(--aa-text-2)] mt-1" title={new Date(item.at).toLocaleString()}>
      {shortAgo(item.at, now())}
    </time>
  </span>
{/snippet}

<ol class="relative flex flex-col">
  {#each items as item, i (`${item.at}:${item.kind}:${item.entity_type}:${item.identifier ?? item.title}:${i}`)}
    <li class="relative">
      {#if i < items.length - 1}
        <span aria-hidden="true" class="absolute left-4 top-[2.875rem] -bottom-0.5 w-px bg-[var(--aa-line)]"></span>
      {/if}
      {#if item.route}
        <a
          href="#{item.route}"
          use:navLink={navigate}
          class="flex items-start gap-3.5 px-2 py-2.5 -mx-2 rounded-lg hover:bg-[var(--aa-hover)] transition-colors"
        >
          {@render body(item)}
        </a>
      {:else}
        <div class="flex items-start gap-3.5 px-2 py-2.5 -mx-2">
          {@render body(item)}
        </div>
      {/if}
    </li>
  {/each}
</ol>

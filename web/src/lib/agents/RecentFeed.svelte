<script lang="ts">
  // LIF-543: newest agent actions in the window, as a vertical timeline.
  // Each item links to its entity through the hash route and navLink, so
  // modified clicks still open a new tab; gone entities render unlinked.

  import { BookOpen, Pencil, Plus, Check } from "lucide-svelte";
  import type { AgentActionKind, AgentRecentItem } from "../api";
  import TimeAgo from "../TimeAgo.svelte";
  import { navLink } from "../navLink";
  import { SERIES, recentVerb } from "./activity";

  let {
    items,
    navigate,
  }: { items: AgentRecentItem[]; navigate: (path: string) => void } = $props();

  const ICON = { read: BookOpen, edit: Pencil, create: Plus, close: Check } as const;
  const COLOR = Object.fromEntries(SERIES.map((s) => [s.key, s.color])) as Record<AgentActionKind, string>;
</script>

{#snippet body(item: AgentRecentItem)}
  {@const Icon = ICON[item.kind]}
  <span
    class="relative z-[1] size-7 shrink-0 rounded-lg border border-[var(--border)] bg-[var(--surface)]
           flex items-center justify-center"
    style="color: {COLOR[item.kind]}"
  >
    <Icon size={14} strokeWidth={2.25} />
  </span>
  <span class="min-w-0 flex-1 text-body-sm leading-snug">
    <span class="block text-[var(--text-muted)]">
      <span class="font-semibold text-[var(--text)]">{item.agent.display_name}</span>
      {recentVerb(item)}
    </span>
    <span class="block text-[var(--text)] break-words">
      {#if item.identifier}
        <span class="font-mono text-caption text-[var(--accent)] mr-1">{item.identifier}</span>
      {/if}
      {item.title ?? (item.identifier ? "" : "Untitled")}
    </span>
    <TimeAgo date={new Date(item.at)} class="block text-micro text-[var(--text-faint)] mt-0.5" />
  </span>
{/snippet}

<ol class="relative flex flex-col">
  {#each items as item, i (`${item.at}:${item.kind}:${item.entity_type}:${item.identifier ?? item.title}:${i}`)}
    <li class="relative">
      {#if i < items.length - 1}
        <span aria-hidden="true" class="absolute left-[1.375rem] top-9 bottom-0 w-px bg-[var(--border)]"></span>
      {/if}
      {#if item.route}
        <a
          href="#{item.route}"
          use:navLink={navigate}
          class="flex items-start gap-3 px-2 py-2 rounded-md hover:bg-[var(--bg-subtle)] transition-colors"
        >
          {@render body(item)}
        </a>
      {:else}
        <div class="flex items-start gap-3 px-2 py-2">
          {@render body(item)}
        </div>
      {/if}
    </li>
  {/each}
</ol>

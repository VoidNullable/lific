<script lang="ts">
  // LIF-543: the agent roster. One row per bot: a generic Bot glyph (never a
  // vendor logo), name and owner, status pill, a sparkline of the window,
  // and the four action counts.

  import { Bot } from "lucide-svelte";
  import type { AgentSummary, AgentWindow } from "../api";
  import { now } from "../now.svelte";
  import Sparkline from "./Sparkline.svelte";
  import {
    SERIES,
    STATUS_LABEL,
    WINDOW_PHRASE,
    countNoun,
    possessive,
    statusDetail,
  } from "./activity";

  let { agents, win }: { agents: AgentSummary[]; win: AgentWindow } = $props();

  const DOT: Record<AgentSummary["status"], string> = {
    active: "var(--success)",
    idle: "var(--warn)",
    inactive: "var(--text-faint)",
  };
</script>

<ul class="flex flex-col">
  {#each agents as agent (agent.user_id)}
    {@const name = agent.display_name || agent.username}
    {@const owner = agent.owner ? agent.owner.display_name || agent.owner.username : null}
    {@const detail = statusDetail(agent.status, agent.last_seen, now())}
    <li
      class="flex flex-wrap items-center gap-x-5 gap-y-3 px-4 py-3.5 border-b border-[var(--border)] last:border-b-0"
      data-agent={agent.username}
    >
      <div class="flex items-center gap-3 min-w-0 flex-1 basis-[220px]">
        <span
          class="size-9 shrink-0 rounded-lg bg-[var(--bg-subtle)] flex items-center justify-center
                 {agent.status === 'active' ? 'text-[var(--text)]' : 'text-[var(--text-muted)]'}"
        >
          <Bot size={18} />
        </span>
        <div class="min-w-0">
          <p class="text-body-sm leading-tight truncate">
            <span class="font-semibold text-[var(--text)]" title={agent.username}>{name}</span>
            {#if owner}
              <span class="text-[var(--text-faint)] ml-1">{possessive(owner)}</span>
            {/if}
          </p>
          <div class="flex flex-wrap items-center gap-x-2 gap-y-0.5 mt-1">
            <span
              class="inline-flex items-center gap-1.5 px-2 py-0.5 rounded-full text-micro font-medium
                     text-[var(--text)] bg-[var(--bg-subtle)]"
            >
              <span class="size-1.5 rounded-full shrink-0" style="background: {DOT[agent.status]}"></span>
              {STATUS_LABEL[agent.status]}
            </span>
            {#if detail}
              <span class="text-micro text-[var(--text-faint)]">{detail}</span>
            {/if}
          </div>
        </div>
      </div>

      <Sparkline
        series={agent.series}
        label="{name}: actions over {WINDOW_PHRASE[win]}"
        class="w-28 h-8 shrink-0 hidden sm:block"
      />

      <dl class="grid grid-cols-4 shrink-0 divide-x divide-[var(--border)]">
        {#each SERIES as s (s.key)}
          {@const v = agent.counts[s.key]}
          <div class="flex flex-col-reverse px-3 first:pl-0 last:pr-0 min-w-[3.75rem]">
            <dt class="text-micro text-[var(--text-faint)]">{countNoun(s.key, v)}</dt>
            <dd
              class="text-body-sm font-semibold tabular-nums leading-tight
                     {v === 0 ? 'text-[var(--text-faint)] font-normal' : 'text-[var(--text)]'}"
            >
              {v}
            </dd>
          </div>
        {/each}
      </dl>
    </li>
  {/each}
</ul>

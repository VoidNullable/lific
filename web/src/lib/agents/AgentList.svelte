<script lang="ts">
  // LIF-543: the agent roster. One row per bot: a generic Bot glyph (never a
  // vendor logo), name and owner, status pill, a sparkline of the window,
  // and the four action counts, all on one line at desktop widths.

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
    active: "var(--aa-active)",
    idle: "var(--aa-idle)",
    inactive: "var(--aa-text-3)",
  };
</script>

<ul class="flex flex-col border-t border-[var(--aa-line)]">
  {#each agents as agent (agent.user_id)}
    {@const name = agent.display_name || agent.username}
    {@const owner = agent.owner ? agent.owner.display_name || agent.owner.username : null}
    {@const detail = statusDetail(agent.status, agent.last_seen, now())}
    <li class="agent-row border-b border-[var(--aa-line)] py-4" data-agent={agent.username}>
      <span
        class="size-11 rounded-xl bg-[var(--aa-tile)] flex items-center justify-center
               {agent.status === 'inactive' ? 'text-[var(--aa-text-2)]' : 'text-[var(--aa-text)]'}"
        style="grid-area: icon"
      >
        <Bot size={22} strokeWidth={1.75} />
      </span>

      <div class="min-w-0" style="grid-area: who">
        <p class="truncate leading-tight">
          <span class="text-[1rem] font-semibold text-[var(--aa-text)]" title={agent.username}>{name}</span>
          {#if owner}
            <span class="text-[0.85rem] text-[var(--aa-text-3)] ml-1">{possessive(owner)}</span>
          {/if}
        </p>
        <div class="flex flex-col items-start gap-1 mt-1.5">
          <span
            class="inline-flex items-center gap-1.5 pl-2 pr-2.5 py-[3px] rounded-full text-[0.8rem] font-medium
                   text-[var(--aa-text)] bg-[var(--aa-tile)]"
          >
            <span class="size-2 rounded-full shrink-0" style="background: {DOT[agent.status]}"></span>
            {STATUS_LABEL[agent.status]}
          </span>
          {#if detail}
            <span class="text-[0.85rem] text-[var(--aa-text-2)] pl-1">{detail}</span>
          {/if}
        </div>
      </div>

      <div class="spark" style="grid-area: spark">
        <Sparkline series={agent.series} label="{name}: actions over {WINDOW_PHRASE[win]}" class="w-full h-9" />
      </div>

      <dl class="grid grid-cols-4" style="grid-area: counts">
        {#each SERIES as s, i (s.key)}
          {@const v = agent.counts[s.key]}
          <div class="flex flex-col-reverse justify-end pl-3.5 {i > 0 ? 'border-l border-[var(--aa-line)]' : ''}">
            <dt class="text-[0.78rem] text-[var(--aa-text-2)] mt-0.5">{countNoun(s.key, v)}</dt>
            <dd class="text-[1.05rem] font-semibold tabular-nums leading-tight text-[var(--aa-text)] h-[1.3em] flex items-center">
              {#if v === 0}
                <span class="block w-2 h-[1.5px] rounded-full bg-[var(--aa-text-2)]" aria-label="none"></span>
              {:else}
                {v}
              {/if}
            </dd>
          </div>
        {/each}
      </dl>
    </li>
  {/each}
</ul>

<style>
  ul {
    container-type: inline-size;
  }
  .agent-row {
    display: grid;
    grid-template-columns: 2.75rem minmax(9rem, 1fr) minmax(5rem, 9rem) 18.5rem;
    grid-template-areas: "icon who spark counts";
    align-items: center;
    column-gap: 1.25rem;
  }
  /* Too narrow for one line: the counts drop under the name. */
  @container (max-width: 660px) {
    .agent-row {
      grid-template-columns: 2.75rem minmax(0, 1fr) minmax(4rem, 8rem);
      grid-template-areas:
        "icon who spark"
        ". counts counts";
      row-gap: 0.75rem;
    }
  }
</style>

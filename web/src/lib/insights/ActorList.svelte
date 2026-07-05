<script lang="ts">
  // LIF-240 — top-actors list for the Insights tab. Visually related to
  // ProjectActivity's actor rail (same initials-avatar + relative-volume
  // bar language) but simpler: no click-to-filter, just a ranked readout
  // scoped to the selected weeks window.

  import type { ActorStat } from "../api";
  import TimeAgo from "../TimeAgo.svelte";

  let { actors }: { actors: ActorStat[] } = $props();

  let maxActions = $derived(Math.max(1, ...actors.map((a) => a.actions)));

  function name(a: ActorStat): string {
    return a.display_name || a.username || "system";
  }

  function initials(n: string): string {
    return n
      .split(/[\s_-]+/)
      .slice(0, 2)
      .map((w) => w[0]?.toUpperCase() ?? "")
      .join("");
  }
</script>

{#if actors.length === 0}
  <p class="text-body-sm text-[var(--text-faint)] py-2">No activity in this window</p>
{:else}
  <div class="flex flex-col gap-1">
    {#each actors as a (a.actor_user_id)}
      {@const n = name(a)}
      <div class="relative flex items-center gap-2.5 px-1 py-1.5 rounded-md overflow-hidden">
        <span
          class="size-6 rounded-full flex items-center justify-center text-micro font-bold
                 shrink-0 select-none
                 {a.is_bot
            ? 'bg-[var(--accent-subtle)] text-[var(--accent)] border border-[var(--accent)]'
            : 'bg-[var(--accent)] text-[var(--accent-text)]'}"
        >
          {initials(n)}
        </span>
        <div class="flex-1 min-w-0">
          <div class="flex items-center gap-1.5">
            <span class="text-body-sm text-[var(--text)] truncate font-medium">{n}</span>
            {#if a.is_bot}
              <span
                class="text-micro font-semibold uppercase tracking-wider px-1 py-px rounded
                       bg-[var(--accent-subtle)] text-[var(--accent)] shrink-0"
              >
                agent
              </span>
            {/if}
          </div>
          <div class="text-micro text-[var(--text-faint)]">
            last seen <TimeAgo date={a.last_ts} />
          </div>
        </div>
        <span class="text-caption text-[var(--text-muted)] tabular-nums shrink-0">
          {a.actions.toLocaleString()}
        </span>
        <span
          class="absolute bottom-0 left-1 h-[2px] rounded-full bg-[var(--accent)] opacity-30"
          style="width: calc({Math.max(4, (a.actions / maxActions) * 100)}% - 0.5rem)"
          aria-hidden="true"
        ></span>
      </div>
    {/each}
  </div>
{/if}

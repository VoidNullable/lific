<script lang="ts">
  // LIF-485: the compact "waiting" indicator on list rows and board cards.
  // Names who or when for the most pressing wait (overdue, then blocking,
  // then due); every wait is in the tooltip. Overdue reads as an error so it
  // stands out in a column of ordinary waits.
  import type { IssueWait } from "../api";
  import { Hourglass, CalendarClock, CalendarCheck, CircleAlert } from "lucide-svelte";
  import Tooltip from "../Tooltip.svelte";
  import { now } from "../now.svelte";
  import { localDay, summarizeWaits } from "./waits";

  let { waits }: { waits: IssueWait[] | undefined } = $props();

  // Rides the shared clock so a card left open across midnight moves from
  // "until" to "due" without a refetch.
  const summary = $derived(summarizeWaits(waits, localDay(new Date(now()))));

  const TONE = {
    holding: "text-[var(--warn-text)] bg-[color-mix(in_srgb,var(--warn)_14%,transparent)]",
    due: "text-[var(--accent)] bg-[var(--accent-subtle)]",
    overdue: "text-[var(--error)] bg-[var(--error-bg)] font-semibold",
  } as const;
</script>

{#if summary}
  <Tooltip content={summary.title}>
    <span
      class="wait-chip inline-flex items-center gap-1 max-w-[96px] sm:max-w-[140px] shrink-0
             text-micro font-medium px-1.5 py-0.5 rounded-full {TONE[summary.state]}"
      data-wait-state={summary.state}
      aria-label={summary.title}
    >
      {#if summary.state === "overdue"}
        <CircleAlert size={11} class="shrink-0" />
      {:else if summary.wait.kind === "user"}
        <Hourglass size={11} class="shrink-0" />
      {:else if summary.state === "due"}
        <CalendarCheck size={11} class="shrink-0" />
      {:else}
        <CalendarClock size={11} class="shrink-0" />
      {/if}
      <span class="truncate">{summary.label}</span>
      {#if summary.more > 0}
        <span class="shrink-0 opacity-70">+{summary.more}</span>
      {/if}
    </span>
  </Tooltip>
{/if}

<script lang="ts">
  // Who last edited a page: initials for a person, the tool's mark for an
  // agent account.
  import { Bot } from "lucide-svelte";
  import type { PageMetaPerson } from "../api";
  import ToolIcon from "../ToolIcon.svelte";
  import Tooltip from "../Tooltip.svelte";
  import { initials } from "../issues/assignees";
  import { personLabel } from "./list";

  let {
    person,
    size = "xs",
    tip = null,
  }: {
    person: PageMetaPerson;
    size?: "xs" | "sm";
    /** Tooltip text; null shows none. */
    tip?: string | null;
  } = $props();

  const box = $derived(size === "xs" ? "size-[18px]" : "size-6");
</script>

<Tooltip content={tip}>
  {#if person.is_bot}
    <span
      class="{box} shrink-0 grid place-items-center rounded-md bg-[var(--bg-subtle)] text-[var(--text-muted)]
             ring-1 ring-[var(--border)]"
      role="img"
      aria-label={personLabel(person)}
      data-editor-kind="agent"
    >
      <ToolIcon tool={person.tool_id ?? person.username} size={size === "xs" ? 11 : 14} fallback={Bot} />
    </span>
  {:else}
    <span
      class="{box} {size === 'xs' ? 'text-[8px]' : 'text-micro'} shrink-0 rounded-full grid place-items-center
             font-semibold tracking-wide select-none leading-none bg-[var(--accent)] text-[var(--accent-text)]"
      role="img"
      aria-label={personLabel(person)}
      data-editor-kind="person"
    >
      {initials(personLabel(person))}
    </span>
  {/if}
</Tooltip>

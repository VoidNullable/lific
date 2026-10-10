<script lang="ts">
  // LIF-147: compact "a person must do this" marker for list rows and board
  // cards. Named people show as stacked initials (two at most, then +N);
  // "any person" shows a person icon. Unassigned issues, which any agent
  // may take, show nothing. Fixed small width so a row's title keeps its
  // room on a phone (LIF-229).
  import { User } from "lucide-svelte";
  import type { Issue } from "../api";
  import Tooltip from "../Tooltip.svelte";
  import PersonAvatar from "./PersonAvatar.svelte";
  import { assignmentKind, describeAssignment } from "./assignees";

  let { issue }: { issue: Pick<Issue, "needs_human" | "assignees"> } = $props();

  const kind = $derived(assignmentKind(issue));
  const people = $derived(issue.assignees ?? []);
  const title = $derived(
    kind === "human" ? "For any person" : `Assigned to ${describeAssignment(issue)}`,
  );
</script>

{#if kind !== "agents"}
  <Tooltip content={title}>
    <span
      class="assignee-marker inline-flex items-center shrink-0"
      data-assignment={kind}
      aria-label={title}
      role="img"
    >
      {#if kind === "human"}
        <span
          class="size-[18px] rounded-full grid place-items-center
                 text-[var(--accent)] bg-[var(--accent-subtle)]"
        >
          <User size={11} />
        </span>
      {:else}
        <span class="inline-flex items-center -space-x-1">
          {#each people.slice(0, 2) as person (person.user_id)}
            <PersonAvatar {person} size="xs" ring />
          {/each}
        </span>
        {#if people.length > 2}
          <span class="ml-0.5 text-micro text-[var(--text-faint)] tabular-nums">+{people.length - 2}</span>
        {/if}
      {/if}
    </span>
  </Tooltip>
{/if}

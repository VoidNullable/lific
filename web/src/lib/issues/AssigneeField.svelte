<script lang="ts">
  // LIF-147: the issue sidebar's "Assignee" field. Shows who the issue is
  // for (any agent, any person, or named people); editors click it to open
  // the AssigneePicker. Viewers get the same text without the control.
  import { Bot, User } from "lucide-svelte";
  import type { Issue } from "../api";
  import AssigneePicker from "./AssigneePicker.svelte";
  import PersonAvatar from "./PersonAvatar.svelte";
  import { assignmentKind, assignmentNames, personName, type Person } from "./assignees";

  let {
    issue,
    editable,
    onApply,
    onOpen,
  }: {
    issue: Issue;
    editable: boolean;
    onApply: (names: string[], people: Person[]) => void;
    /** Lets the sidebar close its other dropdowns. */
    onOpen?: () => void;
  } = $props();

  let pickerOpen = $state(false);
  const kind = $derived(assignmentKind(issue));
</script>

<div class="issue-meta-field" data-testid="issue-assignee" data-assignment={kind}>
  <p class="issue-meta-field-label">Assignee</p>
  <button
    type="button"
    class="flex flex-col items-stretch gap-1 text-body-sm rounded-md px-2 py-1 -mx-2
           transition-colors w-full text-left
           {editable ? 'hover:bg-[var(--bg-subtle)] cursor-pointer' : 'cursor-default'}"
    title={editable ? "Change assignee" : undefined}
    aria-haspopup={editable ? "dialog" : undefined}
    disabled={!editable}
    onclick={(e) => {
      if (!editable) return;
      e.stopPropagation();
      onOpen?.();
      pickerOpen = true;
    }}
  >
    {#if kind === "people"}
      {#each issue.assignees ?? [] as person (person.user_id)}
        <span class="flex items-center gap-2 min-w-0">
          <PersonAvatar {person} size="xs" />
          <span class="text-[var(--text)] truncate">{personName(person)}</span>
        </span>
      {/each}
    {:else if kind === "human"}
      <span class="flex items-center gap-2">
        <User size={14} class="shrink-0 text-[var(--accent)]" />
        <span class="text-[var(--text)]">Any person</span>
      </span>
    {:else}
      <span class="flex items-center gap-2">
        <Bot size={14} class="shrink-0 text-[var(--text-faint)]" />
        <span class="text-[var(--text-faint)]">Any agent</span>
      </span>
    {/if}
  </button>
</div>

{#if editable}
  <AssigneePicker
    bind:open={pickerOpen}
    projectId={issue.project_id}
    subject={issue.identifier}
    current={assignmentNames(issue)}
    {onApply}
  />
{/if}

<script lang="ts">
  // One issue in a cross-project workspace list (Inbox, Assigned to me).
  // A real link, so new-tab clicks keep working; a plain click navigates,
  // shift-click previews in the peek panel, press-and-hold previews on touch.
  import type { Issue } from "../api";
  import StatusIcon from "../StatusIcon.svelte";
  import PriorityIcon from "../PriorityIcon.svelte";
  import TimeAgo from "../TimeAgo.svelte";
  import { openPeek } from "../issues/peek.svelte";
  import { longpress } from "../actions/longpress";
  import { projectCodeOf } from "../references";

  let {
    issue,
    projectName,
    isNew = false,
    navigate,
  }: {
    issue: Issue;
    /** Shown on the right; falls back to the identifier's project code. */
    projectName?: string;
    /** Updated since the previous visit: draws the accent dot. */
    isNew?: boolean;
    navigate: (path: string) => void;
  } = $props();

  const path = $derived(`/${projectCodeOf(issue.identifier)}/issues/${issue.identifier}`);

  function onclick(e: MouseEvent) {
    if (e.button !== 0) return;
    if (e.shiftKey) {
      e.preventDefault();
      openPeek(issue.identifier);
      return;
    }
    if (e.metaKey || e.ctrlKey || e.altKey) return;
    e.preventDefault();
    navigate(path);
  }
</script>

<a
  href={`#${path}`}
  class="w-full flex items-center gap-2.5 pl-2 pr-4 py-2 text-left
         border-b border-[var(--border)] last:border-b-0
         hover:bg-[var(--bg-subtle)] hover:no-underline transition-colors
         focus-visible:outline-2 focus-visible:outline-[var(--accent)] focus-visible:-outline-offset-2"
  data-identifier={issue.identifier}
  use:longpress={{ onLongPress: () => openPeek(issue.identifier) }}
  {onclick}
  onmousedown={(e) => {
    if (e.shiftKey) e.preventDefault();
  }}
>
  <span class="w-1.5 shrink-0 flex items-center justify-center">
    {#if isNew}
      <span class="block size-1.5 rounded-full bg-[var(--accent)]" title="Updated since your last visit"></span>
      <span class="sr-only">Updated since your last visit</span>
    {/if}
  </span>
  <span class="shrink-0 flex" title={issue.status[0].toUpperCase() + issue.status.slice(1)}>
    <StatusIcon status={issue.status} size={14} />
  </span>
  <span class="text-caption font-mono text-[var(--text-faint)] w-[52px] sm:w-[72px] shrink-0 truncate">
    {issue.identifier}
  </span>
  <span class="text-body-sm text-[var(--text)] truncate flex-1 min-w-0">{issue.title}</span>
  <span class="hidden sm:block shrink-0 max-w-[160px] truncate text-caption text-[var(--text-faint)]">
    {projectName ?? projectCodeOf(issue.identifier)}
  </span>
  <span class="shrink-0 w-[15px] flex justify-center" title={`Priority: ${issue.priority}`}>
    <PriorityIcon priority={issue.priority} size={15} />
  </span>
  <TimeAgo
    date={issue.updated_at.replace(" ", "T")}
    class="shrink-0 w-[52px] text-right text-micro tabular-nums text-[var(--text-faint)]"
  />
</a>

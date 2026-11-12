<script lang="ts">
  // A collapsible, counted group of workspace issue rows.
  import type { Snippet } from "svelte";
  import { ChevronRight } from "lucide-svelte";
  import type { Issue } from "../api";
  import WorkspaceIssueRow from "./WorkspaceIssueRow.svelte";

  let {
    key,
    label,
    issues,
    icon,
    collapsed = false,
    onToggle,
    projectName,
    isNew,
    navigate,
    footer,
  }: {
    key: string;
    label: string;
    issues: Issue[];
    /** Leading icon for the group header. */
    icon: Snippet;
    collapsed?: boolean;
    onToggle: () => void;
    projectName: (issue: Issue) => string | undefined;
    isNew?: (issue: Issue) => boolean;
    navigate: (path: string) => void;
    /** Optional note under the rows, such as a truncation notice. */
    footer?: Snippet;
  } = $props();

  const bodyId = $derived(`workspace-group-${key}`);
</script>

<section data-group={key}>
  <h2>
    <button
      type="button"
      class="w-full flex items-center gap-2 px-1 py-1.5 rounded-md text-left
             hover:bg-[var(--bg-subtle)] transition-colors
             focus-visible:outline-2 focus-visible:outline-[var(--accent)]"
      aria-expanded={!collapsed}
      aria-controls={bodyId}
      onclick={onToggle}
    >
      <ChevronRight
        size={13}
        class="shrink-0 text-[var(--text-faint)] transition-transform {collapsed ? '' : 'rotate-90'}"
      />
      <span class="shrink-0 flex text-[var(--text-faint)]">{@render icon()}</span>
      <span class="text-caption font-medium text-[var(--text-muted)]">{label}</span>
      <span class="text-micro text-[var(--text-faint)] tabular-nums">{issues.length}</span>
    </button>
  </h2>
  {#if !collapsed}
    <div
      id={bodyId}
      class="mt-1 rounded-lg bg-[var(--surface)] border border-[var(--border)] overflow-hidden"
    >
      {#each issues as issue (issue.id)}
        <WorkspaceIssueRow
          {issue}
          projectName={projectName(issue)}
          isNew={isNew?.(issue) ?? false}
          {navigate}
        />
      {/each}
      {#if footer}
        <div class="px-4 py-2 border-t border-[var(--border)] text-caption text-[var(--text-faint)]">
          {@render footer()}
        </div>
      {/if}
    </div>
  {/if}
</section>

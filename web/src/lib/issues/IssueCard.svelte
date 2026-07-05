<script lang="ts">
  // Board (kanban) card. Extracted from IssueList.svelte (LIF-99).
  // A pure leaf: it renders one issue and emits a click; all drag behavior
  // is owned by the dndzone in the parent, which wraps this component.
  import type { Issue, Label } from "../api";
  import PriorityIcon from "../PriorityIcon.svelte";
  import Tooltip from "../Tooltip.svelte";
  import TimeAgo from "../TimeAgo.svelte";
  import { PanelRight, ExternalLink } from "lucide-svelte";
  import { openContextMenu } from "../contextMenuState.svelte"; // LIF-248
  import { projectCodeOf } from "../references"; // LIF-248

  let {
    issue,
    labels,
    onOpen,
    onPeek,
  }: {
    issue: Issue;
    /** Project labels, used to resolve each label chip's color. */
    labels: Label[];
    /** Invoked when the card is clicked (parent navigates to the issue). */
    onOpen: (issue: Issue) => void;
    /** LIF-244: opens the peek panel on this issue — mod-click (cmd/ctrl),
     *  shift-click (LIF-248), or the hover affordance button. Unlike
     *  IssueRow, the board has no existing ctrl/cmd-click OR shift-click
     *  behavior to collide with (no range-select on the board), so both
     *  modifiers are safe to wire directly on the card body here. */
    onPeek: (issue: Issue) => void;
  } = $props();

  // LIF-248: right-click → preview / open-in-new-tab. Doesn't touch
  // selection or navigation — a separate event from `onclick` below, so
  // this can't interfere with drag-and-drop (which starts on mousedown,
  // not contextmenu) or the mod/shift-click peek handling.
  function handleContextMenu(e: MouseEvent) {
    e.preventDefault();
    e.stopPropagation();
    openContextMenu(e.clientX, e.clientY, [
      { label: "Open preview", icon: PanelRight, action: () => onPeek(issue) },
      {
        label: "Open in new tab",
        icon: ExternalLink,
        action: () =>
          window.open(
            `${location.origin}/#/${projectCodeOf(issue.identifier)}/issues/${issue.identifier}`,
            "_blank",
            "noopener",
          ),
      },
    ]);
  }
</script>

<!-- svelte-ignore a11y_no_static_element_interactions a11y_no_noninteractive_element_interactions a11y_click_events_have_key_events -->
<article
  class="bg-[var(--surface)] border border-[var(--border)]
         rounded-md p-2.5 cursor-grab active:cursor-grabbing
         hover:border-[var(--text-faint)]
         transition-colors group"
  tabindex="0"
  onclick={(e) => {
    if (e.ctrlKey || e.metaKey || e.shiftKey) {
      e.preventDefault();
      onPeek(issue);
      return;
    }
    onOpen(issue);
  }}
  oncontextmenu={handleContextMenu}
>
  <!-- Top row: identifier + peek affordance + priority -->
  <div class="flex items-center gap-2 mb-1.5">
    <span class="text-micro font-mono text-[var(--text-faint)]">
      {issue.identifier}
    </span>
    <div class="flex-1"></div>
    <!-- LIF-244: hover-revealed peek trigger on pointer devices. On touch
         it's always visible (LIF-275) — with drag being the only other
         board interaction, this is the touch path to change status/
         priority/module via the peek bottom sheet. -->
    <Tooltip content="Peek">
      <button
        class="hidden shrink-0 size-5 items-center justify-center rounded
               text-[var(--text-faint)] hover:text-[var(--accent)]
               hover:bg-[var(--bg-subtle)] transition-colors
               [@media(hover:hover)]:flex [@media(hover:hover)]:opacity-0
               [@media(hover:hover)]:group-hover:opacity-100
               pointer-coarse:flex pointer-coarse:opacity-100 pointer-coarse:size-7"
        onclick={(e) => {
          e.stopPropagation();
          onPeek(issue);
        }}
      >
        <PanelRight size={12} />
      </button>
    </Tooltip>
    {#if issue.priority !== "none"}
      <Tooltip content={issue.priority[0].toUpperCase() + issue.priority.slice(1)}>
        <PriorityIcon priority={issue.priority} size={14} />
      </Tooltip>
    {/if}
  </div>

  <!-- Title -->
  <h3
    class="text-body-sm text-[var(--text)] leading-snug line-clamp-3
           {issue.status === 'done' || issue.status === 'cancelled'
      ? 'line-through text-[var(--text-muted)]'
      : ''}"
  >
    {issue.title}
  </h3>

  <!-- Bottom: labels + updated time. Always rendered — updated_at always
       exists, so the time anchor keeps card heights consistent whether or
       not labels do. -->
  <div class="flex items-center gap-1.5 mt-2 flex-wrap">
    {#each issue.labels.slice(0, 3) as lbl}
      {@const labelObj = labels.find((l) => l.name === lbl)}
      <span
        class="text-micro font-medium px-1.5 py-0.5
               rounded-full border border-[var(--border)]"
        style={labelObj
          ? `color: ${labelObj.color}; border-color: ${labelObj.color}40;`
          : ""}
      >
        {lbl}
      </span>
    {/each}
    {#if issue.labels.length > 3}
      <span class="text-micro text-[var(--text-faint)]">
        +{issue.labels.length - 3}
      </span>
    {/if}
    <div class="flex-1"></div>
    <span class="text-micro text-[var(--text-faint)] tabular-nums">
      <TimeAgo date={issue.updated_at} />
    </span>
  </div>
</article>

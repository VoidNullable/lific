<script lang="ts">
  /*
   * LIF-363 — the issue card rendered inside a Svelte Flow node. Same visual
   * vocabulary as the issue list rows: StatusIcon + mono identifier +
   * PriorityIcon on the top line, truncated title under it.
   *
   * The left/right dots are xyflow connection handles: drag from the right
   * dot of one card onto another card to create a relation (the parent's
   * onconnect opens the relation-type menu). Dragging the card body pans the
   * node itself — positions are ephemeral, relaid out on every load.
   */
  import { Handle, Position, type NodeProps } from "@xyflow/svelte";
  import StatusIcon from "../StatusIcon.svelte";
  import PriorityIcon from "../PriorityIcon.svelte";
  import { longpress } from "../actions/longpress";
  import type { Issue } from "../api";

  let { data, isConnectable }: NodeProps & {
    data: { issue: Issue; onLongPress?: (issue: Issue) => void };
  } = $props();

  let issue = $derived(data.issue as Issue);
  let closed = $derived(issue.status === "done" || issue.status === "cancelled");
</script>

<!-- Touch has no hover, so press-and-hold stands in for the hover preview:
     it opens the issue peek bottom sheet (the richer surface), while a plain
     tap keeps meaning navigate. -->
<div
  class="w-[200px] h-[58px] text-left rounded-lg border bg-[var(--surface)]
         shadow-[0_1px_2px_rgba(0,0,0,0.06)] px-2.5 py-1.5 overflow-hidden
         border-[var(--border)] hover:border-[var(--accent)] transition-colors
         select-none {closed ? 'opacity-50' : ''}"
  use:longpress={{ onLongPress: () => data.onLongPress?.(issue) }}
>
  <span class="flex items-center gap-1.5">
    <StatusIcon status={issue.status} size={12} />
    <span class="font-mono text-micro text-[var(--text-faint)]">
      {issue.identifier}
    </span>
    <span class="ml-auto">
      <PriorityIcon priority={issue.priority} size={12} />
    </span>
  </span>
  <span class="block text-caption text-[var(--text)] truncate leading-snug mt-0.5">
    {issue.title}
  </span>

  <Handle
    type="target"
    position={Position.Left}
    {isConnectable}
    class="!size-2.5 !rounded-full !border-2 !border-[var(--surface)] !bg-[var(--text-faint)]"
  />
  <Handle
    type="source"
    position={Position.Right}
    {isConnectable}
    class="!size-2.5 !rounded-full !border-2 !border-[var(--surface)] !bg-[var(--accent)]"
  />
</div>

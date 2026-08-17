<script lang="ts">
  // LIF-418: the plain download chip.
  //
  // This is the floor every viewer falls back to: unknown type, preview
  // endpoint missing or erroring, file too large to inline, backend older than
  // the frontend. It is deliberately the same object the app showed before the
  // viewers existed, so a degraded render is indistinguishable from the old
  // behavior rather than looking broken.

  import { FileText, Download } from "lucide-svelte";
  import { downloadAttachment, formatBytes } from "../../api";

  let {
    id,
    filename,
    sizeBytes = null,
    /** Shown instead of the size, e.g. "preview unavailable". */
    note = null,
  }: {
    id: number;
    filename: string;
    sizeBytes?: number | null;
    note?: string | null;
  } = $props();
</script>

<button
  type="button"
  class="fc"
  title="Download {filename}"
  onclick={() => void downloadAttachment(id, filename)}
>
  <span class="fc__icon"><FileText size={15} /></span>
  <span class="fc__body">
    <span class="fc__name">{filename}</span>
    {#if note}
      <span class="fc__meta">{note}</span>
    {:else if sizeBytes != null}
      <span class="fc__meta">{formatBytes(sizeBytes)}</span>
    {/if}
  </span>
  <span class="fc__dl"><Download size={13} /></span>
</button>

<style>
  .fc {
    display: inline-flex;
    align-items: center;
    gap: 0.5rem;
    max-width: 20rem;
    padding: 0.375rem 0.5rem 0.375rem 0.4375rem;
    border: 1px solid var(--border);
    border-radius: 0.5rem;
    background: var(--surface);
    text-align: left;
    vertical-align: middle;
    cursor: pointer;
    transition:
      border-color 0.15s var(--ease-out-expo),
      background 0.15s var(--ease-out-expo);
  }
  .fc:hover {
    border-color: var(--accent);
    background: var(--bg-subtle);
  }
  .fc__icon,
  .fc__dl {
    display: inline-flex;
    color: var(--text-muted);
    flex-shrink: 0;
  }
  .fc:hover .fc__dl {
    color: var(--accent);
  }
  .fc__body {
    display: flex;
    flex-direction: column;
    min-width: 0;
  }
  .fc__name {
    font-size: var(--text-body-sm);
    color: var(--text);
    white-space: nowrap;
    overflow: hidden;
    text-overflow: ellipsis;
  }
  .fc__meta {
    font-size: var(--text-micro);
    color: var(--text-faint);
    font-variant-numeric: tabular-nums;
  }
  @media (prefers-reduced-motion: reduce) {
    .fc {
      transition: none;
    }
  }
</style>

<script lang="ts">
  // LIF-418: shared chrome for every inline attachment viewer.
  //
  // One card, one header row (icon, filename, meta, actions, download), one
  // body. Every viewer reuses it so a zip listing, a log, and a patch all read
  // as the same object on the page, and so the download affordance never goes
  // missing: whatever the viewer manages to render, the raw file is still one
  // click away in the same place.

  import type { Snippet } from "svelte";
  import { Download } from "lucide-svelte";
  import { downloadAttachment, formatBytes } from "../../api";

  let {
    id,
    filename,
    sizeBytes = null,
    icon,
    meta = null,
    actions,
    children,
    tone = "default",
  }: {
    id: number;
    filename: string;
    sizeBytes?: number | null;
    icon?: Snippet;
    /** Short descriptor shown after the filename ("412 lines", "3 tables"). */
    meta?: string | null;
    actions?: Snippet;
    children?: Snippet;
    /** "flush" drops the body padding for viewers that draw their own rows. */
    tone?: "default" | "flush";
  } = $props();
</script>

<section class="vf" data-attachment-viewer={id}>
  <header class="vf__head">
    <span class="vf__icon">{@render icon?.()}</span>
    <span class="vf__name" title={filename}>{filename}</span>
    {#if sizeBytes != null}
      <span class="vf__meta">{formatBytes(sizeBytes)}</span>
    {/if}
    {#if meta}
      <span class="vf__meta">{meta}</span>
    {/if}
    <span class="vf__spacer"></span>
    {@render actions?.()}
    <button
      type="button"
      class="vf__btn"
      title="Download {filename}"
      aria-label="Download {filename}"
      onclick={() => void downloadAttachment(id, filename)}
    >
      <Download size={13} />
    </button>
  </header>
  <div class="vf__body" class:vf__body--flush={tone === "flush"}>
    {@render children?.()}
  </div>
</section>

<style>
  .vf {
    display: block;
    margin: 0.75rem 0;
    border: 1px solid var(--border);
    border-radius: 0.5rem;
    background: var(--surface);
    overflow: hidden;
    max-width: 100%;
  }
  .vf__head {
    display: flex;
    align-items: center;
    gap: 0.5rem;
    padding: 0.4375rem 0.5rem 0.4375rem 0.625rem;
    border-bottom: 1px solid var(--border);
    background: var(--bg-subtle);
    font-size: var(--text-body-sm);
    line-height: 1.3;
  }
  .vf__icon {
    display: inline-flex;
    align-items: center;
    color: var(--text-muted);
    flex-shrink: 0;
  }
  .vf__name {
    color: var(--text);
    font-weight: 500;
    white-space: nowrap;
    overflow: hidden;
    text-overflow: ellipsis;
    min-width: 0;
  }
  .vf__meta {
    color: var(--text-faint);
    font-size: var(--text-micro);
    font-variant-numeric: tabular-nums;
    white-space: nowrap;
    flex-shrink: 0;
  }
  .vf__spacer {
    flex: 1 1 auto;
    min-width: 0.25rem;
  }
  .vf__body {
    padding: 0.5rem 0.625rem;
  }
  .vf__body--flush {
    padding: 0;
  }

  /* Header action buttons, shared by every viewer through the `actions`
     snippet (which renders inside this component's style scope). */
  :global(.vf__head .vf__btn) {
    display: inline-flex;
    align-items: center;
    gap: 0.25rem;
    padding: 0.1875rem 0.375rem;
    border: 1px solid transparent;
    border-radius: 0.3125rem;
    background: transparent;
    color: var(--text-muted);
    font-size: var(--text-micro);
    line-height: 1.4;
    cursor: pointer;
    flex-shrink: 0;
    transition:
      color 0.15s var(--ease-out-expo),
      background 0.15s var(--ease-out-expo),
      border-color 0.15s var(--ease-out-expo);
  }
  :global(.vf__head .vf__btn:hover) {
    color: var(--text);
    background: var(--surface);
    border-color: var(--border);
  }
  :global(.vf__head .vf__btn[aria-pressed="true"]) {
    color: var(--accent);
    border-color: var(--border);
  }
  @media (prefers-reduced-motion: reduce) {
    :global(.vf__head .vf__btn) {
      transition: none;
    }
  }
</style>

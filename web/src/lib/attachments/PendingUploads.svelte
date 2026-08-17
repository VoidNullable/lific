<script lang="ts">
  // LIF-268: horizontal strip of in-flight / failed upload chips, rendered
  // directly under a composer's textarea.
  //
  // One chip per pending upload. While uploading it shows a filename + size and
  // a spinner (or a live thumbnail for images). On the server rejecting it the
  // chip flips to an error state carrying the exact reason, with Retry and
  // Dismiss actions. On success the controller removes the item and the markdown
  // reference is inserted at the caret — so a resolved chip simply disappears.
  //
  // Purely presentational: all state + actions come from the shared
  // UploadController the parent composer owns, so Comments and EditableMarkdown
  // never fork this behaviour.

  import { formatBytes } from "../api";
  import { FileText, RotateCw, X, AlertCircle } from "lucide-svelte";
  import AltTextInput from "./AltTextInput.svelte";
  import type { UploadController } from "./uploads.svelte";

  let { controller }: { controller: UploadController } = $props();
</script>

{#if controller.items.length > 0}
  <ul class="pu" aria-label="Pending uploads">
    {#each controller.items as item (item.id)}
      <li
        class="pu__chip"
        class:pu__chip--error={item.status === "error"}
        class:pu__chip--alt={item.status === "alt"}
      >
        <span class="pu__lead">
          {#if item.previewUrl}
            <img class="pu__thumb" src={item.previewUrl} alt={item.filename} />
          {:else}
            <span class="pu__icon"><FileText size={15} /></span>
          {/if}
          {#if item.status === "uploading"}
            <span class="pu__spinner" aria-label="Uploading"></span>
          {:else if item.status === "error"}
            <span class="pu__badge" aria-hidden="true"><AlertCircle size={12} /></span>
          {/if}
        </span>

        <!-- LIF-418: a settled image swaps its metadata line for the alt-text
             offer. Same chip, same position, so nothing jumps. -->
        {#if item.status === "alt"}
          <AltTextInput
            onApply={(alt) => controller.applyAlt(item.id, alt)}
            onSkip={() => controller.dismiss(item.id)}
          />
        {:else}
          <span class="pu__body">
            <span class="pu__name" title={item.filename}>{item.filename}</span>
            {#if item.status === "error"}
              <span class="pu__err" title={item.error ?? "Upload failed"}>
                {item.error ?? "Upload failed"}
              </span>
            {:else}
              <span class="pu__size">{formatBytes(item.size)}</span>
            {/if}
          </span>

          {#if item.status === "error"}
            <span class="pu__actions">
              <button
                type="button"
                class="pu__act"
                title="Retry upload"
                aria-label="Retry upload"
                onclick={() => controller.retry(item.id)}
              >
                <RotateCw size={13} />
              </button>
              <button
                type="button"
                class="pu__act"
                title="Dismiss"
                aria-label="Dismiss"
                onclick={() => controller.dismiss(item.id)}
              >
                <X size={13} />
              </button>
            </span>
          {/if}
        {/if}
      </li>
    {/each}
  </ul>
{/if}

<style>
  .pu {
    display: flex;
    flex-wrap: wrap;
    gap: 0.5rem;
    margin: 0;
    padding: 0.625rem 1rem 0;
    list-style: none;
  }

  /* Chip vocabulary mirrors the read-side attachment chips (surface card,
     1px border, 0.5rem radius) so pending and settled attachments read as one
     family. */
  .pu__chip {
    display: inline-flex;
    align-items: center;
    gap: 0.5rem;
    max-width: 15rem;
    padding: 0.375rem 0.5rem;
    border: 1px solid var(--border);
    border-radius: 0.5rem;
    background: var(--surface);
    transition:
      border-color 0.15s var(--ease-out-expo),
      background 0.15s var(--ease-out-expo);
  }
  .pu__chip--error {
    border-color: color-mix(in srgb, var(--error) 55%, var(--border));
    background: var(--error-bg);
  }
  /* The alt-text chip is wider than a status chip because it holds an input,
     and it is the only chip that should look inviting rather than transient. */
  .pu__chip--alt {
    max-width: 22rem;
    border-color: color-mix(in srgb, var(--accent) 35%, var(--border));
  }

  /* Leading slot stacks the thumbnail/icon with a small status glyph. */
  .pu__lead {
    position: relative;
    display: inline-flex;
    align-items: center;
    justify-content: center;
    flex-shrink: 0;
  }
  .pu__thumb {
    width: 1.75rem;
    height: 1.75rem;
    object-fit: cover;
    border-radius: 0.3125rem;
    display: block;
    background: var(--bg-subtle);
  }
  .pu__icon {
    display: inline-flex;
    align-items: center;
    justify-content: center;
    width: 1.75rem;
    height: 1.75rem;
    border-radius: 0.3125rem;
    background: var(--bg-subtle);
    color: var(--text-muted);
  }

  .pu__spinner {
    position: absolute;
    right: -3px;
    bottom: -3px;
    width: 13px;
    height: 13px;
    border-radius: 999px;
    border: 2px solid var(--surface);
    border-top-color: var(--accent);
    box-shadow: 0 0 0 1px var(--border);
    animation: pu-spin 0.6s linear infinite;
  }
  .pu__badge {
    position: absolute;
    right: -4px;
    bottom: -4px;
    display: inline-flex;
    color: var(--error);
    background: var(--surface);
    border-radius: 999px;
  }

  .pu__body {
    display: flex;
    flex-direction: column;
    min-width: 0;
    line-height: 1.3;
  }
  .pu__name {
    font-size: 0.75rem;
    color: var(--text);
    white-space: nowrap;
    overflow: hidden;
    text-overflow: ellipsis;
  }
  .pu__size {
    font-size: 0.6875rem;
    color: var(--text-faint);
    font-variant-numeric: tabular-nums;
  }
  .pu__err {
    font-size: 0.6875rem;
    color: var(--error);
    white-space: nowrap;
    overflow: hidden;
    text-overflow: ellipsis;
  }

  .pu__actions {
    display: inline-flex;
    align-items: center;
    gap: 0.125rem;
    flex-shrink: 0;
  }
  .pu__act {
    display: inline-flex;
    align-items: center;
    justify-content: center;
    width: 1.375rem;
    height: 1.375rem;
    border: 0;
    border-radius: 0.3125rem;
    background: transparent;
    color: var(--text-muted);
    cursor: pointer;
    transition:
      background 0.15s var(--ease-out-expo),
      color 0.15s var(--ease-out-expo);
  }
  .pu__act:hover {
    background: var(--bg-subtle);
    color: var(--text);
  }
  .pu__act:focus-visible {
    outline: 2px solid var(--accent);
    outline-offset: 1px;
  }

  @keyframes pu-spin {
    to {
      transform: rotate(360deg);
    }
  }

  @media (prefers-reduced-motion: reduce) {
    .pu__chip,
    .pu__act {
      transition: none;
    }
    .pu__spinner {
      animation-duration: 1.4s;
    }
  }
</style>

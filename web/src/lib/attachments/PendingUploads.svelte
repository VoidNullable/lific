<script lang="ts">
  // LIF-268 / LIF-418: strip of outstanding upload chips, rendered directly
  // under a composer's textarea.
  //
  // One chip per upload. While transferring it shows a determinate progress
  // bar with real byte counts (XMLHttpRequest reports them; fetch cannot) and
  // a cancel button. On rejection the chip flips to an error state carrying
  // the exact server reason, with Retry and Dismiss. On success the controller
  // removes the item and inserts the markdown reference at the caret, so a
  // resolved chip simply disappears.
  //
  // Two states bracket the transfer. `offer` is the resize question: an image
  // that is enormous or close to the instance's byte cap pauses here and asks
  // before spending the bandwidth. Answering it once settles it for the
  // session. `alt` is the opposite end - a settled, successful image upload
  // whose chip lingers only to offer a one-line description, in the same slot
  // so nothing jumps.
  //
  // Purely presentational: all state and actions come from the shared
  // UploadController the parent composer owns.

  import { formatBytes } from "../api";
  import { FileText, RotateCw, X, AlertCircle, Scaling } from "lucide-svelte";
  import AltTextInput from "./AltTextInput.svelte";
  import type { PendingUpload, UploadController } from "./uploads.svelte";

  let { controller }: { controller: UploadController } = $props();

  function percent(item: PendingUpload): number {
    if (item.total <= 0) return 0;
    return Math.min(100, Math.round((item.loaded / item.total) * 100));
  }

  function offerLabel(item: PendingUpload): string {
    const offer = item.offer;
    if (!offer) return "";
    return `Resize to ${offer.targetEdge}px (~${formatBytes(offer.estimatedBytes)})`;
  }

  function offerReason(item: PendingUpload): string {
    const offer = item.offer;
    if (!offer) return "";
    if (offer.reason === "size") {
      return `${formatBytes(item.size)} is close to this server's limit.`;
    }
    return `${formatBytes(item.size)}, larger than most screens can show.`;
  }
</script>

{#if controller.items.length > 0}
  <ul class="pu" aria-label="Pending uploads">
    {#each controller.items as item (item.id)}
      <li
        class="pu__chip"
        class:pu__chip--error={item.status === "error"}
        class:pu__chip--offer={item.status === "offer"}
        class:pu__chip--alt={item.status === "alt"}
      >
        <div class="pu__row">
          <span class="pu__lead">
            {#if item.previewUrl}
              <img class="pu__thumb" src={item.previewUrl} alt={item.filename} />
            {:else}
              <span class="pu__icon"><FileText size={15} /></span>
            {/if}
            {#if item.status === "error"}
              <span class="pu__badge" aria-hidden="true"><AlertCircle size={12} /></span>
            {:else if item.status === "offer"}
              <span class="pu__badge pu__badge--offer" aria-hidden="true">
                <Scaling size={12} />
              </span>
            {:else if item.status !== "alt"}
              <span class="pu__spinner" aria-label="Uploading"></span>
            {/if}
          </span>

          <!-- A settled image swaps its metadata line for the alt-text offer.
               Same chip, same position, so nothing jumps. -->
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
              {:else if item.status === "offer"}
                <span class="pu__size">{offerReason(item)}</span>
              {:else if item.status === "queued"}
                <span class="pu__size">Queued · {formatBytes(item.size)}</span>
              {:else}
                <span class="pu__size">
                  {formatBytes(item.loaded)} of {formatBytes(item.total)}
                </span>
              {/if}
            </span>

            <span class="pu__actions">
              {#if item.status === "error"}
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
              {:else if item.status !== "offer"}
                <button
                  type="button"
                  class="pu__act"
                  title="Cancel upload"
                  aria-label={`Cancel upload of ${item.filename}`}
                  onclick={() => controller.cancel(item.id)}
                >
                  <X size={13} />
                </button>
              {/if}
            </span>
          {/if}
        </div>

        {#if item.status === "queued" || item.status === "uploading"}
          <div
            class="pu__track"
            role="progressbar"
            aria-label={`Uploading ${item.filename}`}
            aria-valuemin={0}
            aria-valuemax={100}
            aria-valuenow={percent(item)}
          >
            <div class="pu__fill" style:width={`${percent(item)}%`}></div>
          </div>
        {/if}

        {#if item.status === "offer" && item.offer}
          <div class="pu__choice">
            <button
              type="button"
              class="pu__choice-btn pu__choice-btn--primary"
              onclick={() => controller.acceptDownscale(item.id)}
            >
              {offerLabel(item)}
            </button>
            <button
              type="button"
              class="pu__choice-btn"
              onclick={() => controller.keepOriginal(item.id)}
            >
              Upload original
            </button>
            <button
              type="button"
              class="pu__act pu__choice-cancel"
              title="Cancel upload"
              aria-label={`Cancel upload of ${item.filename}`}
              onclick={() => controller.cancel(item.id)}
            >
              <X size={13} />
            </button>
          </div>
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
    display: flex;
    flex-direction: column;
    gap: 0.375rem;
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
  /* The resize question needs room for two real buttons, so its chip widens
     out of the compact row rhythm. */
  .pu__chip--offer {
    max-width: 22rem;
    border-color: color-mix(in srgb, var(--accent) 45%, var(--border));
  }
  /* The alt-text chip is wider than a status chip because it holds an input,
     and it is the only chip that should look inviting rather than transient. */
  .pu__chip--alt {
    max-width: 22rem;
    border-color: color-mix(in srgb, var(--accent) 35%, var(--border));
  }

  .pu__row {
    display: flex;
    align-items: center;
    gap: 0.5rem;
    min-width: 0;
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
  .pu__badge--offer {
    color: var(--accent);
  }

  .pu__body {
    display: flex;
    flex-direction: column;
    min-width: 0;
    flex: 1;
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
    white-space: nowrap;
    overflow: hidden;
    text-overflow: ellipsis;
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

  /* Determinate progress. A 3px rail under the chip body keeps the chip's
     height stable while still reading as motion across a long upload. */
  .pu__track {
    height: 3px;
    border-radius: 999px;
    background: var(--bg-subtle);
    overflow: hidden;
  }
  .pu__fill {
    height: 100%;
    border-radius: 999px;
    background: var(--accent);
    transition: width 0.18s var(--ease-out-expo);
  }

  .pu__choice {
    display: flex;
    align-items: center;
    gap: 0.375rem;
    flex-wrap: wrap;
  }
  .pu__choice-btn {
    padding: 0.25rem 0.5rem;
    border: 1px solid var(--border);
    border-radius: 0.375rem;
    background: transparent;
    color: var(--text-muted);
    font-size: 0.6875rem;
    font-weight: 500;
    cursor: pointer;
    transition:
      background 0.15s var(--ease-out-expo),
      border-color 0.15s var(--ease-out-expo),
      color 0.15s var(--ease-out-expo);
  }
  .pu__choice-btn:hover {
    border-color: var(--accent);
    color: var(--accent);
  }
  .pu__choice-btn--primary {
    border-color: transparent;
    background: var(--accent);
    color: var(--accent-text);
  }
  .pu__choice-btn--primary:hover {
    background: var(--accent-hover);
    color: var(--accent-text);
  }
  .pu__choice-cancel {
    margin-left: auto;
  }

  @keyframes pu-spin {
    to {
      transform: rotate(360deg);
    }
  }

  @media (prefers-reduced-motion: reduce) {
    .pu__chip,
    .pu__act,
    .pu__choice-btn,
    .pu__fill {
      transition: none;
    }
    .pu__spinner {
      animation-duration: 1.4s;
    }
  }
</style>

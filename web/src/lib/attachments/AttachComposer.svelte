<script lang="ts">
  // LIF-418: the shared chrome around an attachment-enabled composer.
  //
  // Wraps whatever markup a composer already has (textarea, mention popover,
  // toolbar, footer) in the drop zone, hangs the pending-upload strip under
  // it, renders the big-paste offer, and owns the hidden file input the
  // Attach button clicks. Behaviour comes from `createComposerAttachments`;
  // this component only places it.
  //
  // Layout stays with the caller: `children` is the input area and `footer` is
  // whatever sits below the pending strip (a comment toolbar, the editor's
  // Save/Cancel row). Same layout-prop composition the issue list uses, so no
  // surface has to give up its own arrangement to share the behaviour.

  import type { Snippet } from "svelte";
  import DropOverlay from "./DropOverlay.svelte";
  import PendingUploads from "./PendingUploads.svelte";
  import type { ComposerAttachments } from "./composer.svelte";
  import { CornerDownLeft } from "lucide-svelte";

  let {
    composer,
    radius = "0.75rem",
    dropLabel = "Drop files to attach",
    accept = "image/*,application/pdf,text/plain,.log,application/zip",
    children,
    footer,
  }: {
    composer: ComposerAttachments;
    radius?: string;
    dropLabel?: string;
    accept?: string;
    children: Snippet;
    footer?: Snippet;
  } = $props();

  let inputEl = $state<HTMLInputElement | null>(null);

  $effect(() => {
    composer.bindFileInput(inputEl);
    return () => composer.bindFileInput(null);
  });
</script>

<DropOverlay {radius} label={dropLabel} onFiles={(files) => composer.enqueue(files, "drop")}>
  {@render children()}

  <PendingUploads controller={composer.uploads} />

  {@render footer?.()}

  {#if composer.pendingPaste}
    <!-- Inline, not modal: the textarea keeps focus and typing carries on
         while this sits there. Enter attaches, Escape pastes inline, and the
         buttons do the same for anyone reaching for the mouse. -->
    <div class="bp" role="status">
      <span class="bp__text">
        That is {composer.pendingPaste.summary}. Attach it as a file?
      </span>
      <button type="button" class="bp__btn bp__btn--primary" onclick={() => composer.attachPaste()}>
        Attach as file
        <kbd class="bp__kbd"><CornerDownLeft size={10} /></kbd>
      </button>
      <button type="button" class="bp__btn" onclick={() => composer.pasteInline()}>
        Paste inline
        <kbd class="bp__kbd">Esc</kbd>
      </button>
    </div>
  {/if}
</DropOverlay>

<input
  bind:this={inputEl}
  type="file"
  class="ac__file-input"
  multiple
  {accept}
  onchange={(e) => composer.handleFilePicked(e)}
/>

<style>
  .ac__file-input {
    display: none;
  }

  /* Anchored to the composer, not the viewport: the offer belongs to the box
     you pasted into. Sits above the drop overlay's z-index so a drag mid-offer
     does not bury it. */
  .bp {
    position: absolute;
    left: 0.5rem;
    right: 0.5rem;
    bottom: 0.5rem;
    z-index: 35;
    display: flex;
    align-items: center;
    gap: 0.5rem;
    flex-wrap: wrap;
    padding: 0.4375rem 0.625rem;
    border: 1px solid color-mix(in srgb, var(--accent) 45%, var(--border));
    border-radius: 0.5rem;
    background: var(--surface);
    box-shadow: 0 8px 20px rgb(0 0 0 / 0.18);
    animation: bp-rise 0.14s var(--ease-out-expo);
  }
  .bp__text {
    flex: 1;
    min-width: 8rem;
    font-size: 0.75rem;
    color: var(--text);
  }
  .bp__btn {
    display: inline-flex;
    align-items: center;
    gap: 0.3125rem;
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
  .bp__btn:hover {
    border-color: var(--accent);
    color: var(--accent);
  }
  .bp__btn--primary {
    border-color: transparent;
    background: var(--accent);
    color: var(--accent-text);
  }
  .bp__btn--primary:hover {
    background: var(--accent-hover);
    color: var(--accent-text);
  }
  .bp__kbd {
    display: inline-flex;
    align-items: center;
    justify-content: center;
    min-width: 1rem;
    height: 1rem;
    padding: 0 0.1875rem;
    border-radius: 0.25rem;
    background: color-mix(in srgb, currentColor 16%, transparent);
    font-family: var(--font-mono);
    font-size: 0.5625rem;
    line-height: 1;
  }

  @keyframes bp-rise {
    from {
      opacity: 0;
      transform: translateY(4px);
    }
    to {
      opacity: 1;
      transform: none;
    }
  }

  @media (prefers-reduced-motion: reduce) {
    .bp {
      animation: none;
    }
    .bp__btn {
      transition: none;
    }
  }
</style>

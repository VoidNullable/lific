<script lang="ts">
  // LIF-418: the one-line alt-text offer on a settled image upload.
  //
  // Deliberately does NOT autofocus. The chip appears while you are mid
  // sentence in the composer; yanking the caret out of the textarea to ask an
  // optional question would be worse than no accessibility prompt at all. It
  // sits there, it is obvious, and it costs one click to use or nothing to
  // ignore.
  //
  // Remembers nothing between uploads: no history, no suggestion list, no
  // "apply to all". One image, one description.

  import { Check, X } from "lucide-svelte";

  let {
    onApply,
    onSkip,
  }: {
    onApply: (alt: string) => void;
    onSkip: () => void;
  } = $props();

  let value = $state("");

  function commit() {
    // An empty field means "I do not want to do this", not "clear the alt".
    if (value.trim() === "") onSkip();
    else onApply(value);
  }

  function onKeydown(e: KeyboardEvent) {
    if (e.key === "Enter") {
      e.preventDefault();
      e.stopPropagation();
      commit();
    } else if (e.key === "Escape") {
      e.preventDefault();
      e.stopPropagation();
      onSkip();
    }
  }
</script>

<span class="alt">
  <input
    class="alt__input"
    type="text"
    bind:value
    placeholder="Describe this image, Enter to apply, Esc to skip"
    aria-label="Alt text for the image you just uploaded"
    onkeydown={onKeydown}
  />
  <button
    type="button"
    class="alt__act"
    title="Apply description"
    aria-label="Apply description"
    onclick={commit}
  >
    <Check size={13} />
  </button>
  <button type="button" class="alt__act" title="Skip" aria-label="Skip description" onclick={onSkip}>
    <X size={13} />
  </button>
</span>

<style>
  .alt {
    display: inline-flex;
    align-items: center;
    gap: 0.125rem;
    min-width: 0;
  }
  .alt__input {
    flex: 1;
    min-width: 0;
    width: 13rem;
    padding: 0.1875rem 0.375rem;
    border: 1px solid var(--border);
    border-radius: 0.3125rem;
    background: var(--bg-subtle);
    color: var(--text);
    font-family: var(--font-body);
    font-size: var(--text-caption);
    outline: none;
    transition: border-color 0.15s var(--ease-out-expo);
  }
  .alt__input::placeholder {
    color: var(--text-faint);
  }
  .alt__input:focus {
    border-color: var(--accent);
  }
  .alt__act {
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
  .alt__act:hover {
    background: var(--surface);
    color: var(--text);
  }
  .alt__act:focus-visible {
    outline: 2px solid var(--accent);
    outline-offset: 1px;
  }

  @media (prefers-reduced-motion: reduce) {
    .alt__input,
    .alt__act {
      transition: none;
    }
  }
</style>

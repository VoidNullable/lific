<script lang="ts">
  // LIF-418: the single attach affordance every composer toolbar mounts.
  //
  // It replaces what used to be a bare "Attach" button plus a hidden
  // <input type=file> duplicated in each composer, and folds in the two new
  // capture routes so no composer has to know they exist:
  //
  //   Desktop (fine pointer)  — Attach opens the file picker directly, with a
  //                             separate microphone button beside it. Nothing
  //                             about the old interaction changed.
  //   Mobile (coarse pointer) — Attach opens a three-item menu: Files, Camera,
  //                             Record voice. On a phone the camera is the
  //                             most likely source of an attachment and it is
  //                             unreachable through a plain file input on some
  //                             Android pickers, so it gets its own entry with
  //                             `capture="environment"`.
  //
  // Pointer coarseness is read after mount, never during render, because the
  // component is server-rendered in tests where `matchMedia` does not exist.

  import { Paperclip, FileUp, Camera, Mic } from "lucide-svelte";
  import VoiceNote from "./VoiceNote.svelte";
  import type { UploadSource } from "./annotateFlow";

  let {
    onFiles,
    busy = false,
    disabled = false,
    accept = "image/*,application/pdf,text/plain,.log,application/zip",
    /** `outlined` matches the comment composer's bordered chip; `plain` matches
     *  the description editor's borderless footer action. */
    variant = "outlined",
  }: {
    onFiles: (files: File[], source: UploadSource) => void;
    busy?: boolean;
    disabled?: boolean;
    accept?: string;
    variant?: "outlined" | "plain";
  } = $props();

  let coarse = $state(false);
  let menuOpen = $state(false);
  let fileInputEl = $state<HTMLInputElement | null>(null);
  let cameraInputEl = $state<HTMLInputElement | null>(null);
  let rootEl = $state<HTMLSpanElement | null>(null);
  let voice = $state<ReturnType<typeof VoiceNote> | null>(null);

  $effect(() => {
    if (typeof window === "undefined" || typeof window.matchMedia !== "function") return;
    const query = window.matchMedia("(pointer: coarse)");
    coarse = query.matches;
    const onChange = (e: MediaQueryListEvent) => (coarse = e.matches);
    query.addEventListener("change", onChange);
    return () => query.removeEventListener("change", onChange);
  });

  $effect(() => {
    if (!menuOpen) return;
    const onDocPointer = (e: PointerEvent) => {
      if (rootEl && !rootEl.contains(e.target as Node)) menuOpen = false;
    };
    const onDocKey = (e: KeyboardEvent) => {
      if (e.key === "Escape") menuOpen = false;
    };
    document.addEventListener("pointerdown", onDocPointer);
    document.addEventListener("keydown", onDocKey);
    return () => {
      document.removeEventListener("pointerdown", onDocPointer);
      document.removeEventListener("keydown", onDocKey);
    };
  });

  function picked(e: Event, source: UploadSource) {
    const input = e.target as HTMLInputElement;
    if (input.files && input.files.length > 0) onFiles(Array.from(input.files), source);
    // Reset so picking the same file twice in a row still fires a change event.
    input.value = "";
  }

  function chooseFiles() {
    menuOpen = false;
    fileInputEl?.click();
  }

  function chooseCamera() {
    menuOpen = false;
    cameraInputEl?.click();
  }

  function chooseVoice() {
    menuOpen = false;
    void voice?.start();
  }

  function onTrigger() {
    if (coarse) menuOpen = !menuOpen;
    else chooseFiles();
  }
</script>

<span class="at" bind:this={rootEl}>
  <button
    type="button"
    class="at__button"
    class:at__button--plain={variant === "plain"}
    title="Attach files"
    aria-label="Attach files"
    aria-haspopup={coarse ? "menu" : undefined}
    aria-expanded={coarse ? menuOpen : undefined}
    {disabled}
    onclick={onTrigger}
  >
    <Paperclip size={13} />
    <span>{busy ? "Uploading\u2026" : "Attach"}</span>
  </button>

  {#if coarse && menuOpen}
    <div class="at__menu" role="menu" aria-label="Attach">
      <button type="button" role="menuitem" class="at__item" onclick={chooseFiles}>
        <FileUp size={14} /> Files
      </button>
      <button type="button" role="menuitem" class="at__item" onclick={chooseCamera}>
        <Camera size={14} /> Camera
      </button>
      {#if voice?.isSupported()}
        <button type="button" role="menuitem" class="at__item" onclick={chooseVoice}>
          <Mic size={14} /> Record voice
        </button>
      {/if}
    </div>
  {/if}

  <VoiceNote
    bind:this={voice}
    {disabled}
    showButton={!coarse}
    onFile={(file) => onFiles([file], "picker")}
  />

  <input
    bind:this={fileInputEl}
    type="file"
    class="at__input"
    multiple
    {accept}
    onchange={(e) => picked(e, "picker")}
  />
  <!-- `capture` asks the OS for the rear camera directly instead of the photo
       library. Desktop browsers ignore it, which is why this input only ever
       gets clicked from the coarse-pointer menu. -->
  <input
    bind:this={cameraInputEl}
    type="file"
    class="at__input"
    accept="image/*"
    capture="environment"
    onchange={(e) => picked(e, "camera")}
  />
</span>

<style>
  .at {
    position: relative;
    display: inline-flex;
    align-items: center;
    gap: 0.375rem;
  }

  .at__button {
    display: inline-flex;
    align-items: center;
    gap: 0.3125rem;
    padding: 0.25rem 0.5rem;
    border: 1px solid var(--border);
    border-radius: 0.375rem;
    background: transparent;
    color: var(--text-muted);
    font-family: inherit;
    font-size: var(--text-caption);
    font-weight: 500;
    cursor: pointer;
    transition:
      border-color 0.15s var(--ease-out-expo),
      background 0.15s var(--ease-out-expo),
      color 0.15s var(--ease-out-expo);
  }
  .at__button:hover:not(:disabled) {
    border-color: var(--accent);
    color: var(--accent);
  }
  .at__button:disabled {
    opacity: 0.6;
    cursor: not-allowed;
  }
  /* The description editor's footer is a row of borderless actions; a bordered
     chip in the middle of it would read as a different kind of control. */
  .at__button--plain {
    border-color: transparent;
    padding: 0.375rem 0.625rem;
    font-size: var(--text-body-sm);
    font-weight: 400;
  }
  .at__button--plain:hover:not(:disabled) {
    border-color: transparent;
    background: var(--bg-subtle);
    color: var(--accent);
  }

  .at__menu {
    position: absolute;
    left: 0;
    bottom: calc(100% + 0.375rem);
    z-index: 40;
    display: flex;
    flex-direction: column;
    min-width: 10.5rem;
    padding: 0.25rem;
    border: 1px solid var(--border);
    border-radius: 0.5rem;
    background: var(--surface);
    box-shadow: 0 8px 24px rgba(0, 0, 0, 0.16);
  }
  .at__item {
    display: flex;
    align-items: center;
    gap: 0.5rem;
    padding: 0.5rem 0.5rem;
    border: 0;
    border-radius: 0.375rem;
    background: transparent;
    color: var(--text);
    font-family: inherit;
    font-size: var(--text-body-sm);
    text-align: left;
    cursor: pointer;
  }
  .at__item:hover {
    background: var(--bg-subtle);
    color: var(--accent);
  }

  .at__input {
    display: none;
  }

  @media (prefers-reduced-motion: reduce) {
    .at__button {
      transition: none;
    }
  }
</style>

<script lang="ts">
  // A Markdown body (issue description, page content, module description).
  //
  // People who can edit get the body as a live document: they click into the
  // rendered text and type, and the Markdown under it stays exactly what they
  // typed (see ./editor). There is no edit mode, no preview toggle and no Save
  // button. Changes save on their own a moment after typing stops, on blur,
  // and with ⌘/Ctrl+S.
  //
  // Saves carry the body's `seq`, so a body changed elsewhere (an agent, a
  // second tab) is never overwritten blindly: edits to different parts merge,
  // and edits to the same text stop and ask which version to keep. Bodies
  // changed elsewhere arrive through `value`/`seq` and apply in place without
  // moving the caret.
  //
  // People who can only read get the rendered Markdown.

  import { onDestroy } from "svelte";
  import type { ChangeSet } from "@codemirror/state";
  import { TriangleAlert } from "lucide-svelte";
  import Markdown from "./Markdown.svelte";
  import MarkdownEditor from "./editor/MarkdownEditor.svelte";
  import type { BodySaveResult, SyncStatus } from "./editor/bodySync";
  import type { BodySync } from "./editor/bodySync";
  import type { AttachmentEntity } from "./api";

  let {
    value,
    seq = null,
    docId = null,
    editable = true,
    placeholder = "Write something…  Type / for headings, lists, tables and more.",
    emptyReadText = "Nothing here yet",
    proseMinHeight = "120px",
    proseClass = "",
    noun = "This text",
    onSave,
    // LIF-262: when set, files dropped/pasted/attached into the editor link
    // straight to this entity.
    attachTo = null,
    /** Push selected text into the comment box. */
    onQuote = undefined,
  }: {
    value: string;
    /** The body's write sequence as last loaded (null: no concurrency check). */
    seq?: number | null;
    /** Which record this body belongs to. Captured when the editor opens, so
     *  a save still in flight after navigating lands on the right record. */
    docId?: number | null;
    editable?: boolean;
    placeholder?: string;
    emptyReadText?: string;
    proseMinHeight?: string;
    proseClass?: string;
    /** "The description", "This page": used in the conflict notice. */
    noun?: string;
    onSave: (next: string, expectedSeq: number | null, docId: number | null) => Promise<BodySaveResult>;
    attachTo?: { entity_type: AttachmentEntity; entity_id: number } | null;
    onQuote?: ((text: string) => void) | undefined;
  } = $props();

  let editorRef = $state<MarkdownEditor | null>(null);
  let sync = $state<BodySync | null>(null);
  let syncStatus = $state<SyncStatus>("saved");
  let statusDetail = $state("");
  let conflictText = $state<string | null>(null);
  let showTheirs = $state(false);
  let mergedNotice = $state(false);
  let mergedTimer: ReturnType<typeof setTimeout> | null = null;

  // svelte-ignore state_referenced_locally
  const openedFor = docId;
  // svelte-ignore state_referenced_locally
  const openedSeq = seq;

  let hasContent = $derived(value.trim().length > 0);

  // A body changed elsewhere (a refresh after an agent's edit, a realtime
  // event, our own save coming back) reaches the editor through here.
  $effect(() => {
    const text = value;
    const s = seq;
    sync?.remote(text, s);
  });

  function onReady(
    handle: import("./editor/cm/index").MarkdownEditorHandle,
    mod: typeof import("./editor/cm/index"),
  ) {
    sync = new mod.BodySync(
      {
        doc: () => handle.text(),
        apply: (changes, undoable) => handle.applyRemote(changes, undoable),
        save: (text, expected) => onSave(text, expected, openedFor),
        onStatus: (next, detail) => {
          syncStatus = next;
          statusDetail = detail ?? "";
          conflictText = next === "conflict" ? (sync?.conflictText ?? null) : null;
          if (next !== "conflict") showTheirs = false;
        },
        onMerged: () => {
          mergedNotice = true;
          if (mergedTimer) clearTimeout(mergedTimer);
          mergedTimer = setTimeout(() => (mergedNotice = false), 5000);
        },
      },
      handle.text(),
      openedSeq,
    );
    // Anything that changed while the editor was loading.
    sync.remote(value, seq);
  }

  function onChange(changes: ChangeSet, discrete: boolean) {
    sync?.local(changes);
    // A ticked task is a finished action, not text still being typed.
    if (discrete) flushNow();
  }

  function flushNow() {
    void sync?.flush();
  }

  function onVisibility() {
    if (document.visibilityState === "hidden") flushNow();
  }

  function onBeforeUnload(e: BeforeUnloadEvent) {
    if (!sync?.dirty) return;
    flushNow();
    e.preventDefault();
  }

  onDestroy(() => {
    if (mergedTimer) clearTimeout(mergedTimer);
    const s = sync;
    if (!s) return;
    // Leaving the page with unsaved text: send it now. The save already knows
    // which record it belongs to.
    if (s.dirty && s.currentStatus !== "conflict") void s.flush();
    s.destroy();
  });

  /** Put the caret in the body (the "E" shortcut, the palette). */
  export function focus() {
    editorRef?.focus("keep");
  }

  /** Put a snippet (a file from the Attachments panel) at the caret, or at
   *  the end of the body when the caret was never placed. */
  export function insertMarkdown(snippet: string) {
    if (!editable) return;
    editorRef?.insert(snippet);
  }

  /** Save anything unsaved right away. */
  export function flush() {
    return sync?.flush();
  }
</script>

<svelte:document onvisibilitychange={onVisibility} />
<svelte:window onbeforeunload={onBeforeUnload} />

{#if !editable}
  <div class="em-rendered {proseClass}" style:min-height={proseMinHeight}>
    {#if hasContent}
      <Markdown content={value} />
    {:else}
      <p class="em-empty">{emptyReadText}</p>
    {/if}
  </div>
{:else}
  {#if conflictText !== null}
    <div class="em-conflict" role="alert">
      <TriangleAlert size={15} class="shrink-0 mt-0.5" />
      <div class="em-conflict__body">
        <p class="em-conflict__text">
          {noun} was changed somewhere else while you were editing the same part. Your text is unchanged and
          not saved yet.
        </p>
        <div class="em-conflict__actions">
          <button type="button" class="em-btn em-btn--primary" onclick={() => sync?.resolveConflict("mine")}>
            Keep mine
          </button>
          <button type="button" class="em-btn" onclick={() => sync?.resolveConflict("theirs")}>
            Use the other version
          </button>
          <button type="button" class="em-btn em-btn--ghost" onclick={() => (showTheirs = !showTheirs)}>
            {showTheirs ? "Hide" : "Show"} the other version
          </button>
        </div>
        {#if showTheirs}
          <div class="em-conflict__theirs">
            <Markdown content={conflictText} />
          </div>
        {/if}
      </div>
    </div>
  {/if}

  <div class="em-live {proseClass}" style:--md-editor-min-height={proseMinHeight}>
    <MarkdownEditor
      bind:this={editorRef}
      doc={value}
      {placeholder}
      label={noun}
      {attachTo}
      {onReady}
      {onChange}
      {onQuote}
      onSaveNow={flushNow}
      footerVisible={syncStatus !== "saved" || mergedNotice}
      onFocusChange={(f) => {
        if (!f) flushNow();
      }}
    >
      {#snippet status({ focused })}
        <span
          class="em-status"
          class:em-status--error={syncStatus === "error"}
          aria-live="polite"
        >
          {#if syncStatus === "error"}
            Couldn't save{statusDetail ? `: ${statusDetail}` : ""}. Retrying…
            <button type="button" class="em-retry" onclick={flushNow}>Retry now</button>
          {:else if syncStatus === "saving"}
            Saving…
          {:else if syncStatus === "pending"}
            Unsaved changes
          {:else if mergedNotice}
            Merged an edit made elsewhere
          {:else if focused}
            Saved
          {/if}
        </span>
      {/snippet}
    </MarkdownEditor>
  </div>
{/if}

<style>
  .em-empty {
    padding: 0.5rem 0;
    font-size: 0.875rem;
    font-style: italic;
    color: var(--text-faint);
    margin: 0;
  }

  .em-status {
    margin-left: auto;
    flex-shrink: 0;
    display: inline-flex;
    align-items: center;
    gap: 0.5rem;
    font-size: 0.75rem;
    color: var(--text-faint);
  }
  .em-status--error {
    color: var(--error);
  }
  .em-retry {
    border: 0;
    background: transparent;
    padding: 0;
    color: inherit;
    font: inherit;
    text-decoration: underline;
    cursor: pointer;
  }

  .em-conflict {
    display: flex;
    gap: 0.625rem;
    margin-bottom: 0.875rem;
    padding: 0.75rem 0.875rem;
    border: 1px solid color-mix(in srgb, var(--warn) 55%, var(--border));
    border-radius: 0.625rem;
    background: color-mix(in srgb, var(--warn) 10%, var(--surface));
    color: var(--text);
  }
  .em-conflict__body {
    min-width: 0;
    flex: 1;
  }
  .em-conflict__text {
    margin: 0;
    font-size: 0.8125rem;
    line-height: 1.5;
  }
  .em-conflict__actions {
    display: flex;
    flex-wrap: wrap;
    gap: 0.375rem;
    margin-top: 0.625rem;
  }
  .em-conflict__theirs {
    margin-top: 0.75rem;
    padding: 0.75rem;
    border-radius: 0.5rem;
    border: 1px dashed var(--border);
    background: var(--surface);
    max-height: 20rem;
    overflow: auto;
  }
  .em-btn {
    font-size: 0.75rem;
    font-weight: 500;
    padding: 0.3125rem 0.625rem;
    border-radius: 0.375rem;
    border: 1px solid var(--border);
    background: var(--surface);
    color: var(--text);
    cursor: pointer;
  }
  .em-btn:hover {
    background: var(--bg-subtle);
  }
  .em-btn--primary {
    background: var(--accent);
    border-color: var(--accent);
    color: var(--accent-text);
  }
  .em-btn--primary:hover {
    background: var(--accent-hover);
  }
  .em-btn--ghost {
    border-color: transparent;
    background: transparent;
    color: var(--text-muted);
  }
</style>

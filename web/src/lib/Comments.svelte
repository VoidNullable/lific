<script lang="ts">
  // LIF-125 — comment thread, rebuilt from scratch.
  //
  // The old version rendered comments as bare prose indistinguishable
  // from the issue/page description, with faint low-contrast chrome. This
  // is a threaded conversation: an avatar gutter with a connector line
  // ties messages into an obvious discussion, a display-weight header +
  // count badge marks where it begins, and the composer reads as a
  // first-class input (bordered surface card, focus ring, real button).
  //
  // Drop-in contract preserved so DocumentDetail can swap it for the old
  // CommentThread with no other changes.

  import Markdown from "./Markdown.svelte";
  import { formatDate, formatRelative } from "./format";
  import type { Comment } from "./api";
  import { MessageSquare, CornerDownLeft } from "lucide-svelte";
  import { fly } from "svelte/transition";
  import { tick } from "svelte";

  let {
    comments,
    editable = true,
    onSubmit,
    placeholder = "Write a comment\u2026",
  }: {
    comments: Comment[];
    editable?: boolean;
    onSubmit: (content: string) => Promise<Comment | null>;
    placeholder?: string;
  } = $props();

  let draft = $state("");
  let submitting = $state(false);
  let textareaEl = $state<HTMLTextAreaElement | null>(null);

  let canSend = $derived(draft.trim().length > 0 && !submitting);

  // ⌘ on Mac, Ctrl elsewhere — match the platform the user expects.
  const isMac =
    typeof navigator !== "undefined" && /Mac|iP(hone|ad|od)/.test(navigator.platform);

  async function submit() {
    if (!canSend) return;
    submitting = true;
    const created = await onSubmit(draft.trim());
    submitting = false;
    if (created) {
      draft = "";
      requestAnimationFrame(resize);
    }
  }

  function onKeydown(e: KeyboardEvent) {
    if (e.key === "Enter" && (e.metaKey || e.ctrlKey)) {
      e.preventDefault();
      submit();
    }
  }

  // Grow the textarea with its content; CSS min-height floors it.
  function resize() {
    const el = textareaEl;
    if (!el) return;
    el.style.height = "auto";
    el.style.height = `${el.scrollHeight}px`;
  }

  function initials(name: string): string {
    return name
      .split(/[\s_-]+/)
      .slice(0, 2)
      .map((w) => w[0]?.toUpperCase() ?? "")
      .join("");
  }

  // LIF-111 — called by the quote-in-comment toolbar (via DocumentDetail).
  // Prepends a markdown blockquote of the selected text to the composer,
  // then focuses it, resizes, drops the caret at the end, and scrolls the
  // composer into view.
  export async function insertQuote(text: string) {
    const quoted = text.split("\n").map((l) => "> " + l).join("\n") + "\n\n";
    draft = quoted + draft;
    await tick();
    const el = textareaEl;
    if (el) {
      el.focus();
      resize();
      const end = el.value.length;
      el.setSelectionRange(end, end);
      el.scrollIntoView({ behavior: "smooth", block: "center" });
    }
  }
</script>

<section class="cmt">
  <header class="cmt__head">
    <h2 class="cmt__title">Comments</h2>
    {#if comments.length > 0}
      <span class="cmt__count">{comments.length}</span>
    {/if}
  </header>

  {#if comments.length > 0}
    <ol class="cmt__thread">
      {#each comments as comment (comment.id)}
        {@const author = comment.author_display_name || comment.author}
        <li class="cmt__item" in:fly={{ y: 6, duration: 180 }}>
          <div class="cmt__avatar" aria-hidden="true">{initials(author)}</div>
          <div class="cmt__body">
            <div class="cmt__meta">
              <span class="cmt__author">{author}</span>
              <span class="cmt__time" title={formatDate(comment.created_at)}>
                {formatRelative(comment.created_at)}
              </span>
            </div>
            <div class="cmt__md">
              <Markdown content={comment.content} class="text-sm" />
            </div>
          </div>
        </li>
      {/each}
    </ol>
  {:else}
    <div class="cmt__empty">
      <span class="cmt__empty-icon"><MessageSquare size={16} /></span>
      <div>
        <p class="cmt__empty-title">No comments yet</p>
        {#if editable}
          <p class="cmt__empty-sub">Start the conversation below.</p>
        {/if}
      </div>
    </div>
  {/if}

  {#if editable}
    <div class="cmt__composer">
      <textarea
        bind:this={textareaEl}
        bind:value={draft}
        {placeholder}
        class="cmt__input"
        oninput={resize}
        onkeydown={onKeydown}
      ></textarea>
      <div class="cmt__toolbar">
        <span class="cmt__hint">Markdown supported</span>
        <div class="cmt__actions">
          <span class="cmt__kbd" aria-hidden="true">
            <kbd>{isMac ? "\u2318" : "Ctrl"}</kbd>
            <kbd><CornerDownLeft size={11} /></kbd>
          </span>
          <button class="cmt__send" disabled={!canSend} onclick={submit}>
            {submitting ? "Posting\u2026" : "Comment"}
          </button>
        </div>
      </div>
    </div>
  {/if}
</section>

<style>
  /* Clear break from the description above. A real rule + a display-weight
     header is the "comments start here" signal the old design lacked. */
  .cmt {
    margin-top: 2.5rem;
    padding-top: 2rem;
    border-top: 1px solid var(--border);
  }

  .cmt__head {
    display: flex;
    align-items: center;
    gap: 0.625rem;
    margin-bottom: 1.5rem;
  }
  .cmt__title {
    font-family: var(--font-display);
    font-size: 1.0625rem;
    font-weight: 600;
    letter-spacing: -0.01em;
    color: var(--text);
    margin: 0;
  }
  .cmt__count {
    display: inline-flex;
    align-items: center;
    justify-content: center;
    min-width: 1.375rem;
    height: 1.375rem;
    padding: 0 0.4375rem;
    border-radius: 999px;
    background: var(--bg-subtle);
    color: var(--text-muted);
    font-size: 0.6875rem;
    font-weight: 600;
    font-variant-numeric: tabular-nums;
  }

  /* ── Thread ─────────────────────────────────────────── */

  .cmt__thread {
    list-style: none;
    margin: 0 0 1.75rem;
    padding: 0;
  }

  .cmt__item {
    position: relative;
    display: grid;
    grid-template-columns: 2rem 1fr;
    gap: 0.875rem;
  }
  .cmt__item:not(:last-child) {
    padding-bottom: 1.5rem;
  }
  /* Connector line down the avatar gutter — ties the messages into an
     obvious conversation rather than a stack of loose paragraphs. */
  .cmt__item:not(:last-child)::before {
    content: "";
    position: absolute;
    left: 15px;
    top: 2.5rem;
    bottom: 0;
    width: 2px;
    border-radius: 999px;
    background: var(--border);
  }

  .cmt__avatar {
    width: 2rem;
    height: 2rem;
    border-radius: 999px;
    display: flex;
    align-items: center;
    justify-content: center;
    background: var(--accent-subtle);
    color: var(--accent);
    border: 1px solid var(--border);
    font-size: 0.6875rem;
    font-weight: 700;
    letter-spacing: 0.01em;
    user-select: none;
  }

  .cmt__body {
    min-width: 0;
    padding-top: 0.1875rem;
  }
  .cmt__meta {
    display: flex;
    align-items: baseline;
    gap: 0.5rem;
    margin-bottom: 0.125rem;
  }
  .cmt__author {
    font-size: 0.875rem;
    font-weight: 600;
    color: var(--text);
  }
  .cmt__time {
    font-size: 0.75rem;
    color: var(--text-muted);
  }
  /* Markdown body in full-strength text so it reads as content, not an
     afterthought. Kill the leading margin the prose layer adds. */
  .cmt__md :global(.prose > :first-child) {
    margin-top: 0;
  }

  /* ── Empty state ────────────────────────────────────── */

  .cmt__empty {
    display: flex;
    align-items: center;
    gap: 0.75rem;
    margin-bottom: 1.75rem;
  }
  .cmt__empty-icon {
    display: flex;
    align-items: center;
    justify-content: center;
    width: 2rem;
    height: 2rem;
    border-radius: 999px;
    background: var(--bg-subtle);
    color: var(--text-muted);
    flex-shrink: 0;
  }
  .cmt__empty-title {
    font-size: 0.875rem;
    font-weight: 500;
    color: var(--text);
    margin: 0;
  }
  .cmt__empty-sub {
    font-size: 0.8125rem;
    color: var(--text-muted);
    margin: 0;
  }

  /* ── Composer ───────────────────────────────────────── */

  .cmt__composer {
    border: 1px solid var(--border);
    border-radius: 0.75rem;
    background: var(--surface);
    overflow: hidden;
    transition:
      border-color 0.18s var(--ease-out-expo),
      box-shadow 0.18s var(--ease-out-expo);
  }
  .cmt__composer:focus-within {
    border-color: var(--accent);
    box-shadow: 0 0 0 3px var(--accent-subtle);
  }

  .cmt__input {
    display: block;
    width: 100%;
    min-height: 5.25rem;
    resize: none;
    background: transparent;
    border: 0;
    outline: none;
    padding: 0.875rem 1rem 0.5rem;
    font-family: var(--font-body);
    font-size: 0.875rem;
    line-height: 1.6;
    color: var(--text);
  }
  .cmt__input::placeholder {
    color: var(--text-muted);
  }

  .cmt__toolbar {
    display: flex;
    align-items: center;
    justify-content: space-between;
    gap: 0.75rem;
    padding: 0.5rem 0.625rem 0.5rem 1rem;
    border-top: 1px solid var(--border);
  }
  .cmt__hint {
    font-size: 0.75rem;
    color: var(--text-muted);
  }
  .cmt__actions {
    display: flex;
    align-items: center;
    gap: 0.625rem;
  }
  .cmt__kbd {
    display: inline-flex;
    align-items: center;
    gap: 0.25rem;
  }
  .cmt__kbd kbd {
    display: inline-flex;
    align-items: center;
    justify-content: center;
    min-width: 1.125rem;
    height: 1.125rem;
    padding: 0 0.25rem;
    border: 1px solid var(--border);
    border-radius: 0.3125rem;
    background: var(--bg-subtle);
    color: var(--text-muted);
    font-family: var(--font-mono);
    font-size: 0.625rem;
    line-height: 1;
  }

  .cmt__send {
    display: inline-flex;
    align-items: center;
    border: 0;
    border-radius: 0.5rem;
    padding: 0.4375rem 0.875rem;
    background: var(--accent);
    color: var(--accent-text);
    font-size: 0.8125rem;
    font-weight: 600;
    transition:
      background 0.15s var(--ease-out-expo),
      opacity 0.15s var(--ease-out-expo);
  }
  .cmt__send:hover:not(:disabled) {
    background: var(--accent-hover);
  }
  .cmt__send:disabled {
    opacity: 0.4;
    cursor: not-allowed;
  }

  @media (prefers-reduced-motion: reduce) {
    .cmt__composer,
    .cmt__send {
      transition: none;
    }
  }
</style>

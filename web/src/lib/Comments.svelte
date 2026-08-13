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
  import TimeAgo from "./TimeAgo.svelte";
  import {
    listMentionCandidates,
    type Comment,
    type MentionCandidate,
    type AttachmentEntity,
  } from "./api";
  import { fuzzyMatch } from "./fuzzy";
  import { MENTION_RE } from "./mentions";
  import { MessageSquare, CornerDownLeft, Paperclip, MoreHorizontal } from "lucide-svelte";
  import { fly } from "svelte/transition";
  import { tick, onDestroy, onMount } from "svelte";
  import {
    commentTargetFromHash,
    routeWithCommentTarget,
    splitResourcePath,
  } from "./commentLinks";
  import { filesFromClipboard, insertAtCaret } from "./attachments/compose";
  import { createUploadController } from "./attachments/uploads.svelte";
  import DropOverlay from "./attachments/DropOverlay.svelte";
  import PendingUploads from "./attachments/PendingUploads.svelte";
  import { canManageComment, commentWasEdited } from "./commentState";

  let {
    comments,
    editable = true,
    onSubmit,
    currentUser = null,
    onUpdate,
    onDelete,
    placeholder = "Write a comment\u2026",
    // LIF-263: project the comments belong to. When set, the composer
    // fetches @mention candidates and offers autocomplete. Null (workspace
    // pages) disables mentions entirely.
    projectId = null,
    // LIF-262: when set, pasted/dropped/attached files link straight to this
    // entity (comment threads always know their parent). New comments haven't
    // been created yet, so uploads stay unlinked until the comment is posted
    // and its body is re-scanned server-side — we pass no id here and let that
    // re-scan record the link.
    attachTo = null,
  }: {
    comments: Comment[];
    editable?: boolean;
    onSubmit: (content: string) => Promise<Comment | null>;
    currentUser?: { id: number } | null;
    onUpdate?: (id: number, content: string) => Promise<Comment | null>;
    onDelete?: (id: number) => Promise<boolean>;
    placeholder?: string;
    projectId?: number | null;
    attachTo?: { entity_type: AttachmentEntity; entity_id: number } | null;
  } = $props();

  let draft = $state("");
  let submitting = $state(false);
  let textareaEl = $state<HTMLTextAreaElement | null>(null);
  let fileInputEl = $state<HTMLInputElement | null>(null);
  let editingId = $state<number | null>(null);
  let editDraft = $state("");
  let editTextareaEl = $state<HTMLTextAreaElement | null>(null);
  let editFileInputEl = $state<HTMLInputElement | null>(null);
  let updatingId = $state<number | null>(null);
  let deletingId = $state<number | null>(null);
  let menuId = $state<number | null>(null);
  let confirmDeleteId = $state<number | null>(null);

  let canSend = $derived(draft.trim().length > 0 && !submitting);

  let pendingCommentTarget = $state(commentTargetFromHash(window.location.hash));
  let scrollingTarget: string | null = null;

  onMount(() => {
    function onHashChange(event: HashChangeEvent) {
      pendingCommentTarget = commentTargetFromHash(new URL(event.newURL).hash);
    }
    window.addEventListener("hashchange", onHashChange);
    return () => window.removeEventListener("hashchange", onHashChange);
  });

  $effect(() => {
    const target = pendingCommentTarget;
    if (!target || scrollingTarget === target || comments.length === 0) return;
    scrollingTarget = target;
    void tick().then(() => {
      if (pendingCommentTarget !== target) {
        if (scrollingTarget === target) scrollingTarget = null;
        return;
      }
      const comment = document.getElementById(target);
      if (!comment) {
        scrollingTarget = null;
        return;
      }
      comment.scrollIntoView({ block: "center" });
      const resourcePath = splitResourcePath(window.location.pathname);
      if (resourcePath && window.location.hash === `#${target}`) {
        const route = resourcePath.route + window.location.search;
        history.replaceState(
          null,
          "",
          resourcePath.basePath +
            "/#" +
            routeWithCommentTarget(route, target),
        );
      }
      pendingCommentTarget = null;
      scrollingTarget = null;
    });
  });

  // ── @mention autocomplete (LIF-263) ─────────────────────
  //
  // Candidates are fetched once when the project id is known; the popover
  // opens when the caret sits in an `@token` run, fuzzy-filters on username
  // + display name, and is keyboard-first (↑/↓/Enter/Tab select, Esc
  // dismisses). Selecting replaces the active `@token` with `@username `.

  let candidates = $state<MentionCandidate[]>([]);

  $effect(() => {
    const pid = projectId;
    if (pid == null) {
      candidates = [];
      return;
    }
    let cancelled = false;
    listMentionCandidates(pid).then((r) => {
      if (!cancelled && r.ok) candidates = r.data;
    });
    return () => {
      cancelled = true;
    };
  });

  // Active mention query: `@` + the partial token immediately left of the
  // caret. `null` when the caret isn't in a mention context.
  let mentionOpen = $state(false);
  let mentionQuery = $state("");
  // Character offset of the `@` that started the active token.
  let mentionStart = $state(0);
  let mentionIndex = $state(0);

  // Only match an `@run` that ends exactly at the caret and begins at a word
  // boundary — mirrors the render/extract rule so the composer and the
  // stored result agree on what a token is.
  const ACTIVE_MENTION_RE = /(^|[^\w@-])@([A-Za-z0-9_-]*)$/;

  let mentionMatches = $derived.by<MentionCandidate[]>(() => {
    if (!mentionOpen) return [];
    const q = mentionQuery.trim();
    if (q === "") return candidates.slice(0, 8);
    return candidates
      .map((c) => {
        const byUser = fuzzyMatch(q, c.username);
        const byName = fuzzyMatch(q, c.display_name);
        const score = Math.max(byUser?.score ?? 0, byName?.score ?? 0);
        return { c, score };
      })
      .filter((x) => x.score > 0)
      .sort((a, b) => b.score - a.score)
      .slice(0, 8)
      .map((x) => x.c);
  });

  function updateMentionContext() {
    const el = editingId === null ? textareaEl : editTextareaEl;
    if (!el || candidates.length === 0) {
      mentionOpen = false;
      return;
    }
    const caret = el.selectionStart ?? 0;
    const activeDraft = editingId === null ? draft : editDraft;
    const before = activeDraft.slice(0, caret);
    const m = before.match(ACTIVE_MENTION_RE);
    if (m) {
      mentionOpen = true;
      mentionQuery = m[2];
      mentionStart = caret - m[2].length - 1; // index of the '@'
      mentionIndex = 0;
    } else {
      mentionOpen = false;
    }
  }

  function selectMention(cand: MentionCandidate) {
    const el = editingId === null ? textareaEl : editTextareaEl;
    if (!el) return;
    const activeDraft = editingId === null ? draft : editDraft;
    const caret = el.selectionStart ?? activeDraft.length;
    const before = activeDraft.slice(0, mentionStart);
    const after = activeDraft.slice(caret);
    const insert = `@${cand.username} `;
    if (editingId === null) draft = before + insert + after;
    else editDraft = before + insert + after;
    mentionOpen = false;
    // Restore caret just past the inserted token + trailing space.
    const nextCaret = before.length + insert.length;
    tick().then(() => {
      el.focus();
      el.setSelectionRange(nextCaret, nextCaret);
      resize();
    });
  }

  function onMentionKeydown(e: KeyboardEvent): boolean {
    if (!mentionOpen || mentionMatches.length === 0) return false;
    if (e.key === "ArrowDown") {
      e.preventDefault();
      mentionIndex = (mentionIndex + 1) % mentionMatches.length;
      return true;
    }
    if (e.key === "ArrowUp") {
      e.preventDefault();
      mentionIndex =
        (mentionIndex - 1 + mentionMatches.length) % mentionMatches.length;
      return true;
    }
    if (e.key === "Enter" || e.key === "Tab") {
      e.preventDefault();
      selectMention(mentionMatches[Math.min(mentionIndex, mentionMatches.length - 1)]);
      return true;
    }
    if (e.key === "Escape") {
      e.preventDefault();
      mentionOpen = false;
      return true;
    }
    return false;
  }

  function initialsOf(name: string): string {
    return name
      .split(/[\s_-]+/)
      .slice(0, 2)
      .map((w) => w[0]?.toUpperCase() ?? "")
      .join("");
  }

  // Keep MENTION_RE referenced so tree-shaking + lint don't complain when
  // the composer path is the only consumer in this module.
  void MENTION_RE;

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
    // The mention popover consumes ↑/↓/Enter/Tab/Esc while open.
    if (onMentionKeydown(e)) return;
    if (e.key === "Enter" && (e.metaKey || e.ctrlKey)) {
      e.preventDefault();
      if (editingId === null) submit();
      else saveEdit();
    }
    if (e.key === "Escape" && editingId !== null) {
      e.preventDefault();
      cancelEdit();
    }
  }

  function onInput() {
    resize();
    updateMentionContext();
  }

  // Grow the textarea with its content; CSS min-height floors it.
  function resize() {
    const el = editingId === null ? textareaEl : editTextareaEl;
    if (!el) return;
    el.style.height = "auto";
    el.style.height = `${el.scrollHeight}px`;
  }

  // ── LIF-262: attachment uploads ──────────────────────────

  function insertSnippet(snippet: string, edit = false) {
    const el = edit ? editTextareaEl : textareaEl;
    const activeDraft = edit ? editDraft : draft;
    if (!el) {
      const next = activeDraft + (activeDraft.endsWith("\n") || activeDraft === "" ? "" : "\n") + snippet + "\n";
      if (edit) editDraft = next;
      else draft = next;
      return;
    }
    const { text, caret } = insertAtCaret(el, activeDraft, snippet);
    if (edit) editDraft = text;
    else draft = text;
    requestAnimationFrame(() => {
      el.focus();
      el.setSelectionRange(caret, caret);
      resize();
    });
  }

  // Shared pending-upload controller (LIF-268): the same state model drives the
  // strip of chips in both this composer and the description/page editor.
  const uploads = createUploadController({
    link: () => attachTo,
    onInsert: insertSnippet,
  });
  onDestroy(() => uploads.destroy());

  const editUploads = createUploadController({
    // Link only after a successful PUT re-scans the saved Markdown. This
    // keeps Cancel and editor switches free of attachment side effects.
    link: () => null,
    onInsert: (snippet) => {
      if (editingId !== null) insertSnippet(snippet, true);
    },
  });
  onDestroy(() => editUploads.destroy());

  function onPaste(e: ClipboardEvent, edit = false) {
    const files = filesFromClipboard(e);
    if (files.length > 0) {
      e.preventDefault();
      (edit ? editUploads : uploads).enqueue(files);
    }
  }

  function onFilePicked(e: Event, edit = false) {
    const input = e.target as HTMLInputElement;
    if (input.files && input.files.length > 0) {
      (edit ? editUploads : uploads).enqueue(Array.from(input.files));
      input.value = "";
    }
  }

  function initials(name: string): string {
    return name
      .split(/[\s_-]+/)
      .slice(0, 2)
      .map((w) => w[0]?.toUpperCase() ?? "")
      .join("");
  }

  function startEdit(comment: Comment) {
    editUploads.destroy();
    editingId = comment.id;
    editDraft = comment.content;
    menuId = null;
    confirmDeleteId = null;
    tick().then(() => {
      editTextareaEl?.focus();
      resize();
    });
  }

  function cancelEdit() {
    if (updatingId !== null) return;
    editUploads.destroy();
    editingId = null;
    editDraft = "";
    mentionOpen = false;
  }

  async function saveEdit() {
    if (editingId === null || updatingId !== null || editUploads.busy || editDraft.trim() === "" || !onUpdate) return;
    const id = editingId;
    updatingId = id;
    const updated = await onUpdate(id, editDraft.trim());
    updatingId = null;
    if (updated && editingId === id) cancelEdit();
  }

  async function confirmDelete(id: number) {
    if (deletingId !== null || !onDelete) return;
    deletingId = id;
    const deleted = await onDelete(id);
    deletingId = null;
    if (deleted) {
      if (editingId === id) cancelEdit();
      menuId = null;
      confirmDeleteId = null;
    }
  }

  // LIF-159 — palette "Add comment" action: scroll the composer into
  // view and put the caret in it.
  export function focusComposer() {
    const el = textareaEl;
    if (!el) return;
    el.scrollIntoView({ behavior: "smooth", block: "center" });
    el.focus();
  }

  // LIF-111 — called by the quote-in-comment toolbar (via DocumentDetail).
  // Prepends a markdown blockquote of the selected text to the composer,
  // then focuses it, resizes, drops the caret at the end, and scrolls the
  // composer into view.
  export async function insertQuote(text: string) {
    const quoted = text.split("\n").map((l) => "> " + l).join("\n") + "\n\n";
    if (editingId === null) draft = quoted + draft;
    else editDraft = quoted + editDraft;
    await tick();
    const el = editingId === null ? textareaEl : editTextareaEl;
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
        <li id={`comment-${comment.id}`} class="cmt__item" in:fly={{ y: 6, duration: 180 }}>
          <div class="cmt__avatar" aria-hidden="true">{initials(author)}</div>
          <div class="cmt__body">
            <div class="cmt__meta">
              <a href="#comment-{comment.id}" class="cmt__anchor">#{comment.id}</a>
              <span class="cmt__author">{author}</span>
              <span class="cmt__time"><TimeAgo date={comment.created_at} /></span>
              {#if commentWasEdited(comment)}
                <span class="cmt__edited" title={`Edited ${comment.updated_at}`}>edited</span>
              {/if}
              {#if canManageComment(comment, currentUser, onUpdate !== undefined && onDelete !== undefined)}
                <div class="cmt__menu-wrap">
                  <button
                    type="button"
                    class="cmt__menu-button"
                    aria-label={`Comment ${comment.id} actions`}
                    aria-haspopup="menu"
                    aria-expanded={menuId === comment.id}
                    onclick={() => {
                      menuId = menuId === comment.id ? null : comment.id;
                      confirmDeleteId = null;
                    }}
                    onfocus={() => (menuId = comment.id)}
                    onkeydown={(e) => { if (e.key === "Escape") menuId = null; }}
                  ><MoreHorizontal size={16} /></button>
                  {#if menuId === comment.id}
                    <div class="cmt__menu" role="menu" aria-label={`Comment ${comment.id} actions`}>
                      {#if confirmDeleteId === comment.id}
                        <p>Delete this comment?</p>
                        <div class="cmt__menu-confirm">
                          <button type="button" onclick={() => (confirmDeleteId = null)} disabled={deletingId === comment.id}>Cancel</button>
                          <button type="button" class="cmt__delete" onclick={() => confirmDelete(comment.id)} disabled={deletingId === comment.id}>
                            {deletingId === comment.id ? "Deleting…" : "Delete"}
                          </button>
                        </div>
                      {:else}
                        <button type="button" role="menuitem" onclick={() => startEdit(comment)} disabled={updatingId !== null || deletingId !== null}>Edit</button>
                        <button type="button" role="menuitem" onclick={() => (confirmDeleteId = comment.id)} disabled={updatingId !== null || deletingId !== null}>Delete</button>
                      {/if}
                    </div>
                  {/if}
                </div>
              {/if}
            </div>
            {#if editingId === comment.id}
              <DropOverlay onFiles={(files) => editUploads.enqueue(files)}>
                <div class="cmt__composer cmt__editor">
                  <div class="cmt__input-wrap">
                    <textarea bind:this={editTextareaEl} bind:value={editDraft} class="cmt__input" oninput={onInput} onkeydown={onKeydown} onclick={updateMentionContext} onpaste={(e) => onPaste(e, true)}></textarea>
                    {#if mentionOpen && mentionMatches.length > 0}
                      <ul class="mention-pop" role="listbox" aria-label="Mention a user">
                        {#each mentionMatches as cand, i (cand.user_id)}
                          <li><button type="button" role="option" aria-selected={i === mentionIndex} class="mention-pop__row" class:is-active={i === mentionIndex} onmousedown={(e) => { e.preventDefault(); selectMention(cand); }} onmouseenter={() => (mentionIndex = i)}><span class="mention-pop__avatar" aria-hidden="true">{initialsOf(cand.display_name || cand.username)}</span><span class="mention-pop__text"><span class="mention-pop__name">{cand.display_name || cand.username}</span><span class="mention-pop__user">@{cand.username}</span></span></button></li>
                        {/each}
                      </ul>
                    {/if}
                  </div>
                  <PendingUploads controller={editUploads} />
                  <div class="cmt__toolbar">
                    <div class="cmt__toolbar-left"><button type="button" class="cmt__attach" onclick={() => editFileInputEl?.click()} disabled={editUploads.busy}><Paperclip size={13} /><span>{editUploads.busy ? "Uploading…" : "Attach"}</span></button><span class="cmt__hint">Markdown · drag, paste or attach files</span></div>
                    <div class="cmt__actions"><span class="cmt__kbd" aria-hidden="true"><kbd>{isMac ? "⌘" : "Ctrl"}</kbd><kbd><CornerDownLeft size={11} /></kbd></span><button type="button" class="cmt__cancel" onclick={cancelEdit} disabled={updatingId === comment.id}>Cancel</button><button type="button" class="cmt__send" onclick={saveEdit} disabled={editDraft.trim() === "" || editUploads.busy || updatingId === comment.id}>{updatingId === comment.id ? "Saving…" : "Save"}</button></div>
                  </div>
                  <input bind:this={editFileInputEl} type="file" class="cmt__file-input" multiple accept="image/*,application/pdf,text/plain,.log,application/zip" onchange={(e) => onFilePicked(e, true)} />
                </div>
              </DropOverlay>
            {:else}
              <div class="cmt__md">
                <Markdown content={comment.content} mentions={candidates} class="text-sm" />
              </div>
            {/if}
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
    <DropOverlay onFiles={(files) => uploads.enqueue(files)}>
      <div class="cmt__composer">
        <div class="cmt__input-wrap">
        <textarea
          bind:this={textareaEl}
          bind:value={draft}
          {placeholder}
          class="cmt__input"
          oninput={onInput}
          onkeydown={onKeydown}
          onclick={updateMentionContext}
          onpaste={onPaste}
          onblur={() => setTimeout(() => (mentionOpen = false), 120)}
        ></textarea>

        {#if mentionOpen && mentionMatches.length > 0}
          <ul class="mention-pop" role="listbox" aria-label="Mention a user">
            {#each mentionMatches as cand, i (cand.user_id)}
              <li>
                <button
                  type="button"
                  role="option"
                  aria-selected={i === mentionIndex}
                  class="mention-pop__row"
                  class:is-active={i === mentionIndex}
                  onmousedown={(e) => {
                    e.preventDefault();
                    selectMention(cand);
                  }}
                  onmouseenter={() => (mentionIndex = i)}
                >
                  <span class="mention-pop__avatar" aria-hidden="true">
                    {initialsOf(cand.display_name || cand.username)}
                  </span>
                  <span class="mention-pop__text">
                    <span class="mention-pop__name">{cand.display_name || cand.username}</span>
                    <span class="mention-pop__user">@{cand.username}</span>
                  </span>
                </button>
              </li>
            {/each}
          </ul>
        {:else if mentionOpen && candidates.length > 0}
          <div class="mention-pop mention-pop--empty">No people match</div>
        {/if}
        </div>

        <PendingUploads controller={uploads} />

        <div class="cmt__toolbar">
          <div class="cmt__toolbar-left">
            <button
              type="button"
              class="cmt__attach"
              title="Attach files"
              aria-label="Attach files"
              onclick={() => fileInputEl?.click()}
              disabled={uploads.busy}
            >
              <Paperclip size={13} />
              <span>{uploads.busy ? "Uploading\u2026" : "Attach"}</span>
            </button>
            <span class="cmt__hint">Markdown \u00b7 drag, paste or attach files</span>
          </div>
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
        <input
          bind:this={fileInputEl}
          type="file"
          class="cmt__file-input"
          multiple
          accept="image/*,application/pdf,text/plain,.log,application/zip"
          onchange={onFilePicked}
        />
      </div>
    </DropOverlay>
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
  .cmt__anchor {
    font-size: 0.75rem;
    font-variant-numeric: tabular-nums;
    color: var(--text-faint);
    text-decoration: none;
  }
  .cmt__anchor:hover {
    color: var(--text-muted);
    text-decoration: underline;
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
  .cmt__edited {
    font-size: 0.6875rem;
    color: var(--text-faint);
    font-style: italic;
  }
  .cmt__menu-wrap { position: relative; margin-left: auto; }
  .cmt__menu-button {
    display: grid; place-items: center; width: 1.75rem; height: 1.75rem;
    border: 0; border-radius: 0.375rem; background: transparent;
    color: var(--text-muted);
  }
  .cmt__menu-button:hover, .cmt__menu-button:focus-visible { background: var(--bg-subtle); color: var(--text); outline: none; }
  .cmt__menu {
    position: absolute; z-index: 30; right: 0; top: calc(100% + 0.25rem);
    min-width: 8rem; padding: 0.25rem; border: 1px solid var(--border);
    border-radius: 0.5rem; background: var(--surface); box-shadow: 0 8px 20px rgb(0 0 0 / 0.18);
  }
  .cmt__menu > button, .cmt__menu-confirm > button {
    border: 0; border-radius: 0.3125rem; background: transparent; color: var(--text);
    padding: 0.375rem 0.5rem; font-size: 0.75rem;
  }
  .cmt__menu > button { display: block; width: 100%; text-align: left; }
  .cmt__menu > button:hover:not(:disabled) { background: var(--bg-subtle); }
  .cmt__menu p { margin: 0; padding: 0.375rem 0.5rem; font-size: 0.75rem; color: var(--text-muted); }
  .cmt__menu-confirm { display: flex; justify-content: flex-end; gap: 0.25rem; }
  .cmt__menu-confirm .cmt__delete { color: var(--error); }
  .cmt__editor { margin-top: 0.5rem; }
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
    /* visible (not hidden) so the @mention popover can overhang the
       composer edge; children carry their own rounding where it matters. */
    overflow: visible;
    transition:
      border-color 0.18s var(--ease-out-expo),
      box-shadow 0.18s var(--ease-out-expo);
  }

  .cmt__input-wrap {
    position: relative;
    border-top-left-radius: 0.75rem;
    border-top-right-radius: 0.75rem;
    overflow: visible;
  }
  .cmt__composer:focus-within {
    border-color: var(--accent);
    box-shadow: 0 0 0 3px var(--accent-subtle);
  }
  .cmt__composer {
    position: relative;
  }

  .cmt__file-input {
    display: none;
  }

  .cmt__toolbar-left {
    display: flex;
    align-items: center;
    gap: 0.625rem;
    min-width: 0;
  }
  .cmt__attach {
    display: inline-flex;
    align-items: center;
    gap: 0.3125rem;
    padding: 0.25rem 0.5rem;
    border: 1px solid var(--border);
    border-radius: 0.375rem;
    background: transparent;
    color: var(--text-muted);
    font-size: 0.75rem;
    font-weight: 500;
    transition:
      border-color 0.15s var(--ease-out-expo),
      color 0.15s var(--ease-out-expo);
  }
  .cmt__attach:hover:not(:disabled) {
    border-color: var(--accent);
    color: var(--accent);
  }
  .cmt__attach:disabled {
    opacity: 0.6;
    cursor: not-allowed;
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
  .cmt__cancel {
    border: 0; border-radius: 0.5rem; padding: 0.4375rem 0.625rem;
    background: transparent; color: var(--text-muted); font-size: 0.8125rem; font-weight: 500;
  }
  .cmt__cancel:hover:not(:disabled) { color: var(--text); background: var(--bg-subtle); }

  /* ── @mention popover (LIF-263) ─────────────────────── */
  /* Command-Palette vocabulary: a floating surface card with a soft
     shadow, tight rows, an avatar/initials leading each row, and an
     accent-tinted active row. Anchored just below the caret line by
     sitting at the top of the textarea wrap. */
  .mention-pop {
    position: absolute;
    z-index: 40;
    left: 0.5rem;
    top: 100%;
    margin-top: 0.25rem;
    min-width: 15rem;
    max-width: 22rem;
    max-height: 15rem;
    overflow-y: auto;
    list-style: none;
    margin-block: 0.25rem 0;
    padding: 0.25rem;
    background: var(--surface);
    border: 1px solid var(--border);
    border-radius: 0.625rem;
    box-shadow:
      0 10px 24px -8px rgb(0 0 0 / 0.28),
      0 2px 6px -2px rgb(0 0 0 / 0.16);
  }
  .mention-pop--empty {
    padding: 0.625rem 0.75rem;
    font-size: 0.8125rem;
    color: var(--text-muted);
  }
  .mention-pop li {
    list-style: none;
  }
  .mention-pop__row {
    display: flex;
    align-items: center;
    gap: 0.625rem;
    width: 100%;
    padding: 0.375rem 0.5rem;
    border: 0;
    border-radius: 0.4375rem;
    background: transparent;
    text-align: left;
    cursor: pointer;
    color: var(--text);
  }
  .mention-pop__row.is-active {
    background: var(--accent-subtle);
  }
  .mention-pop__avatar {
    flex-shrink: 0;
    width: 1.5rem;
    height: 1.5rem;
    border-radius: 999px;
    display: flex;
    align-items: center;
    justify-content: center;
    background: var(--accent-subtle);
    color: var(--accent);
    border: 1px solid var(--border);
    font-size: 0.5625rem;
    font-weight: 700;
    user-select: none;
  }
  .mention-pop__text {
    display: flex;
    flex-direction: column;
    min-width: 0;
    line-height: 1.25;
  }
  .mention-pop__name {
    font-size: 0.8125rem;
    font-weight: 500;
    color: var(--text);
    white-space: nowrap;
    overflow: hidden;
    text-overflow: ellipsis;
  }
  .mention-pop__user {
    font-size: 0.6875rem;
    color: var(--text-muted);
    font-family: var(--font-mono);
  }

  @media (prefers-reduced-motion: reduce) {
    .cmt__composer,
    .cmt__send {
      transition: none;
    }
  }
</style>

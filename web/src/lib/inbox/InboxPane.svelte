<script lang="ts">
  // The reading pane: the selected inbox issue without leaving the inbox.
  // Why it is here, its waits, the description, the latest comments and a
  // reply box, plus the triage actions. The full view is one click away.
  import {
    ArrowUpRight,
    CircleCheck,
    CirclePlay,
    Clock,
    Hourglass,
    Mail,
    MailOpen,
    MessageSquare,
    Undo2,
    X,
  } from "lucide-svelte";
  import { createComment, listComments, type Comment, type Issue } from "../api";
  import Markdown from "../Markdown.svelte";
  import StatusIcon from "../StatusIcon.svelte";
  import PriorityIcon from "../PriorityIcon.svelte";
  import TimeAgo from "../TimeAgo.svelte";
  import CopyIdButton from "../CopyIdButton.svelte";
  import { toast } from "../toast/toast.svelte";
  import { describeWait } from "../issues/waits";
  import { projectCodeOf } from "../references";
  import LaterMenu from "./LaterMenu.svelte";
  import { REASON_LABEL, isInboxLater, type InboxEntry } from "./inbox";

  let {
    entry,
    projectName,
    unread,
    today,
    onOpen,
    onToggleRead,
    onLater,
    onClearWait,
    onStatus,
    onClose,
  }: {
    entry: InboxEntry;
    projectName?: string;
    unread: boolean;
    today: string | null;
    onOpen: (path: string) => void;
    onToggleRead: () => void;
    onLater: (day: string) => void;
    onClearWait: (waitId: number) => void;
    onStatus: (status: string) => void;
    onClose: () => void;
  } = $props();

  const COMMENTS_SHOWN = 3;

  const issue = $derived<Issue>(entry.issue);
  const path = $derived(`/${projectCodeOf(issue.identifier)}/issues/${issue.identifier}`);

  let comments = $state<Comment[]>([]);
  let moreComments = $state(false);
  let commentsLoading = $state(false);
  let reply = $state("");
  let sending = $state(false);
  let laterOpen = $state(false);
  let token = 0;

  // Refetch when another issue is shown, or this one changed on the server.
  $effect(() => {
    void loadComments(issue.id, issue.updated_at);
  });

  async function loadComments(id: number, _stamp: string) {
    const mine = ++token;
    commentsLoading = comments.length === 0 || comments[0]?.issue_id !== id;
    const res = await listComments(id, null, COMMENTS_SHOWN);
    if (mine !== token) return;
    commentsLoading = false;
    if (res.ok) {
      comments = res.data.items;
      moreComments = res.data.hasMore;
    } else {
      comments = [];
      moreComments = false;
    }
  }

  // A new issue in the pane starts with an empty reply box.
  let replyFor = 0;
  $effect(() => {
    if (issue.id !== replyFor) {
      replyFor = issue.id;
      reply = "";
      laterOpen = false;
    }
  });

  async function send() {
    const text = reply.trim();
    if (!text || sending) return;
    sending = true;
    const id = issue.id;
    const res = await createComment(id, text);
    sending = false;
    if (!res.ok) {
      toast(`Couldn't comment on ${issue.identifier}: ${res.error}`, { kind: "error" });
      return;
    }
    if (id === issue.id) {
      reply = "";
      comments = [...comments, res.data].slice(-COMMENTS_SHOWN);
    }
    toast(`Commented on ${issue.identifier}`, { kind: "success", duration: 2500 });
  }

  const people = $derived.by(() => {
    if (!issue.needs_human) return "";
    const list = issue.assignees ?? [];
    if (list.length === 0) return "Any person";
    return list.map((a) => a.display_name || `@${a.username}`).join(", ");
  });

  const TONE = {
    holding: "text-[var(--warn-text)]",
    due: "text-[var(--accent)]",
    overdue: "text-[var(--error)]",
  } as const;
</script>

<svelte:window onclick={() => (laterOpen = false)} />

<article class="h-full flex flex-col min-h-0" aria-label="{issue.identifier} preview" data-testid="inbox-pane">
  <header class="shrink-0 px-5 pt-4 pb-3 border-b border-[var(--border)]">
    <div class="flex items-center gap-2 text-caption text-[var(--text-faint)]">
      <span class="font-mono">{issue.identifier}</span>
      <CopyIdButton value={issue.identifier} label="ID" iconSize={12} />
      <span aria-hidden="true">·</span>
      <span class="truncate">{projectName ?? projectCodeOf(issue.identifier)}</span>
      <span class="flex-1"></span>
      <button
        type="button"
        class="pane-icon"
        aria-label="Close the reading pane"
        title="Close (Esc)"
        onclick={onClose}
      >
        <X size={14} />
      </button>
    </div>
    <h2 class="mt-1.5 text-heading font-display tracking-tight leading-snug text-[var(--text)]">
      <a href={`#${path}`} class="hover:underline" onclick={(e) => {
        if (e.metaKey || e.ctrlKey || e.button !== 0) return;
        e.preventDefault();
        onOpen(path);
      }}>{issue.title}</a>
    </h2>
    <div class="mt-2 flex flex-wrap items-center gap-x-3 gap-y-1 text-caption text-[var(--text-muted)]">
      <span class="inline-flex items-center gap-1.5 capitalize"><StatusIcon status={issue.status} size={13} />{issue.status}</span>
      <span class="inline-flex items-center gap-1.5 capitalize"><PriorityIcon priority={issue.priority} size={13} />{issue.priority === "none" ? "No priority" : issue.priority}</span>
      {#if people}<span class="truncate max-w-[220px]">For {people}</span>{/if}
      <span class="inline-flex items-center gap-1">Updated <TimeAgo date={issue.updated_at.replace(" ", "T")} /></span>
    </div>

    <!-- Actions -->
    <div class="mt-3 flex flex-wrap items-center gap-1.5">
      <button type="button" class="pane-btn pane-btn-primary" onclick={() => onOpen(path)}>
        <ArrowUpRight size={13} /> Open
      </button>
      {#if issue.status !== "active"}
        <button type="button" class="pane-btn" onclick={() => onStatus("active")}>
          <CirclePlay size={13} /> Start
        </button>
      {/if}
      <button type="button" class="pane-btn" onclick={() => onStatus("done")}>
        <CircleCheck size={13} /> Done
      </button>
      {#if entry.later}
        <button type="button" class="pane-btn" onclick={() => onClearWait(entry.later!.id)}>
          <Undo2 size={13} /> Bring back
        </button>
      {:else if today}
        <div class="relative">
          <button
            type="button"
            class="pane-btn"
            aria-expanded={laterOpen}
            onclick={(e) => {
              e.stopPropagation();
              laterOpen = !laterOpen;
            }}
          >
            <Clock size={13} /> Later
          </button>
          {#if laterOpen}
            <LaterMenu
              {today}
              align="left"
              onPick={(day) => {
                laterOpen = false;
                onLater(day);
              }}
              onClose={() => (laterOpen = false)}
            />
          {/if}
        </div>
      {/if}
      <button type="button" class="pane-btn" onclick={onToggleRead}>
        {#if unread}<MailOpen size={13} /> Mark read{:else}<Mail size={13} /> Mark unread{/if}
      </button>
    </div>
  </header>

  <div class="flex-1 min-h-0 overflow-y-auto px-5 py-4 flex flex-col gap-5">
    <!-- Why it is here -->
    <section class="flex flex-col gap-1.5">
      <h3 class="pane-label">Why it's here</h3>
      <ul class="flex flex-col gap-1 text-body-sm text-[var(--text)]">
        {#each entry.reasons as r (r)}
          <li class="flex items-center gap-2">
            <span class="size-1.5 rounded-full bg-[var(--accent)]"></span>{REASON_LABEL[r]}
          </li>
        {/each}
      </ul>
      {#if (issue.waits ?? []).length}
        <ul class="mt-1 flex flex-col gap-1">
          {#each issue.waits ?? [] as w (w.id)}
            {@const d = describeWait(w, today)}
            <li class="flex items-start gap-2 text-caption {TONE[d.state]}">
              {#if w.kind === "user"}<Hourglass size={12} class="mt-0.5 shrink-0" />{:else}<Clock size={12} class="mt-0.5 shrink-0" />{/if}
              <span class="flex-1 min-w-0">
                {isInboxLater(w) && d.state !== "holding" ? "Back from later" : d.headline}
                {#if w.note && !isInboxLater(w)}<span class="text-[var(--text-muted)]"> · {w.note}</span>{/if}
              </span>
              {#if isInboxLater(w)}
                <button type="button" class="text-micro underline text-[var(--text-muted)] hover:text-[var(--text)]" onclick={() => onClearWait(w.id)}>
                  Clear
                </button>
              {/if}
            </li>
          {/each}
        </ul>
      {/if}
    </section>

    <!-- Description -->
    <section class="flex flex-col gap-1.5">
      <h3 class="pane-label">Description</h3>
      {#if issue.description.trim()}
        <div class="pane-body text-body-sm">
          <Markdown content={issue.description} />
        </div>
      {:else}
        <p class="text-body-sm text-[var(--text-faint)] italic">No description.</p>
      {/if}
    </section>

    <!-- Latest comments -->
    <section class="flex flex-col gap-2">
      <h3 class="pane-label flex items-center gap-1.5">
        <MessageSquare size={11} /> Latest comments
      </h3>
      {#if commentsLoading}
        <div class="h-12 rounded-lg bg-[var(--bg-subtle)] animate-pulse"></div>
      {:else if comments.length === 0}
        <p class="text-body-sm text-[var(--text-faint)] italic">No comments yet.</p>
      {:else}
        {#if moreComments}
          <a href={`#${path}`} class="text-caption text-[var(--text-muted)] hover:text-[var(--text)]" onclick={(e) => {
            if (e.metaKey || e.ctrlKey) return;
            e.preventDefault();
            onOpen(path);
          }}>Earlier comments in the full view</a>
        {/if}
        {#each comments as c (c.id)}
          <div class="rounded-lg bg-[var(--bg-subtle)] px-3 py-2">
            <div class="flex items-center gap-2 text-caption">
              <span class="font-medium text-[var(--text)]">{c.author_display_name || c.author}</span>
              <TimeAgo date={c.created_at.replace(" ", "T")} class="text-micro text-[var(--text-faint)]" />
            </div>
            <div class="pane-body text-body-sm mt-1">
              <Markdown content={c.content} />
            </div>
          </div>
        {/each}
      {/if}

      <form
        class="mt-1 flex flex-col gap-1.5"
        onsubmit={(e) => {
          e.preventDefault();
          void send();
        }}
      >
        <textarea
          bind:value={reply}
          rows="2"
          placeholder="Reply to {issue.identifier}…"
          aria-label="Reply"
          class="w-full resize-y min-h-[56px] px-3 py-2 rounded-lg text-body-sm bg-[var(--bg)]
                 border border-[var(--border)] text-[var(--text)] outline-none
                 focus:border-[var(--accent)] placeholder:text-[var(--text-faint)]"
          onkeydown={(e) => {
            if (e.key === "Enter" && (e.metaKey || e.ctrlKey)) {
              e.preventDefault();
              void send();
            }
            if (e.key === "Escape") (e.currentTarget as HTMLTextAreaElement).blur();
          }}
        ></textarea>
        <div class="flex items-center justify-between">
          <span class="text-micro text-[var(--text-faint)]">Ctrl Enter to send</span>
          <button
            class="pane-btn pane-btn-primary"
            disabled={!reply.trim() || sending}
          >
            {sending ? "Sending…" : "Comment"}
          </button>
        </div>
      </form>
    </section>
  </div>
</article>

<style>
  .pane-label {
    font-size: var(--text-micro);
    text-transform: uppercase;
    letter-spacing: 0.08em;
    font-weight: 600;
    color: var(--text-faint);
  }
  .pane-btn {
    display: inline-flex;
    align-items: center;
    gap: 0.35rem;
    height: 1.75rem;
    padding: 0 0.6rem;
    border-radius: 0.4rem;
    border: 1px solid var(--border);
    background: var(--surface);
    color: var(--text);
    font-size: var(--text-caption);
    font-weight: 500;
    transition: background-color 120ms, border-color 120ms;
  }
  .pane-btn:hover:not(:disabled) {
    background: var(--bg-subtle);
  }
  .pane-btn:disabled {
    opacity: 0.45;
  }
  .pane-btn-primary {
    background: var(--accent);
    border-color: var(--accent);
    color: var(--accent-text);
  }
  .pane-btn-primary:hover:not(:disabled) {
    background: var(--accent-hover);
  }
  .pane-icon {
    display: grid;
    place-items: center;
    width: 1.5rem;
    height: 1.5rem;
    border-radius: 0.35rem;
    color: var(--text-faint);
  }
  .pane-icon:hover {
    background: var(--bg-subtle);
    color: var(--text);
  }
  .pane-body :global(p) {
    margin: 0.25rem 0;
  }
</style>

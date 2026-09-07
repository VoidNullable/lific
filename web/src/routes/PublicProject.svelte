<script lang="ts">
  // LIF-465: the anonymous project view at /public/{IDENTIFIER}.
  //
  // Outside `Layout`, which is the signed-in shell and reads a session a public
  // visitor does not have. This component is the whole page, and it reads only
  // lib/api's public helpers, which send no credential.
  import {
    getPublicIssues,
    getPublicIssue,
    getPublicComments,
    publicAttachmentUrl,
    formatBytes,
    PUBLIC_MAX_PAGES,
    type PublicProject,
    type PublicIssue,
    type PublicIssueDetail,
    type PublicComment,
    type PublicAttachment,
  } from "../lib/api";
  import PublicMarkdown from "../lib/public/PublicMarkdown.svelte";
  import StatusIcon from "../lib/StatusIcon.svelte";
  import PriorityIcon from "../lib/PriorityIcon.svelte";
  import TimeAgo from "../lib/TimeAgo.svelte";
  import ErrorState from "../lib/ErrorState.svelte";
  import Skeleton from "../lib/Skeleton.svelte";
  import { onDestroy } from "svelte";
  import { ArrowLeft, Search, Paperclip, Globe } from "lucide-svelte";

  let {
    navigate,
    projectIdentifier,
    issueIdentifier = null,
  }: {
    navigate: (path: string) => void;
    projectIdentifier: string;
    /** Present on `/public/{PROJECT}/{ISSUE}`; null on the list. */
    issueIdentifier?: string | null;
  } = $props();

  let project = $state<PublicProject | null>(null);
  let issues = $state<PublicIssue[]>([]);
  let truncated = $state(false);
  let detail = $state<PublicIssueDetail | null>(null);
  let comments = $state<PublicComment[]>([]);
  let commentTotal = $state(0);
  let moreComments = $state(false);
  let commentError = $state("");
  let loadingComments = $state(false);
  let loading = $state(true);
  let error = $state("");
  let query = $state("");

  // One message for every way a public read comes back empty-handed. The
  // server answers a private, a missing and an unpublished project identically
  // on purpose; saying more here would undo that from the client side.
  const MISSING =
    "This project isn't published. The link may be wrong, or its owner has turned public access off.";

  // Every load is stamped with a generation. A route change bumps it, so a
  // response that arrives after the reader has navigated is dropped instead of
  // overwriting the view. Checked after EVERY await, not just before the first
  // one: each await is a place the route can change under us.
  let generation = 0;
  let inFlight: AbortController | null = null;

  onDestroy(() => inFlight?.abort());

  $effect(() => {
    void load(projectIdentifier, issueIdentifier);
  });

  function beginLoad(): { id: number; signal: AbortSignal } {
    inFlight?.abort();
    inFlight = new AbortController();
    generation += 1;
    return { id: generation, signal: inFlight.signal };
  }

  async function load(ident: string, issue: string | null) {
    const { id, signal } = beginLoad();
    // Reset before fetching so a new route never shows the previous one's
    // content while its own is in flight.
    loading = true;
    error = "";
    detail = null;
    comments = [];
    commentTotal = 0;
    moreComments = false;
    commentError = "";
    loadingComments = false;
    issues = [];
    truncated = false;
    query = "";

    if (issue) {
      const res = await getPublicIssue(ident, issue, signal);
      if (id !== generation) return;
      if (!res.ok) {
        error = res.status === 404 ? MISSING : res.error;
        loading = false;
        return;
      }
      detail = res.data;
      project = res.data.project;
      loading = false;

      const first = await getPublicComments(ident, issue, 0, signal);
      if (id !== generation) return;
      if (!first.ok) {
        commentError = first.error;
        return;
      }
      comments = first.data.comments;
      commentTotal = first.data.total;
      moreComments = first.data.has_more;
      return;
    }

    // The list is paged server-side so one anonymous request is a bounded
    // amount of work. The filter box searches what the view has, so it walks
    // the pages and appends, rendering the first page before the rest arrive.
    const first = await getPublicIssues(ident, 0, signal);
    if (id !== generation) return;
    if (!first.ok) {
      error = first.status === 404 ? MISSING : first.error;
      loading = false;
      return;
    }
    project = first.data.project;
    issues = first.data.issues;
    loading = false;

    let offset = first.data.issues.length;
    let hasMore = first.data.has_more;
    for (let page = 1; hasMore && page < PUBLIC_MAX_PAGES; page += 1) {
      const next = await getPublicIssues(ident, offset, signal);
      if (id !== generation) return;
      if (!next.ok) {
        // A refusal mid-walk leaves what loaded in place and says the list is
        // partial, which beats discarding it.
        truncated = true;
        return;
      }
      if (next.data.issues.length === 0) break;
      issues = [...issues, ...next.data.issues];
      offset += next.data.issues.length;
      hasMore = next.data.has_more;
    }
    truncated = hasMore;
  }

  async function loadMoreComments() {
    if (!issueIdentifier || loadingComments) return;
    const id = generation;
    const signal = inFlight?.signal;
    loadingComments = true;
    commentError = "";
    try {
      const res = await getPublicComments(
        projectIdentifier,
        issueIdentifier,
        comments.length,
        signal,
      );
      if (id !== generation) return;
      if (!res.ok) {
        commentError = res.error;
        return;
      }
      comments = [...comments, ...res.data.comments];
      commentTotal = res.data.total;
      moreComments = res.data.has_more;
    } finally {
      if (id === generation) loadingComments = false;
    }
  }

  // Client-side only: the public list endpoint takes no query, on purpose.
  let visible = $derived(
    query.trim().length === 0
      ? issues
      : issues.filter((i) => {
          const q = query.trim().toLowerCase();
          return (
            i.title.toLowerCase().includes(q) ||
            i.identifier.toLowerCase().includes(q) ||
            i.labels.some((l) => l.toLowerCase().includes(q))
          );
        }),
  );

  let openCount = $derived(
    issues.filter((i) => i.status !== "done" && i.status !== "cancelled").length,
  );

  function statusLabel(status: string): string {
    return status.charAt(0).toUpperCase() + status.slice(1);
  }

  /** Attachments the body did not already embed, listed as downloads. */
  function fileList(attachments: PublicAttachment[], body: string) {
    return attachments.filter((a) => !body.includes(`/api/attachments/${a.id}`));
  }
</script>

<svelte:head>
  <title>{project ? `${project.name} · Lific` : "Lific"}</title>
</svelte:head>

<div class="min-h-dvh bg-[var(--bg)] text-[var(--text)]">
  <header
    class="border-b border-[var(--border)] bg-[var(--surface)]/80 backdrop-blur
           sticky top-0 z-10"
  >
    <div class="mx-auto max-w-4xl px-4 sm:px-6 py-3 flex items-center gap-3">
      {#if issueIdentifier}
        <button
          class="shrink-0 size-8 grid place-items-center rounded-md
                 text-[var(--text-muted)] hover:text-[var(--text)]
                 hover:bg-[var(--bg-subtle)] transition-colors"
          aria-label="Back to all issues"
          onclick={() => navigate(`/public/${projectIdentifier}`)}
        >
          <ArrowLeft size={16} />
        </button>
      {/if}
      <div class="min-w-0 flex-1">
        <h1 class="text-body font-semibold truncate">
          {project?.name ?? projectIdentifier}
        </h1>
        <p class="text-caption text-[var(--text-muted)] flex items-center gap-1.5">
          <Globe size={11} class="shrink-0" />
          Public read-only view
        </p>
      </div>
    </div>
  </header>

  <main class="mx-auto max-w-4xl px-4 sm:px-6 py-6">
    {#if loading}
      <div class="flex flex-col gap-3" data-testid="public-loading">
        {#each [1, 2, 3, 4] as n (n)}
          <Skeleton class="h-12 w-full rounded-lg" />
        {/each}
      </div>
    {:else if error}
      <ErrorState title="Not available" message={error} />
    {:else if detail}
      <!-- ── ISSUE DETAIL ─────────────────────────────── -->
      <article class="flex flex-col gap-5">
        <div class="flex flex-col gap-2">
          <div class="flex items-center gap-2 text-caption text-[var(--text-muted)]">
            <span class="font-mono">{detail.identifier}</span>
            <span aria-hidden="true">·</span>
            <span class="flex items-center gap-1">
              <StatusIcon status={detail.status} size={13} />
              {statusLabel(detail.status)}
            </span>
            {#if detail.priority !== "none"}
              <span aria-hidden="true">·</span>
              <span class="flex items-center gap-1">
                <PriorityIcon priority={detail.priority} size={13} />
                {statusLabel(detail.priority)}
              </span>
            {/if}
            {#if detail.module}
              <span aria-hidden="true">·</span>
              <span class="truncate">{detail.module}</span>
            {/if}
          </div>
          <h2 class="text-title font-semibold leading-tight">{detail.title}</h2>
          <p class="text-caption text-[var(--text-faint)]">
            Opened <TimeAgo date={detail.created_at} /> · updated
            <TimeAgo date={detail.updated_at} />
          </p>
          {#if detail.labels.length > 0}
            <div class="flex flex-wrap gap-1.5">
              {#each detail.labels as label (label)}
                <span
                  class="text-caption px-2 py-0.5 rounded-full border
                         border-[var(--border)] text-[var(--text-muted)]"
                >{label}</span>
              {/each}
            </div>
          {/if}
        </div>

        {#if detail.description.trim().length > 0}
          <PublicMarkdown
            content={detail.description}
            project={projectIdentifier}
            attachments={detail.attachments}
          />
        {:else}
          <p class="text-body-sm text-[var(--text-faint)] italic">
            No description.
          </p>
        {/if}

        {#if fileList(detail.attachments, detail.description).length > 0}
          <section class="flex flex-col gap-2">
            <h3 class="text-body-sm font-semibold flex items-center gap-1.5">
              <Paperclip size={13} /> Attachments
            </h3>
            <ul class="flex flex-col gap-1.5">
              {#each fileList(detail.attachments, detail.description) as file (file.id)}
                <li>
                  <a
                    class="text-body-sm text-[var(--accent)] hover:underline
                           inline-flex items-center gap-2"
                    href={publicAttachmentUrl(projectIdentifier, file.id)}
                    rel="noopener noreferrer"
                    download
                  >
                    {file.filename}
                    <span class="text-caption text-[var(--text-faint)] tabular-nums">
                      {formatBytes(file.size_bytes)}
                    </span>
                  </a>
                </li>
              {/each}
            </ul>
          </section>
        {/if}

        <section class="flex flex-col gap-4 border-t border-[var(--border)] pt-5">
          <h3 class="text-body-sm font-semibold">
            {commentTotal === 1 ? "1 comment" : `${commentTotal} comments`}
          </h3>
          {#if commentTotal === 0 && !commentError}
            <p class="text-body-sm text-[var(--text-faint)]">
              No comments on this issue.
            </p>
          {/if}
          {#each comments as comment (comment.id)}
            <div
              id="comment-{comment.id}"
              class="rounded-lg border border-[var(--border)] bg-[var(--surface)] p-3.5"
            >
              <p class="text-caption text-[var(--text-faint)] mb-2">
                <TimeAgo date={comment.created_at} />
              </p>
              <PublicMarkdown
                content={comment.content}
                project={projectIdentifier}
                attachments={comment.attachments}
              />
            </div>
          {/each}

          {#if commentError}
            <p class="text-body-sm text-[var(--error)]">
              Couldn't load comments: {commentError}
            </p>
          {/if}

          {#if moreComments}
            <button
              type="button"
              class="self-start text-body-sm font-medium px-3 py-1.5 rounded-md
                     border border-[var(--border)] hover:bg-[var(--bg-subtle)]
                     transition-colors disabled:opacity-50"
              disabled={loadingComments}
              onclick={loadMoreComments}
            >
              {loadingComments
                ? "Loading…"
                : `Load more (${commentTotal - comments.length} left)`}
            </button>
          {/if}
        </section>
      </article>
    {:else}
      <!-- ── ISSUE LIST ───────────────────────────────── -->
      {#if project && project.description.trim().length > 0}
        <p class="text-body-sm text-[var(--text-muted)] mb-5">
          {project.description}
        </p>
      {/if}

      <div class="flex items-center gap-3 mb-4">
        <div class="relative flex-1">
          <Search
            size={14}
            class="absolute left-2.5 top-1/2 -translate-y-1/2 text-[var(--text-faint)]"
          />
          <input
            id="public-search"
            type="search"
            bind:value={query}
            placeholder="Filter issues"
            aria-label="Filter issues"
            class="w-full h-9 pl-8 pr-3 rounded-md border border-[var(--border)]
                   bg-[var(--surface)] text-body-sm
                   focus:outline-none focus:border-[var(--accent)]"
          />
        </div>
        <span class="text-caption text-[var(--text-muted)] tabular-nums shrink-0">
          {openCount} open · {issues.length} total
        </span>
      </div>

      {#if visible.length === 0}
        <p class="text-body-sm text-[var(--text-faint)] py-8 text-center">
          {issues.length === 0
            ? "This project has no issues yet."
            : "No issues match that filter."}
        </p>
      {:else}
        <ul class="flex flex-col rounded-xl border border-[var(--border)] overflow-hidden">
          {#each visible as issue (issue.identifier)}
            <li class="border-b border-[var(--border)] last:border-b-0">
              <a
                href="#/public/{projectIdentifier}/{issue.identifier}"
                class="flex items-center gap-3 px-3 sm:px-4 py-2.5
                       bg-[var(--surface)] hover:bg-[var(--bg-subtle)]
                       transition-colors"
              >
                <StatusIcon status={issue.status} size={14} />
                <span
                  class="font-mono text-caption text-[var(--text-faint)]
                         tabular-nums shrink-0 hidden sm:inline"
                >{issue.identifier}</span>
                <span class="text-body-sm truncate flex-1">{issue.title}</span>
                {#if issue.labels.length > 0}
                  <span
                    class="hidden md:inline text-caption text-[var(--text-muted)] truncate max-w-40"
                  >{issue.labels.join(", ")}</span>
                {/if}
                <PriorityIcon priority={issue.priority} size={13} />
                <span
                  class="text-caption text-[var(--text-faint)] shrink-0 hidden sm:inline"
                >
                  <TimeAgo date={issue.updated_at} />
                </span>
              </a>
            </li>
          {/each}
        </ul>
      {/if}

      {#if truncated}
        <p class="text-caption text-[var(--text-faint)] mt-3">
          Only the first {issues.length} issues are shown here.
        </p>
      {/if}
    {/if}
  </main>

  <footer class="mx-auto max-w-4xl px-4 sm:px-6 pb-8 pt-2">
    <p class="text-caption text-[var(--text-faint)]">
      Published from Lific. Only current issues, their comments and the files
      they reference are shown.
    </p>
  </footer>
</div>

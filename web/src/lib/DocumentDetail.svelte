<script lang="ts">
  // LIF-123 — shared detail-page shell for IssueDetail, PageDetail and
  // PlanDetail.
  //
  // Owns the chrome they share: loading / error states, the click-to-edit
  // title, the live Markdown body, attachments, activity, the comment
  // thread, the outline of a long body, and the topbar (breadcrumb, read-only
  // note, comment count, save state, the "more" menu with copy link, export,
  // the route's own actions and delete).
  //
  // What differs is injected by the route:
  //   - data + save callbacks (the API calls differ)
  //   - `sidebar`     snippet — the issue's properties
  //   - `belowTitle`  snippet — a property strip under the title; it gets a
  //                   function that opens the details drawer on a phone
  //   - `metaFooter`  snippet — anything to close the main column with
  //
  // The body is always a live editor for people who can edit, and "E" puts
  // the caret in it.

  import Comments from "./Comments.svelte";
  import QuoteSelectionToolbar from "./QuoteSelectionToolbar.svelte";
  import EditableMarkdown from "./EditableMarkdown.svelte";
  import AttachmentSection from "./AttachmentSection.svelte";
  import InlineTitle from "./InlineTitle.svelte";
  import DeleteMenu, { type MenuItem } from "./DeleteMenu.svelte";
  import ActivityTimeline from "./ActivityTimeline.svelte";
  import ErrorState from "./ErrorState.svelte";
  import Skeleton from "./Skeleton.svelte";
  import TimeAgo from "./TimeAgo.svelte";
  import Breadcrumbs, { type Crumb } from "./Breadcrumbs.svelte";
  import { Check, Download, Link2, ListTree, MessageSquare, PanelRight, X } from "lucide-svelte";
  import { getContext, type Snippet } from "svelte";
  import type { Activity, AuthUser, Comment } from "./api";
  import type { BodySaveResult } from "./editor/bodySync";
  import { isTypingContext } from "./shortcuts";
  import { peekState } from "./issues/peek.svelte"; // LIF-248
  import { contextMenuState } from "./contextMenuState.svelte"; // LIF-248
  import { copyToClipboard } from "./clipboard";
  import { outlineOf, tableOfContents, type OutlineHeading } from "./detail/outline";
  import type { PaletteAction, PaletteContext } from "./palette";

  let {
    navigate,
    loading = false,
    error = "",
    onRetry,
    deleteNounLabel = "page",
    identifier,
    backRoute,
    backLabel,
    // LIF-286: the full breadcrumb trail (e.g. PROJ › Issues › LIF-42).
    breadcrumbSegments,
    editable = true,
    // LIF-234: commenting is Viewer-gated server-side, so a viewer who can't
    // edit can still comment. Defaults to `editable`.
    canComment,
    // Shown as a "Read-only" badge in the topbar, with this as its tooltip.
    readOnlyNote = null,
    // Title
    title,
    titleSize = "md",
    onSaveTitle,
    // Body
    body,
    // The body's write sequence and record id, for conflict-checked saves
    // (see EditableMarkdown).
    bodySeq = null,
    bodyDocId = null,
    bodyNoun = "This text",
    bodyPlaceholder = undefined,
    bodyEmptyReadText = "Nothing here yet",
    bodyProseMinHeight = "120px",
    onSaveBody,
    autofocusWhenEmpty = false,
    // Save state (route-owned): epoch ms of the last save, or null.
    saving = false,
    savedAt = null,
    // Export (optional): listed in the "more" menu.
    onExport,
    exportError = "",
    // Delete (optional): listed in the "more" menu. Deletes that can be
    // undone from their toast skip the confirm step.
    deleteNoun,
    deleteLabel = "",
    deleteConfirmBody,
    deleteUndoable = false,
    onDelete,
    // LIF-488: the route's own actions, listed above Delete.
    menuItems = [],
    // Comments (optional)
    comments,
    onNewComment,
    currentUser = null,
    onUpdateComment,
    onDeleteComment,
    hasOlderComments = false,
    loadingOlderComments = false,
    onLoadOlderComments,
    // Route-scoped identity of whatever owns this thread, so the deep-link
    // walk in Comments cannot carry state across a navigation.
    commentParentKey = "",
    // LIF-263: project id for @mention candidates. Null for workspace pages.
    mentionProjectId = null,
    // Activity timeline (optional — LIF-157).
    activity,
    // LIF-159: route-specific palette actions.
    paletteActions = [],
    // Layout: "two-column" has a properties sidebar; "wide" is a reading
    // column with the body's outline beside it on wide screens.
    layout = "two-column",
    outline = false,
    sidebar,
    belowTitle,
    metaFooter,
    // LIF-177: replaces the EditableMarkdown body (PlanDetail's step tree).
    bodyContent,
    // Extra content after the breadcrumb (IssueDetail's status chip).
    breadcrumbExtra,
    // LIF-262: link uploads to this entity and list its attachments.
    attachEntity = null,
  }: {
    navigate: (path: string) => void;
    loading?: boolean;
    error?: string;
    /** Re-run the route's load. If omitted, the error state hard-reloads. */
    onRetry?: () => void;
    /** Singular noun for the error title, e.g. "issue" / "page" / "plan". */
    deleteNounLabel?: string;
    identifier: string;
    backRoute: string;
    backLabel: string;
    breadcrumbSegments?: Crumb[];
    editable?: boolean;
    canComment?: boolean;
    readOnlyNote?: string | null;
    title: string;
    titleSize?: "md" | "lg";
    onSaveTitle: (next: string) => Promise<void> | void;
    body: string;
    bodySeq?: number | null;
    bodyDocId?: number | null;
    bodyNoun?: string;
    bodyPlaceholder?: string;
    bodyEmptyReadText?: string;
    bodyProseMinHeight?: string;
    /** Required unless `bodyContent` replaces the body. */
    onSaveBody?: (
      next: string,
      expectedSeq: number | null,
      docId: number | null,
    ) => Promise<BodySaveResult>;
    autofocusWhenEmpty?: boolean;
    saving?: boolean;
    savedAt?: number | null;
    onExport?: () => Promise<void> | void;
    exportError?: string;
    deleteNoun?: string;
    deleteLabel?: string;
    deleteConfirmBody?: string;
    deleteUndoable?: boolean;
    onDelete?: () => Promise<boolean>;
    menuItems?: MenuItem[];
    comments?: Comment[];
    onNewComment?: (content: string) => Promise<Comment | null>;
    currentUser?: AuthUser | null;
    onUpdateComment?: (id: number, content: string) => Promise<Comment | null>;
    onDeleteComment?: (id: number) => Promise<boolean>;
    hasOlderComments?: boolean;
    loadingOlderComments?: boolean;
    onLoadOlderComments?: (() => void) | undefined;
    commentParentKey?: string;
    mentionProjectId?: number | null;
    activity?: Activity[];
    paletteActions?: PaletteAction[];
    layout?: "two-column" | "wide";
    /** Show the body's outline (wide layout). */
    outline?: boolean;
    sidebar?: Snippet;
    belowTitle?: Snippet<[() => void]>;
    metaFooter?: Snippet;
    breadcrumbExtra?: Snippet;
    bodyContent?: Snippet;
    attachEntity?: { entity_type: "issue" | "page"; entity_id: number } | null;
  } = $props();

  const commentsEnabled = $derived(canComment ?? editable);

  // LIF-262: bump to force the AttachmentSection to re-fetch after a body or
  // comment save may have linked/unlinked references server-side.
  let attachmentRefresh = $state(0);

  // Comments already show in the thread, so their create and edit entries
  // would only repeat them in the history. Deletions stay: the thread can't
  // show what is gone.
  const history = $derived(
    (activity ?? []).filter(
      (a) => !(a.entity_type === "comment" && (a.action === "create" || a.action === "update")),
    ),
  );

  const commentCount = $derived(comments?.length ?? 0);

  const topbarCtx = getContext<{
    set: (s: Snippet | undefined) => void;
  } | undefined>("lific:topbar");

  $effect(() => {
    topbarCtx?.set(topbar);
    return () => topbarCtx?.set(undefined);
  });

  function copyLink() {
    void copyToClipboard(location.href.replace(/#comment-\d+$/, ""), { label: "link" });
  }

  // The "more" menu: copy and export for everyone, then the route's actions.
  const allMenuItems = $derived<MenuItem[]>([
    { label: "Copy link", icon: Link2, run: copyLink },
    ...(onExport ? [{ label: "Export as Markdown", icon: Download, run: () => void onExport?.() }] : []),
    ...(editable ? menuItems : []),
  ]);

  // LIF-159: palette actions — the route's specialized ones plus the shared
  // ones every document supports.
  const paletteCtx = getContext<PaletteContext | undefined>("lific:palette");

  $effect(() => {
    if (loading || error) {
      paletteCtx?.set(undefined);
      return;
    }
    const noun = deleteNoun ?? "document";
    const editActions: PaletteAction[] = editable
      ? [
          {
            id: "rename",
            title: `Rename ${noun}…`,
            hint: title,
            prompt: {
              placeholder: `New ${noun} title`,
              initial: title,
              submit: (v) => void onSaveTitle(v),
            },
          },
          ...(bodyContent
            ? []
            : [{ id: "edit-body", title: "Edit description", run: () => bodyRef?.focus() }]),
        ]
      : [];
    const commentActions: PaletteAction[] =
      commentsEnabled && onNewComment
        ? [{ id: "add-comment", title: "Add comment", run: () => commentsRef?.focusComposer() }]
        : [];
    const shared: PaletteAction[] = [
      { id: "copy-link", title: `Copy link to this ${noun}`, run: copyLink },
      ...(onExport ? [{ id: "export", title: "Export as Markdown", run: () => void onExport?.() }] : []),
    ];
    // Route-specific actions (status/priority/…) are edit affordances.
    const routeActions = editable ? paletteActions : [];
    paletteCtx?.set([...routeActions, ...editActions, ...commentActions, ...shared]);
    return () => paletteCtx?.set(undefined);
  });

  let bodyRef = $state<EditableMarkdown | null>(null);
  let scrollEl = $state<HTMLElement | null>(null);

  // LIF-111 — refs for the quote-in-comment selection helper.
  let contentEl = $state<HTMLElement | null>(null);
  let commentsRef = $state<Comments | null>(null);

  // LIF-226: on mobile the metadata sidebar is an off-canvas panel.
  let propsOpen = $state(false);
  const openDetails = () => (propsOpen = true);

  $effect(() => {
    identifier; // track
    propsOpen = false;
  });

  // Auto-enter edit on an empty body, once per document.
  let autofocusedFor = $state<string | null>(null);
  $effect(() => {
    if (
      autofocusWhenEmpty &&
      editable &&
      !loading &&
      !error &&
      identifier &&
      autofocusedFor !== identifier &&
      !body.trim()
    ) {
      autofocusedFor = identifier;
      requestAnimationFrame(() => bodyRef?.focus());
    }
  });

  // ── Outline ────────────────────────────────────────────────────────
  const toc = $derived(outline && !bodyContent ? tableOfContents(outlineOf(body)) : []);
  let activeHeading = $state<number | null>(null);
  let spyFrame = 0;

  function spy() {
    if (spyFrame || toc.length === 0) return;
    spyFrame = requestAnimationFrame(() => {
      spyFrame = 0;
      const tops = bodyRef?.headingTops() ?? [];
      const limit = (scrollEl?.getBoundingClientRect().top ?? 0) + 96;
      let current: number | null = null;
      for (const h of toc) {
        const top = tops[h.index];
        if (top !== undefined && top <= limit) current = h.index;
      }
      activeHeading = current ?? toc[0]?.index ?? null;
    });
  }

  $effect(() => {
    toc;
    spy();
  });

  function jumpTo(h: OutlineHeading) {
    const top = bodyRef?.headingTops()[h.index];
    if (top === undefined || !scrollEl) return;
    const offset = top - scrollEl.getBoundingClientRect().top - 24;
    scrollEl.scrollBy({ top: offset, behavior: "smooth" });
    activeHeading = h.index;
  }

  function scrollToComments() {
    contentEl?.querySelector("#detail-comments")?.scrollIntoView({ behavior: "smooth", block: "start" });
  }

  function handleKeydown(e: KeyboardEvent) {
    // LIF-248: the peek panel and the context menu own the keyboard.
    if (peekState.open || contextMenuState.open) return;
    const inField = isTypingContext();
    const plain = !e.ctrlKey && !e.metaKey && !e.altKey;

    // "E" puts the caret in the body from anywhere outside a field.
    if ((e.key === "e" || e.key === "E") && plain) {
      if (inField || !editable || bodyContent) return;
      e.preventDefault();
      bodyRef?.focus();
      return;
    }

    // "C" goes to the comment box.
    if ((e.key === "c" || e.key === "C") && plain && !e.shiftKey) {
      if (inField || !commentsEnabled || !onNewComment) return;
      e.preventDefault();
      commentsRef?.focusComposer();
      return;
    }

    // Esc backs out to the list, unless a field or a panel has it.
    if (e.key === "Escape") {
      if (inField || e.defaultPrevented) return;
      if (propsOpen) {
        e.preventDefault();
        propsOpen = false;
        return;
      }
      e.preventDefault();
      navigate(backRoute);
    }
  }
</script>

<svelte:window onkeydown={handleKeydown} />

{#if loading}
  <!-- LIF-246: skeletons match the layout the document renders into, so
       nothing reflows once data lands. -->
  <div class="h-full flex flex-col">
    <div class="flex-1 overflow-y-auto">
      <div class="flex gap-0 min-h-full">
        <div
          class="flex-1 min-w-0 {layout === 'wide'
            ? 'px-4 py-6 sm:px-10 sm:py-8'
            : 'px-4 py-5 sm:px-8 sm:py-6'}"
        >
          <div class={layout === "wide" ? "max-w-[760px] mx-auto xl:mx-0" : ""}>
            <Skeleton variant="bar" class="h-7 w-2/3 mb-6" />
            <div class="flex flex-col gap-2.5 max-w-[640px]">
              <Skeleton variant="bar" class="h-3.5 w-full" />
              <Skeleton variant="bar" class="h-3.5 w-full" />
              <Skeleton variant="bar" class="h-3.5 w-5/6" />
              <Skeleton variant="bar" class="h-3.5 w-full mt-2" />
              <Skeleton variant="bar" class="h-3.5 w-3/4" />
            </div>
          </div>
        </div>
        {#if layout === "two-column"}
          <aside class="w-[240px] xl:w-[280px] shrink-0 py-6 px-5 hidden md:flex flex-col gap-5 border-l border-[var(--border)]">
            {#each [0, 1, 2, 3] as i (i)}
              <div class="flex flex-col gap-1.5">
                <Skeleton variant="bar" class="h-2.5 w-14" />
                <Skeleton variant="bar" class="h-3.5 w-24" />
              </div>
            {/each}
          </aside>
        {/if}
      </div>
    </div>
  </div>
{:else if error}
  <ErrorState title={`Couldn't load this ${deleteNounLabel}`} message={error}>
    <button
      class="text-body-sm font-medium text-[var(--btn-success-text)] bg-[var(--btn-success)] px-3 py-1.5 rounded-md hover:bg-[var(--btn-success-hover)] transition-colors"
      onclick={() => (onRetry ? onRetry() : location.reload())}
    >
      Try again
    </button>
    <button
      class="text-body-sm text-[var(--text-muted)] border border-[var(--border)] px-3 py-1.5 rounded-md hover:bg-[var(--bg-subtle)] transition-colors"
      onclick={() => navigate(backRoute)}
    >
      Back to {backLabel.toLowerCase()}
    </button>
  </ErrorState>
{:else}
  <div class="h-full flex flex-col">
    <div class="flex-1 overflow-y-auto" bind:this={scrollEl} onscroll={spy}>
      {#if layout === "two-column"}
        <div class="flex gap-0 min-h-full">
          <div bind:this={contentEl} class="flex-1 min-w-0 px-4 py-5 sm:px-8 sm:py-6">
            {@render mainColumn()}
          </div>
          {#if sidebar}
            {#if propsOpen}
              <button
                class="md:hidden fixed inset-0 z-40 bg-black/40 backdrop-blur-[1px]"
                aria-label="Close details"
                onclick={() => (propsOpen = false)}
              ></button>
            {/if}
            <!-- Metadata sidebar: off-canvas drawer below md, docked at md+
                 (LIF-226). -->
            <aside
              class="w-[300px] md:w-[240px] xl:w-[280px] shrink-0 overflow-y-auto
                     border-l border-[var(--border)] bg-[var(--bg)] py-5 px-5
                     fixed inset-y-0 right-0 z-50 transition-transform duration-200 ease-out
                     {propsOpen ? 'translate-x-0 shadow-2xl' : 'translate-x-full'}
                     md:static md:z-auto md:translate-x-0 md:shadow-none md:transition-none"
              aria-label="Details"
            >
              <div class="md:hidden flex justify-end -mt-2 -mr-1 mb-1">
                <button
                  class="size-9 grid place-items-center rounded-md text-[var(--text-muted)]
                         hover:text-[var(--text)] hover:bg-[var(--bg-subtle)] transition-colors"
                  aria-label="Close details"
                  onclick={() => (propsOpen = false)}
                >
                  <X size={18} />
                </button>
              </div>
              {@render sidebar()}
            </aside>
          {/if}
        </div>
      {:else}
        <div class="px-4 py-6 sm:px-10 sm:py-8 flex gap-12 justify-center">
          <div bind:this={contentEl} class="flex-1 min-w-0 max-w-[760px]">
            {@render mainColumn()}
          </div>
          {#if toc.length}
            <nav class="hidden xl:block w-[220px] shrink-0" aria-label="On this page">
              <div class="sticky top-2">
                {@render outlineList()}
              </div>
            </nav>
          {/if}
        </div>
      {/if}
    </div>
  </div>

  <QuoteSelectionToolbar
    container={contentEl}
    canQuote={commentsEnabled && !!onNewComment}
    onQuote={(t) => commentsRef?.insertQuote(t)}
  />
{/if}

{#snippet outlineList()}
  <p class="flex items-center gap-1.5 mb-2 text-micro font-semibold uppercase tracking-widest text-[var(--text-faint)]">
    <ListTree size={12} /> On this page
  </p>
  {@const top = Math.min(...toc.map((h) => h.level))}
  <ol class="m-0 p-0 list-none border-l border-[var(--border)]">
    {#each toc as h (h.index)}
      <li>
        <button
          type="button"
          class="outline-item {activeHeading === h.index ? 'outline-item-on' : ''}"
          style:padding-left="{0.75 + (h.level - top) * 0.75}rem"
          aria-current={activeHeading === h.index ? "location" : undefined}
          onclick={() => jumpTo(h)}
        >
          {h.text}
        </button>
      </li>
    {/each}
  </ol>
{/snippet}

{#snippet mainColumn()}
  <InlineTitle value={title} {editable} size={titleSize} onSave={onSaveTitle} />

  {#if belowTitle}{@render belowTitle(openDetails)}{/if}

  {#if toc.length}
    <!-- Below xl the outline folds into the top of the page. -->
    <details class="xl:hidden mb-5 rounded-lg border border-[var(--border)] bg-[var(--surface)] px-3 py-2 group">
      <summary class="cursor-pointer list-none flex items-center gap-2 text-caption font-medium text-[var(--text-muted)]">
        <ListTree size={13} /> On this page
        <span class="text-micro text-[var(--text-faint)] tabular-nums">{toc.length}</span>
      </summary>
      <div class="mt-2">
        <ol class="m-0 p-0 list-none border-l border-[var(--border)]">
          {#each toc as h (h.index)}
            <li>
              <button
                type="button"
                class="outline-item"
                style:padding-left="{0.75 + (h.level - Math.min(...toc.map((x) => x.level))) * 0.75}rem"
                onclick={() => jumpTo(h)}
              >
                {h.text}
              </button>
            </li>
          {/each}
        </ol>
      </div>
    </details>
  {/if}

  {#if bodyContent}
    {@render bodyContent()}
  {:else}
    {#key identifier}
      <EditableMarkdown
        bind:this={bodyRef}
        value={body}
        seq={bodySeq}
        docId={bodyDocId}
        noun={bodyNoun}
        {editable}
        placeholder={bodyPlaceholder}
        emptyReadText={bodyEmptyReadText}
        proseMinHeight={bodyProseMinHeight}
        onSave={async (next, expectedSeq, docId) => {
          if (!onSaveBody) return { ok: false, error: "This body can't be saved here" };
          const result = await onSaveBody(next, expectedSeq, docId);
          if (result.ok) attachmentRefresh += 1;
          return result;
        }}
        attachTo={attachEntity}
        onQuote={commentsEnabled && onNewComment ? (t) => commentsRef?.insertQuote(t) : undefined}
      />
    {/key}
  {/if}

  {#if attachEntity}
    <AttachmentSection
      entityType={attachEntity.entity_type}
      entityId={attachEntity.entity_id}
      refreshKey={attachmentRefresh}
      onInsert={editable && !bodyContent ? (snippet) => bodyRef?.insertMarkdown(snippet) : undefined}
    />
  {/if}

  {#if history.length > 0}
    <ActivityTimeline items={history} />
  {/if}

  {#if onNewComment && comments}
    <div id="detail-comments" class="scroll-mt-4">
      <Comments
        bind:this={commentsRef}
        {comments}
        {currentUser}
        editable={commentsEnabled}
        onSubmit={async (content) => {
          const created = await onNewComment(content);
          if (created) attachmentRefresh += 1;
          return created;
        }}
        onUpdate={onUpdateComment}
        onDelete={onDeleteComment}
        projectId={mentionProjectId}
        hasOlder={hasOlderComments}
        loadingOlder={loadingOlderComments}
        onLoadOlder={onLoadOlderComments}
        parentKey={commentParentKey}
      />
    </div>
  {/if}

  {#if metaFooter}{@render metaFooter()}{/if}
{/snippet}

{#snippet topbar()}
  {#if !loading && !error}
    <!-- Wraps below sm only as a safety net for an unusually long trail. -->
    <div class="flex flex-wrap sm:flex-nowrap items-center gap-y-1 gap-x-2 sm:gap-3 px-3 sm:px-6 py-2 w-full">
      <div class="flex items-center gap-1.5 min-w-0">
        {#if breadcrumbSegments}
          <Breadcrumbs segments={breadcrumbSegments} />
        {:else}
          <span class="text-body-sm font-mono text-[var(--text-muted)] truncate">
            {identifier}
          </span>
        {/if}
        {#if breadcrumbExtra}{@render breadcrumbExtra()}{/if}
        {#if !editable && readOnlyNote}
          <span
            class="shrink-0 text-micro font-medium px-1.5 py-0.5 rounded-full text-[var(--text-muted)] bg-[var(--bg-subtle)]"
            title={readOnlyNote}
          >
            Read-only
          </span>
        {/if}
      </div>

      <div class="ml-auto flex items-center gap-1 sm:gap-2 shrink-0">
        {#if exportError}
          <span class="hidden sm:inline text-caption text-[var(--error)]" role="alert">{exportError}</span>
        {/if}

        <span class="hidden sm:inline-flex items-center gap-1 text-caption text-[var(--text-faint)]" aria-live="polite">
          {#if saving}
            <span class="animate-pulse">Saving…</span>
          {:else if savedAt}
            <Check size={12} class="text-[var(--success)]" />
            Saved <TimeAgo date={new Date(savedAt)} />
          {/if}
        </span>

        {#if onNewComment && comments && commentCount > 0}
          <button
            type="button"
            class="touch-target h-7 px-2 inline-flex items-center gap-1.5 rounded-md text-caption tabular-nums
                   text-[var(--text-muted)] hover:text-[var(--text)] hover:bg-[var(--bg-subtle)] transition-colors"
            title="Go to comments"
            aria-label="{commentCount} {commentCount === 1 ? 'comment' : 'comments'}, go to them"
            onclick={scrollToComments}
          >
            <MessageSquare size={14} />
            {commentCount}{hasOlderComments ? "+" : ""}
          </button>
        {/if}

        <DeleteMenu
          noun={deleteNoun ?? deleteNounLabel}
          label={deleteLabel}
          confirmBody={deleteConfirmBody}
          confirm={!deleteUndoable}
          onDelete={onDelete && deleteNoun && editable ? onDelete : undefined}
          items={allMenuItems}
          align="right"
        />

        {#if sidebar}
          <button
            class="md:hidden size-9 grid place-items-center rounded-md
                   text-[var(--text-muted)] hover:text-[var(--text)]
                   hover:bg-[var(--bg-subtle)] transition-colors"
            aria-label="Show details"
            aria-expanded={propsOpen}
            onclick={() => (propsOpen = true)}
          >
            <PanelRight size={16} />
          </button>
        {/if}
      </div>
    </div>
  {/if}
{/snippet}

<style>
  .outline-item {
    display: block;
    width: 100%;
    text-align: left;
    margin-left: -1px;
    padding-top: 0.25rem;
    padding-bottom: 0.25rem;
    padding-right: 0.5rem;
    border-left: 2px solid transparent;
    font-size: var(--text-caption);
    line-height: 1.35;
    color: var(--text-muted);
    transition: color 120ms, border-color 120ms;
  }
  .outline-item:hover {
    color: var(--text);
  }
  .outline-item-on {
    color: var(--accent);
    border-left-color: var(--accent);
  }
  details[open] summary {
    color: var(--text);
  }
</style>

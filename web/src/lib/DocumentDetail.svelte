<script lang="ts">
  // LIF-123 — shared detail-page shell for IssueDetail and PageDetail.
  //
  // Owns the chrome that both pages had copy-pasted: loading / error
  // states, the click-to-edit title, the EditableMarkdown body, the
  // comment thread, and the chrome topbar (back + breadcrumb + mode
  // toggle + save indicator + export pill + delete kebab).
  //
  // Everything that genuinely differs between the two pages is injected
  // by the route:
  //   - data + save callbacks (the API calls differ)
  //   - `sidebar`     snippet — the issue's status/priority/module/etc.
  //   - `belowTitle`  snippet — the page's label strip
  //   - `metaFooter`  snippet — the page's created/updated dates
  //
  // The route stays a thin adapter; this component is presentational +
  // owns only the body's read/edit mode (so the topbar toggle and the
  // "E" shortcut can drive EditableMarkdown without the route caring).

  import Comments from "./Comments.svelte";
  import QuoteSelectionToolbar from "./QuoteSelectionToolbar.svelte";
  import EditableMarkdown from "./EditableMarkdown.svelte";
  import ModeToggle from "./ModeToggle.svelte";
  import InlineTitle from "./InlineTitle.svelte";
  import DeleteMenu from "./DeleteMenu.svelte";
  import ActivityTimeline from "./ActivityTimeline.svelte";
  import ErrorState from "./ErrorState.svelte";
  import Skeleton from "./Skeleton.svelte";
  import { ArrowLeft, Download, PanelRight, X } from "lucide-svelte";
  import { getContext, type Snippet } from "svelte";
  import type { Activity, Comment } from "./api";
  import { isTypingContext } from "./shortcuts";
  import { peekState } from "./issues/peek.svelte"; // LIF-248
  import { contextMenuState } from "./contextMenu.svelte"; // LIF-248
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
    editable = true,
    // Title
    title,
    titleSize = "md",
    onSaveTitle,
    // Body
    body,
    bodyPlaceholder = "Start writing... (markdown supported)",
    bodyEmptyEditCta = "Click to start writing...",
    bodyEmptyReadText = "Nothing here yet",
    bodyProseMinHeight = "120px",
    onSaveBody,
    autofocusWhenEmpty = false,
    // Save indicator (route-owned state)
    saving = false,
    lastSaved = null,
    // Export (optional)
    onExport,
    exporting = false,
    exportError = "",
    // Delete (optional) — rendered as the topbar kebab
    deleteNoun,
    deleteLabel = "",
    deleteConfirmBody,
    onDelete,
    // Comments (optional)
    comments,
    onNewComment,
    // LIF-263: project id the comments belong to, used to fetch @mention
    // autocomplete candidates. Null for workspace pages (no member list).
    mentionProjectId = null,
    // Activity timeline (optional — LIF-157). The route owns fetching;
    // this shell just renders it between the body and the comments.
    activity,
    // LIF-159: route-specific palette actions (status/priority/module/
    // labels). DocumentDetail appends the shared ones (rename, edit
    // description, add comment) and registers the lot with Layout.
    paletteActions = [],
    // Layout
    layout = "two-column",
    sidebar,
    belowTitle,
    metaFooter,
    // LIF-177: when provided, this replaces the EditableMarkdown body
    // entirely (used by PlanDetail to render its step tree while keeping
    // the title / sidebar / activity / chrome). The body-editor props are
    // ignored in this mode.
    bodyContent,
    // Optional extra content rendered in the topbar breadcrumb, right
    // after the identifier (e.g. IssueDetail's status badge). PageDetail
    // omits it.
    breadcrumbExtra,
    // LIF-129: body read/edit mode, surfaced upward (bindable) so a route
    // can pause auto-refresh while the user is editing. Defaults to "read"
    // and is fully optional — IssueDetail doesn't bind it.
    bodyMode = $bindable<"read" | "edit">("read"),
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
    editable?: boolean;
    title: string;
    titleSize?: "md" | "lg";
    onSaveTitle: (next: string) => Promise<void> | void;
    body: string;
    bodyPlaceholder?: string;
    bodyEmptyEditCta?: string;
    bodyEmptyReadText?: string;
    bodyProseMinHeight?: string;
    onSaveBody: (next: string) => Promise<void> | void;
    autofocusWhenEmpty?: boolean;
    saving?: boolean;
    lastSaved?: string | null;
    onExport?: () => Promise<void> | void;
    exporting?: boolean;
    exportError?: string;
    deleteNoun?: string;
    deleteLabel?: string;
    deleteConfirmBody?: string;
    onDelete?: () => Promise<boolean>;
    comments?: Comment[];
    onNewComment?: (content: string) => Promise<Comment | null>;
    mentionProjectId?: number | null;
    activity?: Activity[];
    paletteActions?: PaletteAction[];
    layout?: "two-column" | "wide";
    sidebar?: Snippet;
    belowTitle?: Snippet;
    metaFooter?: Snippet;
    breadcrumbExtra?: Snippet;
    bodyContent?: Snippet;
    bodyMode?: "read" | "edit";
  } = $props();

  // Register our topbar with Layout's chrome slot — same pattern the
  // list/board views use, so the chrome L stays seamless across
  // list → detail transitions.
  const topbarCtx = getContext<{
    set: (s: Snippet | undefined) => void;
  } | undefined>("lific:topbar");

  $effect(() => {
    topbarCtx?.set(topbar);
    return () => topbarCtx?.set(undefined);
  });

  // LIF-159: register palette actions — the route's specialized ones
  // plus the shared trio every document supports. Re-registers whenever
  // the title or route actions change so hints stay current.
  const paletteCtx = getContext<PaletteContext | undefined>("lific:palette");

  $effect(() => {
    if (!editable || loading || error) {
      paletteCtx?.set(undefined);
      return;
    }
    const noun = deleteNoun ?? "document";
    const shared: PaletteAction[] = [
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
        : [
            {
              id: "edit-body",
              title: "Edit description",
              run: () => bodyRef?.focus(),
            },
          ]),
      ...(onNewComment
        ? [
            {
              id: "add-comment",
              title: "Add comment",
              run: () => commentsRef?.focusComposer(),
            },
          ]
        : []),
    ];
    paletteCtx?.set([...paletteActions, ...shared]);
    return () => paletteCtx?.set(undefined);
  });

  // Body read/edit mode is a bindable prop (LIF-129) so a route can read
  // it to pause auto-refresh mid-edit; it still drives EditableMarkdown
  // via the topbar toggle + "E" shortcut here.
  let bodyRef = $state<EditableMarkdown | null>(null);

  // LIF-111 — refs for the quote-in-comment selection helper. `contentEl`
  // scopes which selections count (the main content column); `commentsRef`
  // lets the toolbar push a blockquote into the composer.
  let contentEl = $state<HTMLElement | null>(null);
  let commentsRef = $state<Comments | null>(null);

  // LIF-226: on mobile the metadata sidebar is an off-canvas panel toggled
  // from the topbar (it's statically docked at md+). This tracks its open
  // state; meaningless at md+.
  let propsOpen = $state(false);

  // Reset to read mode + close the mobile props panel whenever we switch
  // documents.
  $effect(() => {
    identifier; // track
    bodyMode = "read";
    propsOpen = false;
  });

  // Auto-enter edit on an empty body, once per document. Keyed on the
  // identifier so navigation re-arms it; the guard prevents re-renders
  // from yanking focus back mid-session.
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

  function handleKeydown(e: KeyboardEvent) {
    // LIF-248: the peek panel and the right-click context menu are now
    // mountable on top of ANY route (previously peek only existed inside
    // IssueList, so this path was unreachable from here). Both own their
    // own Escape handling — without this guard, opening peek from one of
    // this route's relation/anchor chips and then pressing Escape would
    // fire BOTH PeekPanel's closePeek() and this handler's navigate(away),
    // and "E" would silently start editing the document behind the peek's
    // scrim. Bail out entirely while either owns the keyboard, same
    // shape as lib/shortcuts.ts's shortcutsSuppressed().
    if (peekState.open || contextMenuState.open) return;

    // LIF-245: shared with every other keydown handler in the app (was a
    // locally duplicated computation) — see lib/shortcuts.ts.
    const inField = isTypingContext();

    // "E" enters edit mode for the body from anywhere outside a field.
    if ((e.key === "e" || e.key === "E") && !e.ctrlKey && !e.metaKey && !e.altKey) {
      if (inField || !editable || bodyMode !== "read") return;
      e.preventDefault();
      bodyRef?.focus();
      return;
    }

    // Esc backs out to the list, unless we're mid-edit or inside a field
    // (the title input / body textarea handle their own Esc to cancel).
    if (e.key === "Escape") {
      if (inField || bodyMode === "edit") return;
      // The mobile props panel intercepts Esc first — close it instead of
      // navigating away.
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
  <!-- LIF-246: title bar + paragraph blocks, matching whichever layout
       this document will render into (two-column reserves the sidebar's
       width so nothing reflows once data lands). -->
  <div class="h-full flex flex-col">
    <div class="flex-1 overflow-y-auto">
      <div class="max-w-[1120px] mx-auto flex gap-0 min-h-full">
        <div class="flex-1 min-w-0 px-4 py-5 sm:px-8 sm:py-6">
          <Skeleton variant="bar" class="h-7 w-2/3 mb-6" />
          <div class="flex flex-col gap-2.5 max-w-[640px]">
            <Skeleton variant="bar" class="h-3.5 w-full" />
            <Skeleton variant="bar" class="h-3.5 w-full" />
            <Skeleton variant="bar" class="h-3.5 w-5/6" />
            <Skeleton variant="bar" class="h-3.5 w-full mt-2" />
            <Skeleton variant="bar" class="h-3.5 w-3/4" />
          </div>
        </div>
        {#if layout === "two-column"}
          <aside class="w-[280px] sm:w-[300px] md:w-[220px] shrink-0 py-6 px-5 hidden md:flex flex-col gap-5">
            {#each [0, 1, 2] as i (i)}
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
    <div class="flex-1 overflow-y-auto">
      {#if layout === "two-column"}
        <div class="max-w-[1120px] mx-auto flex gap-0 min-h-full">
          <div bind:this={contentEl} class="flex-1 min-w-0 px-4 py-5 sm:px-8 sm:py-6">
            {@render mainColumn()}
          </div>
          {#if sidebar}
            <!-- Mobile backdrop for the off-canvas props panel. -->
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
              class="w-[280px] sm:w-[300px] md:w-[220px] shrink-0 overflow-y-auto
                     border-l border-[var(--border)] bg-[var(--bg)] py-6 px-5
                     fixed inset-y-0 right-0 z-50 transition-transform duration-200 ease-out
                     {propsOpen ? 'translate-x-0 shadow-2xl' : 'translate-x-full'}
                     md:static md:z-auto md:w-[220px] md:translate-x-0 md:shadow-none md:transition-none"
            >
              <!-- In-drawer close affordance (mobile only). -->
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
        <div bind:this={contentEl} class="px-4 py-6 sm:px-10 sm:py-8">
          {@render mainColumn()}
        </div>
      {/if}
    </div>
  </div>

  {#if editable && onNewComment && bodyMode === "read"}
    <QuoteSelectionToolbar
      container={contentEl}
      onQuote={(t) => commentsRef?.insertQuote(t)}
    />
  {/if}
{/if}

{#snippet mainColumn()}
  <InlineTitle value={title} {editable} size={titleSize} onSave={onSaveTitle} />

  {#if belowTitle}{@render belowTitle()}{/if}

  {#if bodyContent}
    {@render bodyContent()}
  {:else}
    <EditableMarkdown
      bind:this={bodyRef}
      bind:mode={bodyMode}
      value={body}
      {editable}
      {saving}
      placeholder={bodyPlaceholder}
      emptyEditCta={bodyEmptyEditCta}
      emptyReadText={bodyEmptyReadText}
      proseMinHeight={bodyProseMinHeight}
      onSave={onSaveBody}
    />
  {/if}

  {#if activity && activity.length > 0}
    <ActivityTimeline items={activity} />
  {/if}

  {#if onNewComment && comments}
    <Comments
      bind:this={commentsRef}
      {comments}
      {editable}
      onSubmit={onNewComment}
      projectId={mentionProjectId}
    />
  {/if}

  {#if metaFooter}{@render metaFooter()}{/if}
{/snippet}

{#snippet topbar()}
  {#if !loading && !error}
    <div class="flex items-center gap-2 sm:gap-3 px-3 sm:px-6 py-2 w-full">
      <!-- Left zone: scope -->
      <div class="flex items-center gap-1.5 shrink-0 min-w-0">
        <button
          class="flex items-center gap-1.5 text-body-sm text-[var(--text-muted)]
                 hover:text-[var(--text)] transition-colors rounded px-1.5 py-0.5
                 hover:bg-[var(--bg-subtle)]"
          onclick={() => navigate(backRoute)}
        >
          <ArrowLeft size={14} class="shrink-0" />
          <span class="hidden sm:inline">{backLabel}</span>
        </button>
        <span class="text-[var(--text-faint)]">/</span>
        <span class="text-body-sm font-mono text-[var(--text-muted)] truncate">
          {identifier}
        </span>
        {#if breadcrumbExtra}{@render breadcrumbExtra()}{/if}
      </div>

      <!-- Right zone: mode toggle + save indicator + export + menu -->
      <div class="ml-auto flex items-center gap-2 shrink-0">
        {#if exportError}
          <span class="hidden sm:inline text-caption text-[var(--error)]">{exportError}</span>
        {/if}

        {#if editable && body.trim() && !bodyContent}
          <ModeToggle
            mode={bodyMode}
            size="sm"
            disabled={saving}
            onSelect={(next) => bodyRef?.setMode(next)}
          />
        {/if}

        <span class="hidden sm:inline text-caption text-[var(--text-faint)] sm:min-w-[5rem] text-right">
          {#if saving}
            <span class="animate-pulse">Saving...</span>
          {:else if lastSaved}
            Saved at {lastSaved}
          {/if}
        </span>

        {#if onExport}
          <button class="toolbar-pill" onclick={onExport} disabled={exporting}>
            <Download size={14} />
            {exporting ? "Exporting..." : "Export"}
          </button>
        {/if}

        {#if onDelete && deleteNoun && editable}
          <DeleteMenu
            noun={deleteNoun}
            label={deleteLabel}
            confirmBody={deleteConfirmBody}
            {onDelete}
            align="right"
          />
        {/if}

        <!-- Props panel toggle (mobile only). Opens the off-canvas metadata
             sidebar that's statically docked at md+ (LIF-226). -->
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

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
  import { ArrowLeft, Download } from "lucide-svelte";
  import { getContext, type Snippet } from "svelte";
  import type { Comment } from "./api";

  let {
    navigate,
    loading = false,
    error = "",
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
    // Layout
    layout = "two-column",
    sidebar,
    belowTitle,
    metaFooter,
  }: {
    navigate: (path: string) => void;
    loading?: boolean;
    error?: string;
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
    layout?: "two-column" | "wide";
    sidebar?: Snippet;
    belowTitle?: Snippet;
    metaFooter?: Snippet;
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

  // Body read/edit mode lives here so the topbar toggle + "E" shortcut
  // can drive the same EditableMarkdown instance.
  let bodyMode = $state<"read" | "edit">("read");
  let bodyRef = $state<EditableMarkdown | null>(null);

  // LIF-111 — refs for the quote-in-comment selection helper. `contentEl`
  // scopes which selections count (the main content column); `commentsRef`
  // lets the toolbar push a blockquote into the composer.
  let contentEl = $state<HTMLElement | null>(null);
  let commentsRef = $state<Comments | null>(null);

  // Reset to read mode whenever we switch documents.
  $effect(() => {
    identifier; // track
    bodyMode = "read";
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
    const el = document.activeElement;
    const inField =
      !!el &&
      (el.tagName === "INPUT" ||
        el.tagName === "TEXTAREA" ||
        el.tagName === "SELECT" ||
        (el as HTMLElement).isContentEditable);

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
      e.preventDefault();
      navigate(backRoute);
    }
  }
</script>

<svelte:window onkeydown={handleKeydown} />

{#if loading}
  <div class="h-full flex items-center justify-center">
    <div
      class="size-6 rounded-full border-2 border-[var(--border)]
             border-t-[var(--accent)] animate-spin"
    ></div>
  </div>
{:else if error}
  <div class="h-full flex flex-col items-center justify-center gap-3">
    <p class="text-[var(--error)] text-[0.875rem]">{error}</p>
    <button
      class="text-[0.8125rem] text-[var(--accent)] hover:underline"
      onclick={() => navigate(backRoute)}
    >
      Back to {backLabel.toLowerCase()}
    </button>
  </div>
{:else}
  <div class="h-full flex flex-col">
    <div class="flex-1 overflow-y-auto">
      {#if layout === "two-column"}
        <div class="max-w-[1120px] mx-auto flex gap-0 min-h-full">
          <div bind:this={contentEl} class="flex-1 min-w-0 px-8 py-6">
            {@render mainColumn()}
          </div>
          {#if sidebar}
            <aside
              class="w-[220px] shrink-0 border-l border-[var(--border)] py-6 px-5"
            >
              {@render sidebar()}
            </aside>
          {/if}
        </div>
      {:else}
        <div bind:this={contentEl} class="px-10 py-8">
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

  {#if onNewComment && comments}
    <Comments bind:this={commentsRef} {comments} {editable} onSubmit={onNewComment} />
  {/if}

  {#if metaFooter}{@render metaFooter()}{/if}
{/snippet}

{#snippet topbar()}
  {#if !loading && !error}
    <div class="flex items-center gap-3 px-6 py-2 w-full">
      <!-- Left zone: scope -->
      <div class="flex items-center gap-1.5 shrink-0">
        <button
          class="flex items-center gap-1.5 text-[0.8125rem] text-[var(--text-muted)]
                 hover:text-[var(--text)] transition-colors rounded px-1.5 py-0.5
                 hover:bg-[var(--bg-subtle)]"
          onclick={() => navigate(backRoute)}
        >
          <ArrowLeft size={14} />
          {backLabel}
        </button>
        <span class="text-[var(--text-faint)]">/</span>
        <span class="text-[0.8125rem] font-mono text-[var(--text-muted)]">
          {identifier}
        </span>
      </div>

      <!-- Right zone: mode toggle + save indicator + export + menu -->
      <div class="ml-auto flex items-center gap-2 shrink-0">
        {#if exportError}
          <span class="text-[0.75rem] text-[var(--error)]">{exportError}</span>
        {/if}

        {#if editable && body.trim()}
          <ModeToggle
            mode={bodyMode}
            size="sm"
            disabled={saving}
            onSelect={(next) => bodyRef?.setMode(next)}
          />
        {/if}

        <span class="text-[0.75rem] text-[var(--text-faint)] min-w-[5rem] text-right">
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
      </div>
    </div>
  {/if}
{/snippet}

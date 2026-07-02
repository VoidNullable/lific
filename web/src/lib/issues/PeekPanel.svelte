<script lang="ts">
  // LIF-244 — issue peek panel: a slide-over preview of an issue from the
  // list/board, without leaving the route. LIF-248: mounted once in
  // Layout.svelte (available on every authenticated route, not just
  // list/board) and driven by the peek.svelte.ts module singleton, so any
  // row/card/chip can open it via `openPeek(identifier)` without a prop
  // chain.
  //
  // Deliberately NOT a re-render of IssueDetail: no activity timeline, no
  // export/delete, no full comment thread — just enough to preview and
  // triage without a route change. "Open full view →" hands off to the
  // real detail route for everything else.
  //
  // Positioning: the panel slides in via Svelte's `fly` transition (like
  // Toaster.svelte) rather than a persistent `translate-x` Tailwind class.
  // This matters beyond aesthetics — a persistent CSS `transform` on an
  // ancestor establishes a new containing block for `position: fixed`
  // descendants (CSS spec), which would silently break Select.svelte's
  // fixed-position dropdown menu (it computes menu coordinates via
  // getBoundingClientRect(), i.e. real viewport coordinates, and expects
  // `position: fixed` to honor those relative to the viewport — not
  // relative to a transformed ancestor). Svelte's transition-driven
  // `transform` only exists as a CSS animation during the ~200ms
  // open/close and is removed once it settles, so at rest (the only time
  // a dropdown can be open) there's no transform in the ancestor chain and
  // Select behaves exactly as it does everywhere else in the app.
  import {
    resolveIssue,
    updateIssue,
    listModules,
    listLabels,
    listComments,
    type Issue,
    type Module,
    type Label,
  } from "../api";
  import { peekState, closePeek, notifyPeekSync } from "./peek.svelte";
  import { updateIssueWithUndo } from "./state.svelte";
  import { STATUSES, PRIORITIES } from "./grouping";
  import { projectCodeOf } from "../references";
  import StatusIcon from "../StatusIcon.svelte";
  import PriorityIcon from "../PriorityIcon.svelte";
  import ProjectIcon from "../ProjectIcon.svelte";
  import InlineTitle from "../InlineTitle.svelte";
  import Markdown from "../Markdown.svelte";
  import Select from "../Select.svelte";
  import { formatDate } from "../format";
  import { motionReduced } from "../theme";
  import { fly, fade } from "svelte/transition";
  import { X, Copy, Check, ArrowUpRight, MessageSquare, Layers } from "lucide-svelte";

  let {
    navigate,
  }: {
    navigate: (path: string) => void;
  } = $props();

  let issue = $state<Issue | null>(null);
  let modules = $state<Module[]>([]);
  let labels = $state<Label[]>([]);
  let commentCount = $state<number | null>(null);
  let loading = $state(false);
  let error = $state("");
  let copied = $state(false);

  // Guards against a stale fetch (for a since-superseded identifier)
  // landing after a newer one — "opening another issue while open swaps
  // content in place" means identifier can change while a fetch is still
  // in flight.
  let loadToken = 0;

  $effect(() => {
    const ident = peekState.identifier;
    if (!ident) {
      issue = null;
      return;
    }
    loadIssue(ident);
  });

  async function loadIssue(identifier: string) {
    const token = ++loadToken;
    loading = true;
    error = "";
    commentCount = null;
    const res = await resolveIssue(identifier);
    if (token !== loadToken) return;
    if (!res.ok) {
      // Covers both "deleted underneath us" and "never existed" — the
      // panel shows a quiet error instead of stale content.
      error = res.error;
      issue = null;
      loading = false;
      return;
    }
    issue = res.data;
    loading = false;

    const [modRes, lblRes, cmtRes] = await Promise.all([
      listModules(res.data.project_id),
      listLabels(res.data.project_id),
      listComments(res.data.id),
    ]);
    if (token !== loadToken) return;
    if (modRes.ok) modules = modRes.data;
    if (lblRes.ok) labels = lblRes.data;
    if (cmtRes.ok) commentCount = cmtRes.data.length;
  }

  // ── Mutations ─────────────────────────────────────────
  // Status/priority/module route through the shared undo layer (LIF-243) —
  // same toast + single-shot Undo as every other entry point. `onApplied`
  // updates this panel's own copy of `issue` AND forwards the patch to the
  // caller (IssueList) so the row/card behind the scrim reflects it live.

  // Bumped after every mutation attempt settles (success or failure) and
  // mixed into each Select's `{#key}` below. On success `issue.status` /
  // `.priority` / `.module_id` also changed, so the key would remount
  // anyway — but on failure `updateIssueWithUndo` never calls `onApplied`,
  // meaning the field prop never changes even though Select already
  // flipped its own internal display optimistically at click-time (before
  // the PUT resolved). Without `resyncTick` in the key, that failure would
  // leave the control silently stuck showing a value nothing ever
  // confirmed until the next unrelated remount. ProjectMembers.svelte's
  // role picker has the identical fix for the identical reason.
  let resyncTick = $state(0);

  async function applyMeta(patch: Record<string, unknown>, prevPatch: Record<string, unknown>) {
    if (!issue) return;
    const id = issue.id;
    const identifier = issue.identifier;
    await updateIssueWithUndo({
      id,
      identifier,
      patch,
      prevPatch,
      modules,
      onApplied: (applied) => {
        if (issue && issue.id === id) {
          issue = { ...issue, ...(applied as Partial<Issue>) };
        }
        notifyPeekSync(id, applied);
      },
    });
    resyncTick++;
  }

  function setStatus(value: string) {
    if (issue && value !== issue.status) applyMeta({ status: value }, { status: issue.status });
  }
  function setPriority(value: string) {
    if (issue && value !== issue.priority) applyMeta({ priority: value }, { priority: issue.priority });
  }
  function setModule(id: number | null) {
    if (issue && id !== issue.module_id) applyMeta({ module_id: id }, { module_id: issue.module_id });
  }

  // Title isn't a one-click-reversible value (it's free text), so it skips
  // the undo layer — mirrors IssueDetail's saveField for the same field.
  async function saveTitle(next: string) {
    if (!issue) return;
    const id = issue.id;
    const res = await updateIssue(id, { title: next });
    if (res.ok) {
      if (issue && issue.id === id) issue = res.data;
      notifyPeekSync(id, { title: next });
    }
  }

  async function copyIdentifier() {
    if (!issue) return;
    try {
      await navigator.clipboard.writeText(issue.identifier);
      copied = true;
      window.setTimeout(() => { copied = false; }, 1500);
    } catch {
      // Clipboard blocked (permissions/insecure context) — silently no-op,
      // same tradeoff ProjectSettings' copy button makes.
    }
  }

  function openFullView() {
    if (!issue) return;
    const path = `/${projectCodeOf(issue.identifier)}/issues/${issue.identifier}`;
    closePeek();
    navigate(path);
  }

  function moduleEmoji(id: number | null): string | null {
    if (id == null) return null;
    return modules.find((m) => m.id === id)?.emoji ?? null;
  }

  const statusOptions = STATUSES.map((s) => ({
    value: s,
    label: s[0].toUpperCase() + s.slice(1),
  }));
  const priorityOptions = PRIORITIES.map((p) => ({
    value: p,
    label: p === "none" ? "No priority" : p[0].toUpperCase() + p.slice(1),
  }));
  let moduleOptions = $derived([
    { value: null as number | null, label: "No module" },
    ...modules.map((m) => ({ value: m.id as number | null, label: m.name })),
  ]);

  // ── Panel transition ─────────────────────────────────
  // Slides from the right on md+ (side panel), from the bottom on mobile
  // (bottom sheet) — see the mobile-vs-desktop layout classes below.
  // Respects the appearance system's reduced-motion setting like every
  // other transition in the app (Toaster, etc).
  function isMobileViewport(): boolean {
    return typeof window !== "undefined" && window.innerWidth < 768;
  }
  function panelInParams() {
    if (motionReduced()) return { duration: 0 };
    return isMobileViewport() ? { y: 480, duration: 240 } : { x: 480, duration: 240 };
  }
  function panelOutParams() {
    if (motionReduced()) return { duration: 0 };
    return isMobileViewport() ? { y: 480, duration: 180 } : { x: 480, duration: 180 };
  }
  function scrimParams() {
    return motionReduced() ? { duration: 0 } : { duration: 180 };
  }

  function handleKeydown(e: KeyboardEvent) {
    if (!peekState.open) return;
    if (e.key === "Escape") {
      e.preventDefault();
      closePeek();
    }
  }
</script>

<!-- `<svelte:window>` must be a top-level tag (can't live inside the
     `{#if peekState.open}` block below), so the open-guard lives inside
     handleKeydown instead — this listener is a permanent no-op cost while
     the panel is closed. -->
<svelte:window onkeydown={handleKeydown} />

{#if peekState.open}
  <!-- svelte-ignore a11y_no_static_element_interactions a11y_click_events_have_key_events -->
  <!-- Deliberately doesn't touch body/html overflow — the list/board
       behind the scrim stays exactly as scrollable as it was; peek is a
       lightweight preview, not a scroll-locking modal. -->
  <div
    class="fixed inset-0 z-[90] bg-black/30 backdrop-blur-[1px]"
    onclick={closePeek}
    transition:fade={scrimParams()}
  ></div>

  <div
    class="fixed z-[95] flex flex-col bg-[var(--surface)] shadow-2xl
           inset-x-0 bottom-0 h-[85vh] rounded-t-xl border-t border-[var(--border)]
           md:inset-y-0 md:right-0 md:left-auto md:bottom-auto
           md:h-full md:w-[480px] md:max-w-[92vw]
           md:rounded-none md:border-t-0 md:border-l"
    in:fly={panelInParams()}
    out:fly={panelOutParams()}
    role="dialog"
    aria-modal="true"
    aria-label={issue ? `${issue.identifier} preview` : "Issue preview"}
  >
    <!-- Drag-handle visual (mobile bottom sheet only — decorative, scrim
         click is the close affordance, not a drag gesture). -->
    <div class="md:hidden flex justify-center pt-2 pb-1 shrink-0">
      <div class="h-1 w-9 rounded-full bg-[var(--border)]"></div>
    </div>

    <!-- Header: identifier + copy + close. Stays put while the body below
         scrolls. -->
    <div class="shrink-0 flex items-center gap-2 px-4 pt-2 pb-2 md:pt-4 border-b border-[var(--border)]">
      {#if issue}
        <button
          class="group inline-flex items-center gap-1 text-caption font-mono font-semibold
                 px-1.5 py-0.5 rounded border border-[var(--border)] text-[var(--text-muted)]
                 hover:border-[var(--accent)] hover:text-[var(--accent)] transition-colors"
          onclick={copyIdentifier}
          title="Copy identifier"
        >
          {issue.identifier}
          {#if copied}<Check size={11} />{:else}<Copy size={11} class="opacity-0 group-hover:opacity-100 transition-opacity" />{/if}
        </button>
        {#if commentCount !== null && commentCount > 0}
          <span class="inline-flex items-center gap-1 text-caption text-[var(--text-faint)]">
            <MessageSquare size={12} />
            {commentCount}
          </span>
        {/if}
      {/if}
      <div class="flex-1"></div>
      <button
        class="size-7 flex items-center justify-center rounded-md
               text-[var(--text-faint)] hover:text-[var(--text)]
               hover:bg-[var(--bg-subtle)] transition-colors"
        aria-label="Close preview"
        onclick={closePeek}
      >
        <X size={16} />
      </button>
    </div>

    <!-- Body: its own scroll region, independent of the list behind it. -->
    <div class="flex-1 overflow-y-auto px-4 py-4">
      {#if loading && !issue}
        <div class="flex items-center justify-center py-16">
          <div class="size-6 rounded-full border-2 border-[var(--border)] border-t-[var(--accent)] animate-spin"></div>
        </div>
      {:else if error}
        <div class="flex flex-col items-center gap-2 py-16 text-center">
          <p class="text-body-sm text-[var(--text-muted)]">Couldn't load this issue.</p>
          <p class="text-caption text-[var(--text-faint)]">{error}</p>
        </div>
      {:else if issue}
        <InlineTitle value={issue.title} size="md" onSave={saveTitle} />

        <!-- Status / priority / module. Each Select is keyed by its
             current server value so a failed mutation (updateIssueWithUndo
             returns without calling onApplied) or an Undo re-mounts the
             control from truth instead of leaving it stuck on an
             optimistic value nothing ever confirmed — same fix
             ProjectMembers.svelte uses for its role picker. -->
        <div class="flex flex-wrap items-center gap-2 mb-4">
          {#key `${issue.status}:${resyncTick}`}
            <Select
              options={statusOptions}
              value={issue.status}
              onchange={(opt) => setStatus(String(opt.value))}
              size="sm"
              class="w-auto"
            >
              {#snippet renderSelected(opt)}
                <span class="flex items-center gap-1.5 text-body-sm text-[var(--text)]">
                  <StatusIcon status={String(opt.value)} size={13} />
                  {opt.label}
                </span>
              {/snippet}
              {#snippet renderOption(opt, isSelected)}
                <span class="flex items-center gap-2 text-body-sm">
                  <StatusIcon status={String(opt.value)} size={13} />
                  <span class={isSelected ? "text-[var(--accent)] font-medium" : "text-[var(--text)]"}>{opt.label}</span>
                </span>
              {/snippet}
            </Select>
          {/key}

          {#key `${issue.priority}:${resyncTick}`}
            <Select
              options={priorityOptions}
              value={issue.priority}
              onchange={(opt) => setPriority(String(opt.value))}
              size="sm"
              class="w-auto"
            >
              {#snippet renderSelected(opt)}
                <span class="flex items-center gap-1.5 text-body-sm text-[var(--text)]">
                  <PriorityIcon priority={String(opt.value)} size={13} />
                  {opt.label}
                </span>
              {/snippet}
              {#snippet renderOption(opt, isSelected)}
                <span class="flex items-center gap-2 text-body-sm">
                  <PriorityIcon priority={String(opt.value)} size={13} />
                  <span class={isSelected ? "text-[var(--accent)] font-medium" : "text-[var(--text)]"}>{opt.label}</span>
                </span>
              {/snippet}
            </Select>
          {/key}

          {#if modules.length > 0}
            {#key `${issue.module_id}:${resyncTick}`}
              <Select
                options={moduleOptions}
                value={issue.module_id}
                onchange={(opt) => setModule(opt.value as number | null)}
                size="sm"
                class="w-auto"
              >
                {#snippet renderSelected(opt)}
                  <span class="flex items-center gap-1.5 text-body-sm text-[var(--text)]">
                    {#if moduleEmoji(opt.value as number | null)}
                      <ProjectIcon value={moduleEmoji(opt.value as number | null)} size={13} class="shrink-0" />
                    {:else}
                      <Layers size={13} class="text-[var(--text-faint)] shrink-0" />
                    {/if}
                    {opt.label}
                  </span>
                {/snippet}
                {#snippet renderOption(opt, isSelected)}
                  <span class="flex items-center gap-2 text-body-sm {isSelected ? 'text-[var(--accent)] font-medium' : 'text-[var(--text)]'}">
                    {#if moduleEmoji(opt.value as number | null)}
                      <ProjectIcon value={moduleEmoji(opt.value as number | null)} size={13} class="shrink-0" />
                    {:else}
                      <Layers size={13} class="text-[var(--text-faint)] shrink-0" />
                    {/if}
                    {opt.label}
                  </span>
                {/snippet}
              </Select>
            {/key}
          {/if}
        </div>

        <!-- Labels — read-only chips (edit lives in the full detail view). -->
        {#if issue.labels.length > 0}
          <div class="flex flex-wrap gap-1.5 mb-4">
            {#each issue.labels as lbl}
              {@const labelObj = labels.find((l) => l.name === lbl)}
              <span
                class="text-caption font-medium px-2 py-0.5 rounded-full border"
                style={labelObj
                  ? `color: ${labelObj.color}; border-color: ${labelObj.color}40; background: ${labelObj.color}10;`
                  : "border-color: var(--border);"}
              >
                {lbl}
              </span>
            {/each}
          </div>
        {/if}

        <!-- Relations — free on the wire (resolveIssue already returns
             these arrays), so shown whenever present. Compact chips, not
             the full labeled sections IssueDetail's sidebar uses — this is
             a preview, not the editor. -->
        {#if (issue.blocked_by && issue.blocked_by.length > 0) || (issue.blocks && issue.blocks.length > 0) || (issue.relates_to && issue.relates_to.length > 0)}
          <div class="flex flex-wrap items-center gap-1.5 mb-4 text-caption">
            {#each issue.blocked_by ?? [] as rel}
              <span class="font-mono text-[var(--error)] bg-[var(--error-bg)] px-1.5 py-0.5 rounded" title="Blocked by {rel}">
                ⛔ {rel}
              </span>
            {/each}
            {#each issue.blocks ?? [] as rel}
              <span class="font-mono text-[var(--accent)] bg-[var(--accent-subtle)] px-1.5 py-0.5 rounded" title="Blocks {rel}">
                → {rel}
              </span>
            {/each}
            {#each issue.relates_to ?? [] as rel}
              <span class="font-mono text-[var(--text-muted)] bg-[var(--bg-subtle)] px-1.5 py-0.5 rounded" title="Related to {rel}">
                {rel}
              </span>
            {/each}
          </div>
        {/if}

        <div class="border-t border-[var(--border)] -mx-4 mb-4"></div>

        <!-- Description — rendered, read-only (hover cards on any
             auto-linked identifiers come free from Markdown.svelte). -->
        {#if issue.description}
          <Markdown content={issue.description} />
        {:else}
          <p class="text-body-sm text-[var(--text-faint)] italic">No description</p>
        {/if}

        <p class="text-caption text-[var(--text-faint)] mt-4">
          Updated {formatDate(issue.updated_at)}
        </p>
      {/if}
    </div>

    <!-- Footer: hand off to the real editor for everything peek doesn't
         cover. -->
    {#if issue}
      <div class="shrink-0 border-t border-[var(--border)] px-4 py-3 flex items-center justify-between gap-2">
        <button
          class="inline-flex items-center gap-1 text-body-sm text-[var(--text-muted)]
                 hover:text-[var(--text)] transition-colors"
          onclick={copyIdentifier}
        >
          {#if copied}<Check size={13} />{:else}<Copy size={13} />{/if}
          Copy identifier
        </button>
        <button
          class="inline-flex items-center gap-1.5 text-body-sm font-medium
                 text-[var(--accent)] hover:underline transition-colors"
          onclick={openFullView}
        >
          Open full view
          <ArrowUpRight size={14} />
        </button>
      </div>
    {/if}
  </div>
{/if}

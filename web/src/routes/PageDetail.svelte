<script lang="ts">
  import {
    getPage,
    updatePage,
    deletePage,
    restorePage,
    downloadPageExport,
    listPageComments,
    createPageComment,
    updateComment,
    deleteComment,
    me,
    listLabels,
    listFolders,
    listPageActivity,
    type Page,
    type Label,
    type Folder,
    type Activity,
    type AuthUser,
  } from "../lib/api";
  import DocumentDetail from "../lib/DocumentDetail.svelte";
  import type { BodySaveResult } from "../lib/editor/bodySync";
  import LabelEditor from "../lib/LabelEditor.svelte";
  import Select from "../lib/Select.svelte";
  import TimeAgo from "../lib/TimeAgo.svelte";
  import { recordRecent } from "../lib/home/recents"; // LIF-237
  import { getContext } from "svelte";
  import { pageTitle, PAGE_TITLE_CONTEXT } from "../lib/pageTitle";
  import { startAutoRefresh } from "../lib/autoRefresh.svelte";
  import { projectRole, loadProjectRole, ensureMeAdmin } from "../lib/projectRole.svelte"; // LIF-234
  import { toast } from "../lib/toast/toast.svelte"; // LIF-284
  import { inPublicScope } from "../lib/publicScope"; // LIF-471
  import { CommentThread } from "../lib/detail/commentThread.svelte";
  import { readingMinutes, wordCount } from "../lib/detail/outline";
  import { PenLine, CircleDot, CheckCircle2, Archive, Pin } from "lucide-svelte";

  // LIF-112: page lifecycle statuses.
  const PAGE_STATUSES = [
    { value: "draft", label: "Draft", icon: PenLine },
    { value: "active", label: "Active", icon: CircleDot },
    { value: "complete", label: "Complete", icon: CheckCircle2 },
    { value: "archived", label: "Archived", icon: Archive },
  ] as const;

  const statusOptions = PAGE_STATUSES.map((s) => ({ value: s.value, label: s.label }));

  function statusMeta(value: string) {
    return PAGE_STATUSES.find((s) => s.value === value) ?? PAGE_STATUSES[0];
  }

  let {
    navigate,
    projectIdentifier,
    pageId,
    editable: editableProp,
  }: {
    navigate: (path: string) => void;
    projectIdentifier: string;
    pageId: number;
    editable?: boolean;
  } = $props();

  let page = $state<Page | null>(null);

  const titleCtx = getContext<{ set: (title?: string) => void } | undefined>(PAGE_TITLE_CONTEXT);
  $effect(() => {
    if (!page) return;
    titleCtx?.set(pageTitle(page.identifier, page.title, projectIdentifier));
  });

  // LIF-234: a project page follows that project's role; a workspace page
  // (no project) is admin-only.
  const isWorkspacePage = $derived(page != null && page.project_id === null);
  const editable = $derived(
    editableProp ?? (isWorkspacePage ? projectRole.canEditWorkspacePage : projectRole.canEdit),
  );
  const canComment = $derived(isWorkspacePage ? projectRole.canEditWorkspacePage : projectRole.canComment);

  let currentUser = $state<AuthUser | null>(null);
  let activity = $state<Activity[]>([]);
  // LIF-105: project labels; empty for workspace pages.
  let labels = $state<Label[]>([]);
  // LIF-286: folders, only to name this page's folder in the breadcrumb.
  let folders = $state<Folder[]>([]);
  let loading = $state(true);
  let error = $state("");

  void me().then((res) => {
    if (res.ok) currentUser = res.data;
  });

  let saving = $state(false);
  let savedAt = $state<number | null>(null);

  let exportError = $state("");
  let exporting = false;

  // Bumped on every navigation; a load started under an older generation
  // drops its result, so a slow response can't paint over a newer page.
  let loadGen = 0;

  /** Whether `parentId` is still the page on screen. Both halves matter:
   *  `pageId` flips the instant the reader navigates, while `page` keeps
   *  the previous document until the next one resolves. */
  function isCurrentPage(parentId: number): boolean {
    return pageId === parentId && page?.id === parentId;
  }

  const thread = new CommentThread({
    list: listPageComments,
    create: createPageComment,
    update: updateComment,
    remove: deleteComment,
    gen: () => loadGen,
    currentId: () => page?.id ?? null,
    isCurrent: isCurrentPage,
  });

  $effect(() => {
    const id = pageId;
    savedAt = null;
    loadPage(id);
  });

  // LIF-129: focus and realtime refresh, no interval. The body editor merges
  // a refreshed body into whatever is being typed.
  async function refreshPage() {
    const gen = loadGen;
    const id = pageId;
    const [res, , actRes] = await Promise.all([getPage(id), thread.load(id, gen, true), listPageActivity(id)]);
    if (gen !== loadGen) return;
    if (res.ok) page = res.data;
    if (actRes.ok) activity = actRes.data.items;
  }

  $effect(() =>
    startAutoRefresh({
      refresh: refreshPage,
      isBusy: () => saving || loading,
      intervalMs: 0,
      shouldRefresh: (event) =>
        event.type === "resync.required" ||
        (event.type.startsWith("project.") &&
          typeof event.project_id === "number" &&
          event.project_id === page?.project_id),
    }),
  );

  async function loadPage(id: number) {
    const gen = ++loadGen;
    thread.reset();
    loading = true;
    error = "";
    activity = [];
    labels = [];
    folders = [];
    const res = await getPage(id);
    if (gen !== loadGen) return;
    if (!res.ok) {
      error = res.error;
      loading = false;
      return;
    }
    page = res.data;
    if (page.project_id !== null) loadProjectRole(page.project_id);
    else ensureMeAdmin();
    recordRecent({ type: "page", routeId: String(page.id), identifier: page.identifier, title: page.title, project: projectIdentifier }); // LIF-237

    const p = page;
    const tasks: Promise<unknown>[] = [
      thread.load(p.id, gen),
      listPageActivity(p.id).then((r) => {
        if (gen === loadGen && r.ok) activity = r.data.items;
      }),
    ];
    if (p.project_id !== null) {
      tasks.push(listLabels(p.project_id).then((r) => { if (gen === loadGen && r.ok) labels = r.data; }));
      if (p.folder_id !== null) {
        tasks.push(listFolders(p.project_id).then((r) => { if (gen === loadGen && r.ok) folders = r.data; }));
      }
    }
    await Promise.all(tasks);
    if (gen !== loadGen) return;
    loading = false;
  }

  // ── Save ─────────────────────────────────────────────

  function markSaved(id: number) {
    savedAt = Date.now();
    void listPageActivity(id).then((r) => {
      if (r.ok && page?.id === id) activity = r.data.items;
    });
  }

  async function saveField(patch: Record<string, unknown>) {
    if (!page) return;
    const id = page.id;
    saving = true;
    const res = await updatePage(id, patch);
    saving = false;
    if (!res.ok) {
      toast(`Couldn't save ${page?.identifier ?? "the page"}: ${res.error}`, { kind: "error" });
      return;
    }
    if (page?.id === id) {
      page = res.data;
      markSaved(id);
    }
  }

  /** Save the body, guarded by `expected_seq` (merged or reported as a
   *  conflict by the editor, never overwritten). */
  async function saveBody(next: string, expectedSeq: number | null, docId: number | null): Promise<BodySaveResult> {
    const id = docId ?? page?.id;
    if (id == null) return { ok: false, error: "No page loaded" };
    const onScreen = () => page?.id === id;
    saving = true;
    const res = await updatePage(id, {
      content: next,
      ...(expectedSeq != null ? { expected_seq: expectedSeq } : {}),
    });
    saving = false;
    if (res.ok) {
      if (onScreen()) {
        page = res.data;
        markSaved(id);
      }
      return { ok: true, body: res.data.content, seq: res.data.seq ?? null };
    }
    if (res.status === 409) {
      const fresh = await getPage(id);
      if (fresh.ok) {
        if (onScreen()) page = fresh.data;
        return { ok: false, conflict: { body: fresh.data.content, seq: fresh.data.seq ?? null } };
      }
    }
    return { ok: false, error: res.error };
  }

  function setStatus(next: string) {
    if (page && next !== page.status) void saveField({ status: next });
  }

  function togglePin() {
    if (page) void saveField({ pinned: !page.pinned });
  }

  // LIF-105: the backend replaces the whole set, so send all of it.
  async function toggleLabel(name: string) {
    if (!page) return;
    const next = page.labels.includes(name) ? page.labels.filter((l) => l !== name) : [...page.labels, name];
    await saveField({ labels: next });
  }

  async function exportMarkdown() {
    if (!page || exporting) return;
    exporting = true;
    exportError = "";
    const res = await downloadPageExport(page.identifier);
    if (!res.ok) exportError = res.error;
    exporting = false;
  }

  /** Delete, leave, and offer Undo: a deleted page waits in the trash. */
  async function handleDelete(): Promise<boolean> {
    if (!page) return false;
    const { id, identifier } = page;
    const res = await deletePage(id);
    if (!res.ok) {
      toast(`Couldn't delete ${identifier}: ${res.error}`, { kind: "error" });
      return false;
    }
    navigate(`/${projectIdentifier}/pages`);
    toast(`Deleted ${identifier}`, {
      kind: "success",
      action: {
        label: "Undo",
        fn: async () => {
          const back = await restorePage(id);
          if (!back.ok) {
            toast(`Couldn't restore ${identifier}: ${back.error}`, { kind: "error" });
            return;
          }
          navigate(`/${projectIdentifier}/pages/${id}`);
        },
      },
    });
    return true;
  }

  // LIF-286: PROJ › Pages › (Folder ›) LIF-DOC-N.
  const breadcrumbSegments = $derived.by<import("../lib/Breadcrumbs.svelte").Crumb[]>(() => {
    const crumbs: import("../lib/Breadcrumbs.svelte").Crumb[] = [
      { label: projectIdentifier, href: `#/${projectIdentifier}/overview`, mono: true, hideBelowSm: true, copy: projectIdentifier },
      { label: "Pages", href: `#/${projectIdentifier}/pages`, hideBelowSm: true },
    ];
    if (page?.folder_id != null) {
      const name = folders.find((f) => f.id === page!.folder_id)?.name;
      if (name) crumbs.push({ label: name, href: `#/${projectIdentifier}/pages` });
    }
    // `copy` stays unset until the page loads.
    crumbs.push({ label: page?.identifier || "Page", mono: true, copy: page?.identifier });
    return crumbs;
  });

  const words = $derived(page ? wordCount(page.content) : 0);

  // ── LIF-159: palette actions ─────────────────────────
  const paletteActions = $derived.by<import("../lib/palette").PaletteAction[]>(() => {
    if (!page) return [];
    const p = page;
    return [
      {
        id: "set-status",
        title: "Set status…",
        hint: p.status,
        children: () =>
          PAGE_STATUSES.map((s) => ({
            title: s.label,
            hint: s.value === p.status ? "current" : undefined,
            run: () => setStatus(s.value),
          })),
      },
      { id: "toggle-pin", title: p.pinned ? "Unpin page" : "Pin page", run: togglePin },
      ...(p.project_id !== null && labels.length > 0
        ? [
            {
              id: "toggle-label",
              title: "Add or remove label…",
              hint: p.labels.length > 0 ? p.labels.join(", ") : undefined,
              children: () =>
                labels.map((l) => ({
                  title: l.name,
                  color: l.color,
                  hint: p.labels.includes(l.name) ? "remove" : "add",
                  run: () => void toggleLabel(l.name),
                })),
            },
          ]
        : []),
    ];
  });

  const readOnlyNote = $derived(
    inPublicScope()
      ? "Read-only. This is the public view of the project; sign in to edit or comment."
      : isWorkspacePage
        ? "Workspace pages can only be edited by an admin."
        : "You're a viewer on this project. You can still comment.",
  );
</script>

<DocumentDetail
  {navigate}
  {loading}
  {error}
  deleteNounLabel="page"
  onRetry={() => loadPage(pageId)}
  identifier={page?.identifier ?? ""}
  attachEntity={page ? { entity_type: "page", entity_id: page.id } : null}
  backRoute={`/${projectIdentifier}/pages`}
  backLabel="Pages"
  {breadcrumbSegments}
  {editable}
  {canComment}
  {readOnlyNote}
  title={page?.title ?? ""}
  titleSize="lg"
  onSaveTitle={(next) => saveField({ title: next })}
  body={page?.content ?? ""}
  bodySeq={page?.seq ?? null}
  bodyDocId={page?.id ?? null}
  bodyNoun="This page"
  bodyPlaceholder="Start writing…  Type / for headings, lists, tables and more."
  bodyEmptyReadText="Empty page"
  bodyProseMinHeight="120px"
  onSaveBody={saveBody}
  {saving}
  {savedAt}
  onExport={inPublicScope() ? undefined : exportMarkdown}
  {exportError}
  deleteNoun="page"
  deleteLabel={page?.identifier ?? ""}
  deleteUndoable
  onDelete={handleDelete}
  comments={thread.comments}
  onNewComment={(c, kind) => thread.create(c, kind)}
  decisionsOnly={thread.decisionsOnly}
  onFilterDecisions={(enabled) => thread.filterDecisions(enabled)}
  {currentUser}
  onUpdateComment={(id, c) => thread.update(id, c)}
  onDeleteComment={(id) => thread.remove(id)}
  hasOlderComments={thread.hasOlder}
  loadingOlderComments={thread.loadingOlder}
  onLoadOlderComments={() => thread.loadOlder()}
  commentParentKey={`page:${pageId}`}
  mentionProjectId={page?.project_id ?? null}
  {activity}
  {paletteActions}
  layout="wide"
  outline
>
  {#snippet belowTitle()}
    <!-- LIF-112 + LIF-105 + LIF-183: status, pin and labels, then how long
         the page is and when it last changed. -->
    {#if page}
      <div class="mb-7 flex flex-col gap-2.5">
        <div class="flex flex-wrap items-center gap-2">
          {#if editable}
            <Select options={statusOptions} value={page.status} onchange={(o) => setStatus(String(o.value))} size="sm" class="w-auto">
              {#snippet renderSelected(opt)}
                {@const meta = statusMeta(String(opt.value))}
                <span class="flex items-center gap-1.5 text-body-sm text-[var(--text)]">
                  <meta.icon size={13} class="shrink-0 text-[var(--text-muted)]" />
                  {meta.label}
                </span>
              {/snippet}
              {#snippet renderOption(opt, isSelected)}
                {@const meta = statusMeta(String(opt.value))}
                <span class="flex items-center gap-2 text-body-sm {isSelected ? 'font-medium' : ''}">
                  <meta.icon size={13} class="shrink-0 {isSelected ? 'text-[var(--accent)]' : 'text-[var(--text-muted)]'}" />
                  <span class={isSelected ? "text-[var(--accent)]" : "text-[var(--text)]"}>{meta.label}</span>
                </span>
              {/snippet}
            </Select>
            <button
              class="size-8 grid place-items-center rounded-md border transition-colors
                     {page.pinned
                ? 'text-[var(--accent)] border-[var(--accent)] bg-[var(--accent-subtle)]'
                : 'text-[var(--text-muted)] border-[var(--border)] hover:bg-[var(--bg-subtle)] hover:text-[var(--text)]'}"
              title={page.pinned ? "Pinned to the top of the page list. Click to unpin." : "Pin to the top of the page list"}
              aria-label={page.pinned ? "Unpin page" : "Pin page"}
              aria-pressed={page.pinned}
              onclick={togglePin}
              disabled={saving}
            >
              <Pin size={14} class={page.pinned ? "fill-current" : ""} />
            </button>
          {:else}
            {@const meta = statusMeta(page.status)}
            <span class="flex items-center gap-1.5 text-body-sm text-[var(--text-muted)]">
              <meta.icon size={13} class="shrink-0" />
              {meta.label}
            </span>
            {#if page.pinned}
              <span class="flex items-center gap-1 text-caption text-[var(--accent)]"><Pin size={12} class="fill-current" /> Pinned</span>
            {/if}
          {/if}

          {#if page.project_id !== null}
            <LabelEditor
              attached={page.labels}
              all={labels}
              {editable}
              onToggle={toggleLabel}
              emptyText="No labels"
              emptyItalic
              hideEmptyWhenEditable
              popoverWidth="w-[200px]"
              emptyPickerText="No labels defined in this project."
            />
          {/if}
        </div>
        <p class="page-meta m-0 flex flex-wrap items-center text-caption text-[var(--text-faint)]" data-testid="page-meta">
          <span>Updated <TimeAgo date={page.updated_at.replace(" ", "T")} class="text-[var(--text-muted)]" /></span>
          <span>Created <TimeAgo date={page.created_at.replace(" ", "T")} /></span>
          {#if words > 0}
            <span>{words.toLocaleString()} {words === 1 ? "word" : "words"}</span>
            <span>{readingMinutes(words)} min read</span>
          {/if}
        </p>
      </div>
    {/if}
  {/snippet}
</DocumentDetail>

<style>
  .page-meta > span + span::before {
    content: "·";
    margin: 0 0.45em;
  }
</style>

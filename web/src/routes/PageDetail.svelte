<script lang="ts">
  import {
    getPage,
    updatePage,
    deletePage,
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
    type Comment,
    type Label,
    type Folder,
    type Activity,
    type AuthUser,
  } from "../lib/api";
  import DocumentDetail from "../lib/DocumentDetail.svelte";
  import LabelEditor from "../lib/LabelEditor.svelte";
  import Select from "../lib/Select.svelte";
  import { formatDate } from "../lib/format";
  import { recordRecent } from "../lib/home/recents"; // LIF-237
  import { startAutoRefresh } from "../lib/autoRefresh.svelte";
  import { projectRole, loadProjectRole, ensureMeAdmin } from "../lib/projectRole.svelte"; // LIF-234
  import { toast } from "../lib/toast/toast.svelte"; // LIF-284
  import {
    COMMENT_WINDOW_RETRY_LIMIT,
    commentOpIsCurrent,
    commentWindowOutcome,
    loadCommentWindow,
    olderCursor,
    prependOlderComments,
    reconcileCommentWindow,
    removeComment,
    upsertComment,
  } from "../lib/commentState";
  import {
    PenLine,
    CircleDot,
    CheckCircle2,
    Archive,
    Pin,
  } from "lucide-svelte";

  // LIF-112: page lifecycle statuses. Icon + label per value, used by
  // the status picker in the belowTitle strip.
  const PAGE_STATUSES = [
    { value: "draft", label: "Draft", icon: PenLine },
    { value: "active", label: "Active", icon: CircleDot },
    { value: "complete", label: "Complete", icon: CheckCircle2 },
    { value: "archived", label: "Archived", icon: Archive },
  ] as const;

  const statusOptions = PAGE_STATUSES.map((s) => ({
    value: s.value,
    label: s.label,
  }));

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

  // LIF-234: role-aware gating. A page with a project follows that project's
  // role (maintainer+ edits, viewer read-only, viewer may still comment). A
  // workspace page (project_id === null) is admin-only once enforcement is
  // on, mirroring the server (authz::require_workspace_admin). `editableProp`
  // remains an optional hard override.
  const isWorkspacePage = $derived(page != null && page.project_id === null);
  const editable = $derived(
    editableProp ??
      (isWorkspacePage ? projectRole.canEditWorkspacePage : projectRole.canEdit),
  );
  const canComment = $derived(
    isWorkspacePage ? projectRole.canEditWorkspacePage : projectRole.canComment,
  );

  let comments = $state<Comment[]>([]);
  // The thread on screen is a contiguous run ending at the newest comment.
  // The cursor for the page before it comes from `comments[0]`, never from a
  // row count: offsets move under a thread that is still being written to.
  let hasOlderComments = $state(false);
  let loadingOlderComments = $state(false);
  let currentUser = $state<AuthUser | null>(null);
  let activity = $state<Activity[]>([]);
  // LIF-105: project labels available for attachment. Stays empty for
  // workspace pages (project_id === null) — labels are project-scoped.
  let labels = $state<Label[]>([]);
  // LIF-286: project folders, fetched only when this page lives in one, so the
  // breadcrumb can show PROJ › Pages › Folder › Title. Workspace pages (no
  // project) and root pages skip the fetch.
  let folders = $state<Folder[]>([]);
  let loading = $state(true);
  let error = $state("");

  void me().then((res) => {
    if (res.ok) currentUser = res.data;
  });

  // Save indicator
  let saving = $state(false);
  let lastSaved = $state<string | null>(null);

  // Export
  let exportError = $state("");
  let exporting = $state(false);

  // Request-generation guard. Bumped on every navigation (pageId change);
  // any load/refresh started under an older generation discards its result
  // so a slow response can't stomp a newer page's data. This is what kills
  // the "switching pages loads the wrong/old page" race.
  let loadGen = 0;
  // Orders the operations that all replace or extend `comments` within one
  // route: the initial window, a background refresh, a manual older page, and
  // the local fold after a mutation. Last started wins; see
  // `commentOpIsCurrent`.
  let commentOp = 0;

  // Bumped by every successful local create, edit or delete. A replacement
  // refresh captures it when it starts and refuses to land if it changed,
  // because that refresh read the thread before the edit existed and applying
  // it now would undo work the user just did and watched succeed.
  let commentMutationEpoch = 0;

  /** Take ownership of the comment window for a new operation.
   *
   *  Only the two operations that rewrite or extend the whole window claim it:
   *  a replacement refresh and a manual older page. Claiming cancels whatever
   *  held it, which is why the spinner comes down here, since the superseded
   *  operation is forbidden from touching it. Mutations deliberately do not
   *  claim: they are surgical, idempotent, and must never cancel a page the
   *  reader asked for. */
  function claimCommentOp(): number {
    loadingOlderComments = false;
    commentOp += 1;
    return commentOp;
  }

  $effect(() => {
    const id = pageId;
    lastSaved = null;
    loadPage(id);
  });

  // ── LIF-129: auto-refresh ────────────────────────────
  // Focus-only (no interval): the page body is an inline editor, so a
  // periodic poll mid-read is more disruptive than it's worth. Refetching
  // when the tab regains focus covers the real case — the agent edited a
  // page while you were elsewhere. We never refetch while editing or
  // while a save is in flight, so unsaved keystrokes can't be clobbered.
  // `bodyMode` is bound up from DocumentDetail's EditableMarkdown.
  let bodyMode = $state<"read" | "edit">("read");

  // Refresh the page *currently routed to* (pageId), not whatever `page`
  // happens to hold — and drop the result if navigation moved on while
  // the request was in flight.
  async function refreshPage() {
    const gen = loadGen;
    // Pin the routed id for the whole refresh, so every request and the retry
    // below all name the same page even though `pageId` is reactive.
    const parentId = pageId;
    // A refresh replaces the whole window, so any manual older page in flight
    // is obsolete from here on. The mutation epoch is captured rather than
    // claimed: this read reflects the thread as it is now, and any edit the
    // user makes while it is in flight is newer truth than it holds.
    const op = claimCommentOp();
    const epoch = commentMutationEpoch;
    const loadedRows = comments.length;
    const [res, commentsRes, actRes] = await Promise.all([
      getPage(parentId),
      // Reconcile every loaded page, not only the newest, so an older comment
      // edited or deleted elsewhere stops being frozen on screen.
      loadCommentWindow((before, size) => listPageComments(parentId, before, size), loadedRows),
      listPageActivity(parentId),
    ]);
    if (gen !== loadGen) return; // navigated away mid-flight — discard
    if (res.ok) page = res.data;
    // The comment window is guarded separately, so the rest of the page still
    // updates even when the thread has moved on: a manual older page may have
    // taken the window over, or a mutation may have landed an edit this read
    // predates.
    if (commentsRes.ok) {
      const outcome = commentWindowOutcome(
        { route: gen, op, epoch },
        loadGen,
        commentOp,
        commentMutationEpoch,
      );
      if (outcome === "apply") {
        // Anything the reader loaded past the refresh's row budget sits below
        // the refreshed window and is kept rather than discarded.
        const merged = reconcileCommentWindow(
          { items: comments, hasOlder: hasOlderComments },
          commentsRes.data,
        );
        comments = merged.items;
        hasOlderComments = merged.hasOlder;
      } else if (outcome === "retry") {
        void restabilizeCommentWindow(parentId, gen);
      }
    }
    if (actRes.ok) activity = actRes.data.items;
  }

  /// Re-read the newest window after a mutation landed while a replacement was
  /// already in flight.
  ///
  /// That read predates the edit, so it cannot be applied, but discarding it
  /// outright is what leaves a thread showing nothing but the row the mutation
  /// folded in: navigate away and back with a write still pending and the
  /// replacement for the second visit is invalidated by that write. Read again
  /// against the epoch the mutation established.
  ///
  /// An async loop, never recursion on the synchronous stack, and capped: if
  /// mutations keep landing faster than a window can be read, the locally
  /// folded rows stay visible and the next focus or realtime refresh
  /// reconciles them. Giving up is the correct end state, storming is not.
  async function restabilizeCommentWindow(parentId: number, gen: number) {
    for (let attempt = 0; attempt < COMMENT_WINDOW_RETRY_LIMIT; attempt += 1) {
      if (gen !== loadGen || !isCurrentPage(parentId)) return;
      const token = { route: gen, op: claimCommentOp(), epoch: commentMutationEpoch };
      const loadedRows = comments.length;
      const res = await loadCommentWindow(
        (before, size) => listPageComments(parentId, before, size),
        loadedRows,
      );
      if (!res.ok) return;
      const outcome = commentWindowOutcome(token, loadGen, commentOp, commentMutationEpoch);
      if (outcome === "abandon") return;
      if (outcome === "retry") continue;
      const merged = reconcileCommentWindow(
        { items: comments, hasOlder: hasOlderComments },
        res.data,
      );
      comments = merged.items;
      hasOlderComments = merged.hasOlder;
      return;
    }
  }

  async function loadOlderComments() {
    const current = page;
    if (!current || loadingOlderComments || !hasOlderComments) return;
    const gen = loadGen;
    const op = claimCommentOp();
    loadingOlderComments = true;
    // Keyed on the oldest comment on screen, so a comment posted while this
    // request is in flight cannot shift what "the previous page" means.
    const res = await listPageComments(current.id, olderCursor(comments));
    // Token first. A page that arrives after the reader navigated, or after a
    // refresh took the window over, must not clear the newer operation's
    // loading flag, let alone prepend this page's comments to another thread.
    if (!commentOpIsCurrent({ route: gen, op }, loadGen, commentOp)) return;
    loadingOlderComments = false;
    if (!res.ok) {
      toast(`Couldn't load older comments: ${res.error}`, { kind: "error" });
      return;
    }
    comments = prependOlderComments(comments, res.data.items);
    hasOlderComments = res.data.hasMore;
  }

  $effect(() =>
    startAutoRefresh({
      refresh: refreshPage,
      // Also skip while a navigation load is running (loading) so a focus
      // event can't fire a redundant fetch on top of the mount load.
      isBusy: () => bodyMode === "edit" || saving || loading,
      // Focus-only — no background interval for the page editor.
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
    const op = claimCommentOp();
    const epoch = commentMutationEpoch;
    loading = true;
    error = "";
    comments = [];
    hasOlderComments = false;
    activity = [];
    labels = [];
    folders = [];
    const res = await getPage(id);
    if (gen !== loadGen) return; // a newer navigation superseded this load
    if (!res.ok) { error = res.error; loading = false; return; }
    page = res.data;
    // LIF-234: prime role gating — a project page reads that project's role;
    // a workspace page needs the workspace-admin flag instead.
    if (page.project_id !== null) loadProjectRole(page.project_id);
    else ensureMeAdmin();
    recordRecent({ type: "page", routeId: String(page.id), identifier: page.identifier, title: page.title, project: projectIdentifier }); // LIF-237

    // Load page comments and (project) labels in parallel. Workspace
    // pages skip the labels fetch — they can't carry any (LIF-105).
    const commentParentId = page.id;
    const tasks: Promise<unknown>[] = [
      loadCommentWindow((before, size) =>
        listPageComments(commentParentId, before, size),
      ).then((r) => {
        if (!r.ok) return;
        const outcome = commentWindowOutcome(
          { route: gen, op, epoch },
          loadGen,
          commentOp,
          commentMutationEpoch,
        );
        if (outcome === "abandon") return;
        if (outcome === "retry") {
          // Fired, not awaited: this task must settle so `loading` comes down.
          void restabilizeCommentWindow(commentParentId, gen);
          return;
        }
        comments = r.data.items;
        hasOlderComments = r.data.hasOlder;
      }),
      listPageActivity(page.id).then((r) => { if (gen === loadGen && r.ok) activity = r.data.items; }),
    ];
    if (page.project_id !== null) {
      tasks.push(
        listLabels(page.project_id).then((r) => { if (gen === loadGen && r.ok) labels = r.data; }),
      );
      // Only need folder names to render the breadcrumb's folder segment.
      if (page.folder_id !== null) {
        tasks.push(
          listFolders(page.project_id).then((r) => { if (gen === loadGen && r.ok) folders = r.data; }),
        );
      }
    }
    await Promise.all(tasks);

    if (gen !== loadGen) return;
    loading = false;
  }

  // ── Save ─────────────────────────────────────────────

  async function saveField(field: string, value: unknown) {
    if (!page) return;
    saving = true;
    const res = await updatePage(page.id, { [field]: value });
    if (res.ok) {
      page = res.data;
      lastSaved = new Date().toLocaleTimeString([], {
        hour: "2-digit",
        minute: "2-digit",
      });
      // Surface the edit in the Activity timeline immediately.
      listPageActivity(page.id).then((r) => {
        if (r.ok) activity = r.data.items;
      });
    } else {
      // Error-only: title/content/status/labels are optimistic inline edits;
      // a failed save must still surface (LIF-284).
      toast(`Couldn't save ${page.identifier}: ${res.error}`, { kind: "error" });
    }
    saving = false;
  }

  async function saveTitle(next: string) {
    await saveField("title", next);
  }

  async function saveBody(next: string) {
    if (!page) return;
    if (next !== page.content) {
      await saveField("content", next);
    }
  }

  // LIF-112: persist a lifecycle status change. The Select binds to a
  // local mirror so the dropdown reflects the new value immediately. The
  // effect below syncs the mirror down from the loaded page, and persists
  // up when the user picks a different value — `lastStatus` guards against
  // the load-sync re-triggering a save.
  let statusValue = $state("draft");
  let lastStatus = $state("draft");
  $effect(() => {
    // Sync down whenever the loaded page's status changes (page switch
    // or server refresh). Update lastStatus together so the persistence
    // branch doesn't fire on this server-driven change.
    const serverStatus = page?.status ?? "draft";
    if (serverStatus !== lastStatus && serverStatus !== statusValue) {
      statusValue = serverStatus;
      lastStatus = serverStatus;
    }
  });
  $effect(() => {
    // Persist up when the user picks a new value via the Select.
    if (statusValue !== lastStatus) {
      lastStatus = statusValue;
      saveStatus(statusValue);
    }
  });

  async function saveStatus(next: string) {
    if (!page || next === page.status) return;
    await saveField("status", next);
  }

  // LIF-105: toggle a label name on/off, then persist the full set
  // (backend does delete-all + reinsert, so we send the entire array).
  async function toggleLabel(name: string) {
    if (!page) return;
    const current = [...page.labels];
    const idx = current.indexOf(name);
    if (idx >= 0) current.splice(idx, 1);
    else current.push(name);
    await saveField("labels", current);
  }

  // ── Comments / export / delete ───────────────────────

  /** Whether `parentId` is still the page on screen.
   *
   *  Both halves are load-bearing. `pageId` is the routed prop and flips the
   *  instant the reader navigates; `page` keeps the previous document until
   *  the new one resolves, because `loadPage` cannot clear it without
   *  blanking the view on every reload. Checking only the loaded copy would
   *  leave an interval where a write for the page just left folds into a
   *  thread that has already been emptied for the next one. Checking only the
   *  routed prop would accept a write while the matching page is still
   *  loading, against comments that are not there yet. */
  function isCurrentPage(parentId: number): boolean {
    return pageId === parentId && page?.id === parentId;
  }

  // Each mutation captures the thread it belongs to before it sends, by parent
  // id rather than by route generation. Those differ: a failed load retried,
  // or a navigation away and back to the same page, bumps the generation
  // while leaving the thread on screen exactly the one the write was for.
  // Gating on identity means such a write still lands, which is the coherent
  // answer, and a write for a different parent is dropped.
  //
  // On success the mutation epoch is bumped and the result folded in
  // unconditionally, so a comment the user just posted, edited or deleted is
  // visible immediately. `upsertComment` and `removeComment` are idempotent
  // and order-preserving, so the same row arriving later through a refresh
  // cannot duplicate it or move it.
  //
  // Bumping the epoch is what protects the fold. A replacement refresh
  // captured the old epoch when it started, so it can no longer land and undo
  // this edit; the next refresh captures the new one and reconciles normally.
  // A manual older page needs no such guard: it returns rows strictly below a
  // cursor these mutations never touch.
  async function handleNewComment(content: string) {
    const parentId = page?.id;
    if (parentId === undefined) return null;
    const res = await createPageComment(parentId, content);
    if (!res.ok) {
      toast(`Couldn't add comment: ${res.error}`, { kind: "error" });
      return null;
    }
    if (isCurrentPage(parentId)) {
      commentMutationEpoch += 1;
      comments = upsertComment(comments, res.data);
    }
    return res.data;
  }

  async function handleUpdateComment(id: number, content: string) {
    const parentId = page?.id;
    if (parentId === undefined) return null;
    const res = await updateComment(id, content);
    if (!res.ok) {
      toast(`Couldn't update comment: ${res.error}`, { kind: "error" });
      return null;
    }
    if (isCurrentPage(parentId)) {
      commentMutationEpoch += 1;
      comments = upsertComment(comments, res.data);
    }
    return res.data;
  }

  async function handleDeleteComment(id: number): Promise<boolean> {
    const parentId = page?.id;
    if (parentId === undefined) return false;
    const res = await deleteComment(id);
    if (!res.ok) {
      toast(`Couldn't delete comment: ${res.error}`, { kind: "error" });
      return false;
    }
    if (isCurrentPage(parentId)) {
      commentMutationEpoch += 1;
      comments = removeComment(comments, id);
    }
    return true;
  }

  async function exportMarkdown() {
    if (!page || exporting) return;
    exporting = true;
    exportError = "";
    const res = await downloadPageExport(page.identifier);
    if (!res.ok) exportError = res.error;
    exporting = false;
  }

  async function handleDelete(): Promise<boolean> {
    if (!page) return false;
    const res = await deletePage(page.id);
    if (res.ok) {
      navigate(`/${projectIdentifier}/pages`);
      return true;
    }
    toast(`Couldn't delete ${page.identifier}: ${res.error}`, { kind: "error" });
    return false;
  }

  // LIF-286: breadcrumb trail — PROJ › Pages › (Folder ›) LIF-DOC-N. The
  // folder segment appears only when the page is filed under one; its name is
  // resolved from the folders fetched in loadPage. PageList has no per-folder
  // route, so the folder crumb links to the flat pages list. The trail ends
  // with the page identifier in mono, matching IssueDetail's convention: the
  // title already reads as the document heading directly below, while the
  // identifier (the handle used in MCP tools, exports, and cross-references)
  // otherwise never surfaced in the shell.
  let breadcrumbSegments = $derived.by<import("../lib/Breadcrumbs.svelte").Crumb[]>(() => {
    const crumbs: import("../lib/Breadcrumbs.svelte").Crumb[] = [
      { label: projectIdentifier, href: `#/${projectIdentifier}/overview`, mono: true, hideBelowSm: true, copy: projectIdentifier },
      // Collapsed below sm — the app header already states the section.
      { label: "Pages", href: `#/${projectIdentifier}/pages`, hideBelowSm: true },
    ];
    if (page?.folder_id != null) {
      const name = folders.find((f) => f.id === page!.folder_id)?.name;
      if (name) crumbs.push({ label: name, href: `#/${projectIdentifier}/pages` });
    }
    // `copy` stays unset until the page loads, so nobody can copy the
    // placeholder "Page" label.
    crumbs.push({ label: page?.identifier || "Page", mono: true, copy: page?.identifier });
    return crumbs;
  });

  // ── LIF-159: palette actions ─────────────────────────
  let paletteActions = $derived.by<import("../lib/palette").PaletteAction[]>(() => {
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
            // Setting the local mirror persists via the LIF-112 effect.
            run: () => { statusValue = s.value; },
          })),
      },
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
  title={page?.title ?? ""}
  titleSize="lg"
  onSaveTitle={saveTitle}
  body={page?.content ?? ""}
  bodyPlaceholder="Start writing... (markdown supported)"
  bodyEmptyEditCta="Click to start writing..."
  bodyEmptyReadText="Empty page"
  bodyProseMinHeight="120px"
  onSaveBody={saveBody}
  {saving}
  {lastSaved}
  onExport={exportMarkdown}
  {exporting}
  {exportError}
  deleteNoun="page"
  deleteLabel={page?.identifier ?? ""}
  onDelete={handleDelete}
  {comments}
  onNewComment={handleNewComment}
  {currentUser}
  onUpdateComment={handleUpdateComment}
  onDeleteComment={handleDeleteComment}
  {hasOlderComments}
  {loadingOlderComments}
  onLoadOlderComments={loadOlderComments}
  commentParentKey={`page:${pageId}`}
  mentionProjectId={page?.project_id ?? null}
  {activity}
  {paletteActions}
  layout="wide"
  bind:bodyMode
>
  {#snippet breadcrumbExtra()}
    {#if !editable && (isWorkspacePage ? projectRole.globalEnforced : projectRole.enforced)}
      <!-- LIF-234: read-only cue for a viewer (project page) or non-admin
           (workspace page). Commenting stays available on project pages. -->
      <span class="text-micro font-medium px-1.5 py-0.5 rounded-full text-[var(--text-muted)] bg-[var(--bg-subtle)]"
            title={isWorkspacePage
              ? "Read-only — workspace pages can only be edited by an admin."
              : "Read-only — you're a viewer on this project. You can still comment."}>
        Read-only
      </span>
    {/if}
  {/snippet}

  {#snippet belowTitle()}
    <!-- LIF-112 + LIF-105: lifecycle status picker and labels strip. Both
         sit between title and body, mirroring the issue sidebar's UX but
         laid out horizontally since pages have no sidebar. -->
    {#if page}
      <div class="mb-6 flex flex-wrap items-center gap-4">
        <!-- LIF-183: pin toggle. Pinned pages surface in a section atop the
             page list regardless of folder. -->
        {#if editable}
          <button
            class="flex items-center gap-1.5 text-body-sm font-medium
                   px-2 py-1 rounded-md border transition-colors
                   {page.pinned
              ? 'text-[var(--accent)] border-[var(--accent)] bg-[var(--accent-subtle)]'
              : 'text-[var(--text-muted)] border-[var(--border)] hover:bg-[var(--bg-subtle)] hover:text-[var(--text)]'}"
            title={page.pinned ? "Unpin this page" : "Pin to top of the page list"}
            onclick={() => { if (page) saveField("pinned", !page.pinned); }}
            disabled={saving}
          >
            <Pin size={13} class={page.pinned ? "fill-current" : ""} />
            {page.pinned ? "Pinned" : "Pin"}
          </button>
        {/if}

        <!-- LIF-112: status picker. Available for every page (workspace
             pages included — status isn't project-scoped). -->
        {#if editable}
          <Select
            options={statusOptions}
            bind:value={statusValue}
            size="sm"
            class="w-auto"
          >
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
                <span class="{isSelected ? 'text-[var(--accent)]' : 'text-[var(--text)]'}">{meta.label}</span>
              </span>
            {/snippet}
          </Select>
        {:else}
          {@const meta = statusMeta(page.status)}
          <span class="flex items-center gap-1.5 text-body-sm text-[var(--text-muted)]">
            <meta.icon size={13} class="shrink-0" />
            {meta.label}
          </span>
        {/if}

        <!-- LIF-105: labels strip. Workspace pages skip it — labels are
             project-scoped. -->
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
    {/if}
  {/snippet}

  {#snippet metaFooter()}
    {#if page}
      <div class="mt-10 pt-6 border-t border-[var(--border)] flex gap-8">
        <div>
          <span class="block text-micro font-semibold uppercase tracking-widest text-[var(--text-faint)] mb-0.5">
            Created
          </span>
          <span class="text-body-sm text-[var(--text-muted)]">
            {formatDate(page.created_at)}
          </span>
        </div>
        <div>
          <span class="block text-micro font-semibold uppercase tracking-widest text-[var(--text-faint)] mb-0.5">
            Updated
          </span>
          <span class="text-body-sm text-[var(--text-muted)]">
            {formatDate(page.updated_at)}
          </span>
        </div>
      </div>
    {/if}
  {/snippet}
</DocumentDetail>

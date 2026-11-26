<script lang="ts">
  import {
    resolveIssue,
    getIssue,
    updateIssue,
    downloadIssueExport,
    listModules,
    listLabels,
    createLabel,
    listComments,
    createComment,
    updateComment,
    deleteComment,
    listIssueActivity,
    linkIssues,
    unlinkIssues,
    me,
    type Issue,
    type Module,
    type Label,
    type Activity,
    type AuthUser,
  } from "../lib/api";
  import DocumentDetail from "../lib/DocumentDetail.svelte";
  import type { BodySaveResult } from "../lib/editor/bodySync";
  import LabelEditor from "../lib/LabelEditor.svelte";
  import IssuePickerModal from "../lib/IssuePickerModal.svelte";
  import MoveIssueModal from "../lib/issues/MoveIssueModal.svelte"; // LIF-488
  import { moveNotice } from "../lib/issues/move";
  import { projectCodeOf } from "../lib/references";
  import {
    RELATION_KINDS,
    hasRelations,
    linkRequest,
    relationsOf,
    type RelationKind,
  } from "../lib/issues/relations";
  import WaitEditor from "../lib/issues/WaitEditor.svelte"; // LIF-485
  import AssigneeField from "../lib/issues/AssigneeField.svelte"; // LIF-147
  import { assignmentKind, assignmentNames, personName, sameNames, type Person } from "../lib/issues/assignees";
  import ProjectIcon from "../lib/ProjectIcon.svelte";
  import PriorityIcon from "../lib/PriorityIcon.svelte";
  import StatusIcon, { statusCssColor } from "../lib/StatusIcon.svelte";
  import TimeAgo from "../lib/TimeAgo.svelte";
  import { recordRecent } from "../lib/home/recents"; // LIF-237
  import { updateIssueWithUndo } from "../lib/issues/state.svelte"; // LIF-243
  import { scheduleDelete } from "../lib/issues/deferredDelete.svelte"; // LIF-283
  import { openPeek } from "../lib/issues/peek.svelte"; // LIF-248
  import { loadLayout } from "../lib/issues/persistence"; // LIF-434
  import { PRIORITIES, STATUSES, TERMINAL_STATUSES, priorityLabel, statusLabel } from "../lib/issues/grouping";
  import { formatDay, localDay } from "../lib/issues/waits";
  import { projectRole, loadProjectRole } from "../lib/projectRole.svelte"; // LIF-234
  import { startAutoRefresh } from "../lib/autoRefresh.svelte";
  import { shortcutsSuppressed } from "../lib/shortcuts";
  import { toast } from "../lib/toast/toast.svelte"; // LIF-284
  import { CommentThread } from "../lib/detail/commentThread.svelte";
  import { ArrowUpRight, CalendarDays, ChevronDown, FolderInput, Plus, Tag, X } from "lucide-svelte";
  import { getContext } from "svelte";
  import { pageTitle, PAGE_TITLE_CONTEXT } from "../lib/pageTitle";
  import { isSignedOut } from "../lib/signedOut"; // LIF-583

  let {
    navigate,
    projectIdentifier,
    issueIdentifier,
    editable: editableProp,
  }: {
    navigate: (path: string) => void;
    projectIdentifier: string;
    issueIdentifier: string;
    /** Optional hard override. When omitted, the caller's project role
     *  drives it: a viewer is read-only (LIF-234). */
    editable?: boolean;
  } = $props();

  // LIF-234: content edits require maintainer+ once enforcement is on;
  // commenting stays available for viewers.
  const editable = $derived(editableProp ?? projectRole.canEdit);
  const canComment = $derived(projectRole.canComment);

  // The list this issue was opened from: whichever layout the user last
  // used for this project (set by IssueList).
  function backHref(): string {
    return loadLayout(projectIdentifier) === "board"
      ? `/${projectIdentifier}/board`
      : `/${projectIdentifier}/issues`;
  }
  function backText(): string {
    return loadLayout(projectIdentifier) === "board" ? "Board" : "Issues";
  }

  let issue = $state<Issue | null>(null);
  let modules = $state<Module[]>([]);
  let labels = $state<Label[]>([]);
  let activity = $state<Activity[]>([]);
  let currentUser = $state<AuthUser | null>(null);
  let loading = $state(true);
  let error = $state("");
  // Bumped on every routed navigation. Anything in flight from before the
  // bump belongs to an issue the reader has left, and is discarded.
  let loadGen = 0;
  // Orders the reads that replace `issue` within one route (the initial
  // load, refreshes, reloads after a relation change): only the latest
  // started may land.
  let issueRead = 0;

  // Tab title: the real title once the issue lands.
  const titleCtx = getContext<{ set: (title?: string) => void } | undefined>(PAGE_TITLE_CONTEXT);
  $effect(() => {
    if (!issue) return;
    titleCtx?.set(pageTitle(issue.identifier, issue.title, projectIdentifier));
  });

  if (!isSignedOut()) {
    void me().then((res) => {
      if (res.ok) currentUser = res.data;
    });
  }

  const thread = new CommentThread({
    list: (id, before, size) => listComments(id, before, size),
    create: createComment,
    update: updateComment,
    remove: deleteComment,
    gen: () => loadGen,
    currentId: () => issue?.id ?? null,
    isCurrent: (id) => issue?.id === id,
    onMutated: () => void refreshActivity(),
  });

  // One menu open at a time, by name. Every picker in the view reads this.
  type Menu = "status" | "header-status" | "priority" | "module" | "labels" | "relation" | "start" | "due";
  let openMenu = $state<Menu | null>(null);
  function toggleMenu(menu: Menu, e?: Event) {
    e?.stopPropagation();
    if (!editable) return;
    openMenu = openMenu === menu ? null : menu;
  }

  // What the open relation picker will link, captured when it opens: the
  // picker resolves the chosen issue asynchronously, and by then the reader
  // may be on another issue. `gen` lets that late selection be dropped.
  let addingRelation = $state<{ kind: RelationKind; source: string; gen: number } | null>(null);
  let relationPickerOpen = $state(false);

  /** The issues this one links to, for their titles and status. */
  let related = $state<Map<string, Issue>>(new Map());

  let saving = $state(false);
  let savedAt = $state<number | null>(null);

  let exportError = $state("");
  let exporting = false;
  // LIF-488: "Move to project", from the menu or the palette.
  let moveOpen = $state(false);
  const canMove = $derived(editable && !isSignedOut());

  $effect(() => {
    const id = issueIdentifier;
    openMenu = null;
    relationPickerOpen = false;
    addingRelation = null;
    savedAt = null;
    // Claim the route synchronously, before any await: a slow response for
    // LIF-9 must never paint over LIF-10.
    loadGen += 1;
    thread.reset();
    issue = null;
    modules = [];
    labels = [];
    activity = [];
    related = new Map();
    loadIssue(id, false, loadGen);
  });

  $effect(() =>
    startAutoRefresh({
      // Rides the current generation, so a navigation mid-refresh wins.
      refresh: () => loadIssue(issueIdentifier, true, loadGen),
      // The description editor merges refreshes into what is being typed.
      isBusy: () => saving || loading || openMenu !== null || relationPickerOpen,
      intervalMs: 0,
      shouldRefresh: (event) =>
        event.type === "resync.required" ||
        (typeof event.issue_id === "number" && event.issue_id === issue?.id) ||
        (event.type.startsWith("project.") &&
          typeof event.project_id === "number" &&
          event.project_id === issue?.project_id),
    }),
  );

  async function loadIssue(identifier: string, background: boolean, gen: number) {
    if (!background) {
      loading = true;
      error = "";
    }
    const read = ++issueRead;
    const res = await resolveIssue(identifier);
    if (gen !== loadGen || read !== issueRead) return;
    if (!res.ok) {
      error = res.error;
      loading = false;
      return;
    }
    issue = res.data;
    loadProjectRole(issue.project_id); // LIF-234
    recordRecent({ type: "issue", routeId: issue.identifier, identifier: issue.identifier, title: issue.title, project: projectIdentifier }); // LIF-237

    const current = issue;
    const [modRes, lblRes, actRes] = await Promise.all([
      listModules(current.project_id),
      listLabels(current.project_id),
      listIssueActivity(current.id),
      // A refresh reconciles every comment page the reader has loaded.
      thread.load(current.id, gen, background),
      loadRelated(current, gen),
    ]);
    if (gen !== loadGen || read !== issueRead) return;
    if (modRes.ok) modules = modRes.data;
    if (lblRes.ok) labels = lblRes.data;
    if (actRes.ok) activity = actRes.data.items;
    loading = false;
  }

  /** Fetch the linked issues so relations read as titles, not bare ids. */
  async function loadRelated(source: Issue, gen: number) {
    const ids = [...new Set(RELATION_KINDS.flatMap((k) => relationsOf(source, k)))].slice(0, 40);
    if (ids.length === 0) {
      related = new Map();
      return;
    }
    const found = await Promise.all(ids.map((id) => resolveIssue(id)));
    if (gen !== loadGen) return;
    const next = new Map<string, Issue>();
    found.forEach((r, i) => {
      if (r.ok) next.set(ids[i], r.data);
    });
    related = next;
  }

  // Re-pull the timeline after any mutation so the user's own edit shows.
  async function refreshActivity() {
    if (!issue) return;
    const gen = loadGen;
    const res = await listIssueActivity(issue.id);
    if (gen !== loadGen) return;
    if (res.ok) activity = res.data.items;
  }

  function markSaved() {
    savedAt = Date.now();
    void refreshActivity();
  }

  // ── Saving ───────────────────────────────────────────

  async function saveField(patch: Record<string, unknown>) {
    if (!issue) return;
    const id = issue.id;
    saving = true;
    const res = await updateIssue(id, patch);
    saving = false;
    if (!res.ok) {
      // Inline edits are optimistic and quiet on success, never on failure.
      toast(`Couldn't save ${issue?.identifier ?? "the issue"}: ${res.error}`, { kind: "error" });
      return;
    }
    if (issue?.id === id) {
      issue = res.data;
      markSaved();
    }
  }

  /** Save the description, guarded by `expected_seq`: a description changed
   *  elsewhere is merged or reported as a conflict by the editor, never
   *  overwritten. `issueId` is the issue the editor was opened for. */
  async function saveDescription(
    next: string,
    expectedSeq: number | null,
    issueId: number | null,
  ): Promise<BodySaveResult> {
    const id = issueId ?? issue?.id;
    if (id == null) return { ok: false, error: "No issue loaded" };
    const onScreen = () => issue?.id === id;
    saving = true;
    const res = await updateIssue(id, {
      description: next,
      ...(expectedSeq != null ? { expected_seq: expectedSeq } : {}),
    });
    saving = false;
    if (res.ok) {
      if (onScreen()) {
        issue = res.data;
        markSaved();
      }
      return { ok: true, body: res.data.description, seq: res.data.seq ?? null };
    }
    if (res.status === 409) {
      const fresh = await getIssue(id);
      if (fresh.ok) {
        if (onScreen()) issue = fresh.data;
        return { ok: false, conflict: { body: fresh.data.description, seq: fresh.data.seq ?? null } };
      }
    }
    return { ok: false, error: res.error };
  }

  /** LIF-243: one-click values (status, priority, module, assignee) save
   *  with an Undo toast. `onApplied` also runs after an Undo, possibly from
   *  another route, so it only touches the issue it was for. */
  async function saveWithUndo(
    patch: Record<string, unknown>,
    prevPatch: Record<string, unknown>,
    people?: Person[],
  ) {
    if (!issue) return;
    const id = issue.id;
    saving = true;
    await updateIssueWithUndo({
      id,
      identifier: issue.identifier,
      patch,
      prevPatch,
      modules,
      people,
      onApplied: (applied, updated) => {
        if (issue && issue.id === id) {
          // LIF-147: an assignment is sent as names but read back as people.
          issue =
            "assignees" in applied
              ? { ...issue, needs_human: updated.needs_human, assignees: updated.assignees, seq: updated.seq }
              : { ...issue, ...(applied as Partial<Issue>) };
        }
        markSaved();
      },
    });
    saving = false;
  }

  async function setStatus(value: string) {
    openMenu = null;
    if (issue && value !== issue.status) await saveWithUndo({ status: value }, { status: issue.status });
  }

  async function setPriority(value: string) {
    openMenu = null;
    if (issue && value !== issue.priority) await saveWithUndo({ priority: value }, { priority: issue.priority });
  }

  async function setModule(id: number | null) {
    openMenu = null;
    if (issue && id !== issue.module_id) await saveWithUndo({ module_id: id }, { module_id: issue.module_id });
  }

  /** LIF-147: replace who the issue is for. */
  async function setAssignees(names: string[], people: Person[]) {
    if (!issue) return;
    const prev = assignmentNames(issue);
    if (sameNames(names, prev)) return;
    await saveWithUndo({ assignees: names }, { assignees: prev }, people);
  }

  async function setDate(field: "start_date" | "target_date", day: string) {
    openMenu = null;
    if (!issue || (issue[field] ?? "") === day) return;
    await saveField({ [field]: day });
  }

  async function toggleLabel(name: string) {
    if (!issue) return;
    const next = issue.labels.includes(name) ? issue.labels.filter((l) => l !== name) : [...issue.labels, name];
    await saveField({ labels: next });
  }

  /** Create a project label from the picker and attach it. */
  async function createLabelInline(name: string, color: string): Promise<boolean> {
    if (!issue) return false;
    const res = await createLabel({ project_id: issue.project_id, name, color });
    if (!res.ok) {
      toast(`Couldn't create label: ${res.error}`, { kind: "error" });
      return false;
    }
    labels = [...labels, res.data].sort((a, b) => a.name.localeCompare(b.name));
    await toggleLabel(res.data.name);
    return true;
  }

  // ── Relations ────────────────────────────────────────

  /** LIF-248: shift-click previews instead of navigating. A related issue
   *  may live in another project (LIF-504), so route by its own code. */
  function openRelation(e: MouseEvent, rel: string) {
    if (e.shiftKey) {
      e.preventDefault();
      openPeek(rel);
      return;
    }
    navigate(`/${projectCodeOf(rel)}/issues/${rel}`);
  }

  function startAddRelation(kind: RelationKind) {
    if (!issue) return;
    openMenu = null;
    addingRelation = { kind, source: issue.identifier, gen: loadGen };
    relationPickerOpen = true;
  }

  async function addRelation(other: Issue) {
    const pending = addingRelation;
    if (!pending || pending.gen !== loadGen) return;
    const req = linkRequest(pending.kind, pending.source, other.identifier);
    const res = await linkIssues(req.source, req.target, req.relation_type);
    if (!res.ok) {
      toast(`Couldn't link ${other.identifier}: ${res.error}`, { kind: "error" });
      return;
    }
    await loadIssue(issueIdentifier, true, loadGen);
  }

  // The API has no per-type unlink: this drops every relation between the
  // two issues, so a pair linked twice (blocks + related) loses both.
  async function removeRelation(other: string) {
    if (!issue) return;
    const res = await unlinkIssues(issue.identifier, other);
    if (!res.ok) {
      toast(`Couldn't unlink ${other}: ${res.error}`, { kind: "error" });
      return;
    }
    await loadIssue(issueIdentifier, true, loadGen);
  }

  // ── Export / delete / move ───────────────────────────

  async function exportMarkdown() {
    if (!issue || exporting) return;
    exporting = true;
    exportError = "";
    const res = await downloadIssueExport(issue.identifier);
    if (!res.ok) exportError = res.error;
    exporting = false;
  }

  async function handleDelete(): Promise<boolean> {
    if (!issue) return false;
    // LIF-283: leave at once and delete after the Undo window. Undo (the
    // toast survives navigation) brings the reader back here.
    const captured = issue;
    const detailHref = `/${projectIdentifier}/issues/${captured.identifier}`;
    navigate(backHref());
    scheduleDelete([captured], { onRestore: () => navigate(detailHref) });
    return true;
  }

  /** LIF-488: the issue now lives under another identifier; follow it. */
  function handleMoved(moved: import("../lib/api").MovedIssue) {
    toast(moveNotice(moved));
    const next = moved.issue.identifier;
    navigate(`/${projectCodeOf(next)}/issues/${next}`);
  }

  const currentModule = $derived(issue?.module_id ? modules.find((m) => m.id === issue!.module_id) : undefined);

  // ── Dates ────────────────────────────────────────────

  const DATE_FIELDS = [
    { field: "start_date", label: "Start", menu: "start" },
    { field: "target_date", label: "Due", menu: "due" },
  ] as const;

  const today = localDay();
  function dueState(i: Issue): "overdue" | "today" | "soon" | null {
    const due = i.target_date;
    if (!due || TERMINAL_STATUSES.includes(i.status)) return null;
    if (due < today) return "overdue";
    if (due === today) return "today";
    const soon = new Date();
    soon.setDate(soon.getDate() + 3);
    return due <= localDay(soon) ? "soon" : null;
  }

  // ── Keyboard: S status, P priority, M module, L labels, D due date ──

  function onKeydown(e: KeyboardEvent) {
    if (!issue || !editable || shortcutsSuppressed() || e.metaKey || e.ctrlKey || e.altKey) return;
    if (window.matchMedia("(max-width: 767px)").matches) return;
    const menu = ({ s: "status", p: "priority", m: "module", l: "labels", d: "due" } as const)[e.key];
    if (!menu || (menu === "module" && modules.length === 0)) return;
    e.preventDefault();
    openMenu = openMenu === menu ? null : menu;
  }

  // ── LIF-159: palette actions ─────────────────────────
  const paletteActions = $derived.by<import("../lib/palette").PaletteAction[]>(() => {
    if (!issue) return [];
    const i = issue;
    return [
      {
        id: "set-status",
        title: "Set status…",
        hint: i.status,
        children: () =>
          STATUSES.map((s) => ({
            title: statusLabel(s),
            status: s,
            hint: s === i.status ? "current" : undefined,
            run: () => void setStatus(s),
          })),
      },
      {
        id: "set-priority",
        title: "Set priority…",
        hint: i.priority,
        children: () =>
          PRIORITIES.map((p) => ({
            title: priorityLabel(p),
            priority: p,
            hint: p === i.priority ? "current" : undefined,
            run: () => void setPriority(p),
          })),
      },
      ...(modules.length > 0
        ? [
            {
              id: "set-module",
              title: "Set module…",
              hint: currentModule?.name ?? "None",
              children: () => [
                { title: "None", hint: i.module_id === null ? "current" : undefined, run: () => void setModule(null) },
                ...modules.map((m) => ({
                  title: m.name,
                  hint: m.id === i.module_id ? "current" : undefined,
                  run: () => void setModule(m.id),
                })),
              ],
            },
          ]
        : []),
      ...(labels.length > 0
        ? [
            {
              id: "toggle-label",
              title: "Add or remove label…",
              hint: i.labels.length > 0 ? i.labels.join(", ") : undefined,
              children: () =>
                labels.map((l) => ({
                  title: l.name,
                  color: l.color,
                  hint: i.labels.includes(l.name) ? "remove" : "add",
                  run: () => void toggleLabel(l.name),
                })),
            },
          ]
        : []),
      {
        id: "set-due",
        title: "Set due date…",
        hint: i.target_date ? formatDay(i.target_date) : undefined,
        prompt: {
          placeholder: "YYYY-MM-DD, or empty to clear",
          initial: i.target_date ?? "",
          submit: (v) => {
            const day = v.trim();
            if (day && !/^\d{4}-\d{2}-\d{2}$/.test(day)) {
              toast("Use a date like 2026-12-31", { kind: "error" });
              return;
            }
            void setDate("target_date", day);
          },
        },
      },
      ...(canMove ? [{ id: "move-project", title: "Move to project…", run: () => (moveOpen = true) }] : []),
    ];
  });

  const readOnlyNote = $derived(
    projectRole.enforced
      ? isSignedOut()
        ? "Read-only. Sign in to edit or comment."
        : projectRole.role === null
          ? "Read-only. You're not a member of this project."
          : "You're a viewer on this project. You can still comment."
      : null,
  );
</script>

<svelte:window onclick={() => (openMenu = null)} onkeydown={onKeydown} />

<DocumentDetail
  {navigate}
  {loading}
  {error}
  deleteNounLabel="issue"
  onRetry={() => loadIssue(issueIdentifier, false, ++loadGen)}
  identifier={issue?.identifier ?? issueIdentifier}
  attachEntity={issue ? { entity_type: "issue", entity_id: issue.id } : null}
  backRoute={backHref()}
  backLabel={backText()}
  breadcrumbSegments={[
    { label: projectIdentifier, href: `#/${projectIdentifier}/overview`, mono: true, hideBelowSm: true, copy: projectIdentifier },
    // Collapsed below sm: the app header already names the section.
    { label: backText(), href: `#${backHref()}`, hideBelowSm: true },
    { label: issue?.identifier ?? issueIdentifier, mono: true, copy: issue?.identifier ?? issueIdentifier },
  ]}
  {editable}
  {canComment}
  {readOnlyNote}
  title={issue?.title ?? ""}
  titleSize="md"
  onSaveTitle={(next) => saveField({ title: next })}
  body={issue?.description ?? ""}
  bodySeq={issue?.seq ?? null}
  bodyDocId={issue?.id ?? null}
  bodyNoun="The description"
  bodyPlaceholder="Add a description…  Type / for headings, lists, checklists and more."
  bodyEmptyReadText="No description"
  bodyProseMinHeight="60px"
  onSaveBody={saveDescription}
  autofocusWhenEmpty
  {saving}
  {savedAt}
  onExport={isSignedOut() ? undefined : exportMarkdown}
  {exportError}
  deleteNoun="issue"
  deleteLabel={issue?.identifier ?? ""}
  deleteUndoable
  onDelete={handleDelete}
  menuItems={canMove ? [{ label: "Move to project…", icon: FolderInput, run: () => (moveOpen = true) }] : []}
  comments={thread.comments}
  onNewComment={(c) => thread.create(c)}
  {currentUser}
  onUpdateComment={(id, c) => thread.update(id, c)}
  onDeleteComment={(id) => thread.remove(id)}
  hasOlderComments={thread.hasOlder}
  loadingOlderComments={thread.loadingOlder}
  onLoadOlderComments={() => thread.loadOlder()}
  commentParentKey={`issue:${issueIdentifier}`}
  mentionProjectId={issue?.project_id ?? null}
  {activity}
  {paletteActions}
  layout="two-column"
>
  {#snippet breadcrumbExtra()}
    {#if issue}
      <span class="text-[var(--text-faint)]">/</span>
      <!-- LIF-359: a visible status is also the control that changes it. -->
      <div class="relative shrink-0">
        <button
          class="flex items-center gap-1.5 text-body-sm rounded-md px-1.5 py-0.5 -mx-1.5 transition-colors
                 {editable ? 'hover:bg-[var(--bg-subtle)] cursor-pointer' : 'cursor-default'}"
          aria-haspopup={editable ? "menu" : undefined}
          aria-expanded={editable ? openMenu === "header-status" : undefined}
          title={editable ? "Change status" : `Status: ${issue.status}`}
          onclick={(e) => toggleMenu("header-status", e)}
        >
          <StatusIcon status={issue.status} size={13} />
          <span style="color: {statusCssColor(issue.status)}">{statusLabel(issue.status)}</span>
          {#if editable}
            <ChevronDown size={11} class="text-[var(--text-faint)] shrink-0" />
          {/if}
        </button>
        {#if openMenu === "header-status"}
          {@render menuPanel(statusOptions)}
        {/if}
      </div>
    {/if}
  {/snippet}

  {#snippet belowTitle(openDetails)}
    {#if issue}
      <!-- On a phone the sidebar is a drawer: this strip shows what it holds
           at a glance and opens it. -->
      <button
        type="button"
        class="md:hidden -mt-1 mb-4 flex flex-wrap items-center gap-1.5 text-left"
        aria-label="Open issue properties"
        onclick={openDetails}
      >
        <span class="strip-chip"><StatusIcon status={issue.status} size={12} />{statusLabel(issue.status)}</span>
        <span class="strip-chip"><PriorityIcon priority={issue.priority} size={12} />{priorityLabel(issue.priority)}</span>
        {#if assignmentKind(issue) === "people"}
          <span class="strip-chip">{(issue.assignees ?? []).map(personName).join(", ")}</span>
        {/if}
        {#if issue.target_date}
          <span class="strip-chip {dueState(issue) === 'overdue' ? 'text-[var(--error)]' : ''}">
            <CalendarDays size={12} />{formatDay(issue.target_date)}
          </span>
        {/if}
        {#if issue.labels.length}
          <span class="strip-chip"><Tag size={12} />{issue.labels.length}</span>
        {/if}
      </button>
    {/if}
  {/snippet}

  {#snippet sidebar()}
    {#if issue}
      <div class="issue-meta-aside">
        <div class="issue-props">
          <div class="issue-meta-field">
            <p class="issue-meta-field-label">Status</p>
            <div class="relative">
              <button class="prop-trigger" class:prop-trigger-edit={editable} aria-haspopup={editable ? "menu" : undefined}
                      aria-expanded={editable ? openMenu === "status" : undefined}
                      title={editable ? "Change status (S)" : undefined}
                      onclick={(e) => toggleMenu("status", e)}>
                <StatusIcon status={issue.status} size={14} />
                <span class="text-[var(--text)]">{statusLabel(issue.status)}</span>
              </button>
              {#if openMenu === "status"}{@render menuPanel(statusOptions)}{/if}
            </div>
          </div>

          <div class="issue-meta-field">
            <p class="issue-meta-field-label">Priority</p>
            <div class="relative">
              <button class="prop-trigger" class:prop-trigger-edit={editable} aria-haspopup={editable ? "menu" : undefined}
                      aria-expanded={editable ? openMenu === "priority" : undefined}
                      title={editable ? "Change priority (P)" : undefined}
                      onclick={(e) => toggleMenu("priority", e)}>
                <PriorityIcon priority={issue.priority} />
                <span class={priorityTextClass(issue.priority)}>{priorityLabel(issue.priority)}</span>
              </button>
              {#if openMenu === "priority"}{@render menuPanel(priorityOptions)}{/if}
            </div>
          </div>

          <div class="issue-meta-field">
            <p class="issue-meta-field-label">Module</p>
            <div class="relative flex items-center min-w-0">
              <button class="prop-trigger flex-1" class:prop-trigger-edit={editable} aria-haspopup={editable ? "menu" : undefined}
                      aria-expanded={editable ? openMenu === "module" : undefined}
                      title={editable ? "Change module (M)" : undefined}
                      onclick={(e) => toggleMenu("module", e)}>
                {#if currentModule?.emoji}
                  <ProjectIcon value={currentModule.emoji} size={14} class="text-[var(--text-muted)] shrink-0" />
                {/if}
                <span class="truncate {currentModule ? 'text-[var(--text)]' : 'text-[var(--text-faint)]'}">
                  {currentModule?.name ?? "None"}
                </span>
              </button>
              {#if currentModule}
                {@const target = currentModule.id}
                <button
                  class="size-6 flex items-center justify-center rounded text-[var(--text-faint)]
                         hover:text-[var(--accent)] hover:bg-[var(--bg-subtle)] transition-colors shrink-0"
                  onclick={(e) => {
                    e.stopPropagation();
                    navigate(`/${projectIdentifier}/modules/${target}`);
                  }}
                  title="Open module"
                  aria-label="Open module"
                >
                  <ArrowUpRight size={13} />
                </button>
              {/if}
              {#if openMenu === "module"}{@render menuPanel(moduleOptions)}{/if}
            </div>
          </div>

          <!-- LIF-147: who must do it. -->
          <AssigneeField {issue} {editable} onApply={setAssignees} onOpen={() => (openMenu = null)} />

          <div class="issue-meta-field">
            <p class="issue-meta-field-label">Labels</p>
            <LabelEditor
              attached={issue.labels}
              all={labels}
              {editable}
              onToggle={toggleLabel}
              onCreate={editable ? createLabelInline : undefined}
              bind:open={() => openMenu === "labels", (v) => (openMenu = v ? "labels" : openMenu === "labels" ? null : openMenu)}
            />
          </div>

          {#each DATE_FIELDS as { field, label, menu } (field)}
            {@const day = issue[field]}
            {@const due = field === "target_date" ? dueState(issue) : null}
            {#if day || editable}
              <div class="issue-meta-field">
                <p class="issue-meta-field-label">{label}</p>
                <div class="relative">
                  <button class="prop-trigger" class:prop-trigger-edit={editable}
                          aria-haspopup={editable ? "dialog" : undefined}
                          aria-expanded={editable ? openMenu === menu : undefined}
                          title={editable ? (field === "target_date" ? "Set the due date (D)" : "Set the start date") : undefined}
                          onclick={(e) => toggleMenu(menu, e)}>
                    <CalendarDays size={14} class={due === "overdue" ? "text-[var(--error)]" : due ? "text-[var(--warn)]" : "text-[var(--text-faint)]"} />
                    {#if day}
                      <span class={due === "overdue" ? "text-[var(--error)] font-medium" : "text-[var(--text)]"}>{formatDay(day)}</span>
                      {#if due === "overdue"}<span class="text-micro text-[var(--error)]">overdue</span>
                      {:else if due === "today"}<span class="text-micro text-[var(--warn-text)]">today</span>{/if}
                    {:else}
                      <span class="text-[var(--text-faint)]">None</span>
                    {/if}
                  </button>
                  {#if openMenu === menu}
                    <!-- svelte-ignore a11y_no_static_element_interactions, a11y_click_events_have_key_events -->
                    <div class="prop-menu p-2 w-[220px]" role="dialog" aria-label="{label} date" tabindex="-1" onclick={(e) => e.stopPropagation()}>
                      <input
                        type="date"
                        class="w-full h-8 px-2 rounded-md text-body-sm bg-[var(--bg)] border border-[var(--border)] text-[var(--text)] outline-none focus:border-[var(--accent)]"
                        value={day ?? ""}
                        aria-label="{label} date"
                        onchange={(e) => setDate(field, (e.currentTarget as HTMLInputElement).value)}
                        onkeydown={(e) => {
                          if (e.key === "Escape") {
                            e.stopPropagation();
                            openMenu = null;
                          }
                        }}
                      />
                      {#if day}
                        <button class="mt-1.5 w-full text-left px-2 py-1 rounded-md text-caption text-[var(--text-muted)] hover:bg-[var(--bg-subtle)]"
                                onclick={() => setDate(field, "")}>
                          Clear the {label.toLowerCase()} date
                        </button>
                      {/if}
                    </div>
                  {/if}
                </div>
              </div>
            {/if}
          {/each}
        </div>

        <!-- LIF-485: user and date blockers. -->
        {#if issue.waits?.length || editable}
          <div class="meta-section">
            <WaitEditor
              issueId={issue.id}
              projectId={issue.project_id}
              waits={issue.waits ?? []}
              {editable}
              onChange={(next) => {
                if (!issue) return;
                issue = { ...issue, waits: next };
                void refreshActivity();
              }}
            />
          </div>
        {/if}

        {#if editable || hasRelations(issue)}
          <div class="issue-meta-relations meta-section">
            {#each RELATION_KINDS as kind (kind.field)}
              {@const rels = relationsOf(issue, kind)}
              {#if rels.length > 0}
                <div class="issue-meta-field">
                  <p class="issue-meta-field-label">{kind.label}</p>
                  <ul class="flex flex-col gap-0.5 m-0 p-0 list-none -mx-2">
                    {#each rels as rel (rel)}
                      {@const other = related.get(rel)}
                      <li class="group/rel flex items-center gap-2 rounded-md px-2 py-1 hover:bg-[var(--bg-subtle)] transition-colors min-w-0">
                        {#if other}
                          <StatusIcon status={other.status} size={13} />
                        {/if}
                        <button
                          class="flex-1 min-w-0 flex items-baseline gap-2 text-left"
                          aria-label={rel}
                          title="{rel}{other ? `: ${other.title}` : ''}  ·  Shift-click to preview"
                          onclick={(e) => openRelation(e, rel)}
                        >
                          <span class="shrink-0 text-caption font-mono {relationIdClass(kind.field)}">{rel}</span>
                          {#if other}
                            <span class="truncate text-caption {TERMINAL_STATUSES.includes(other.status) ? 'text-[var(--text-faint)] line-through' : 'text-[var(--text-muted)]'}">{other.title}</span>
                          {/if}
                        </button>
                        {#if editable}
                          <button
                            class="shrink-0 opacity-0 group-hover/rel:opacity-100 focus-visible:opacity-100 text-[var(--text-faint)] hover:text-[var(--error)] transition-opacity"
                            title="Remove every relation with {rel}"
                            aria-label="Remove relation with {rel}"
                            onclick={() => removeRelation(rel)}
                          >
                            <X size={12} />
                          </button>
                        {/if}
                      </li>
                    {/each}
                  </ul>
                </div>
              {/if}
            {/each}

            {#if editable}
              <div class="relative">
                <button
                  class="flex items-center gap-1.5 text-body-sm rounded-md px-2 py-1 -mx-2
                         text-[var(--text-faint)] hover:text-[var(--text)] hover:bg-[var(--bg-subtle)] transition-colors"
                  aria-haspopup="menu"
                  aria-expanded={openMenu === "relation"}
                  onclick={(e) => toggleMenu("relation", e)}
                >
                  <Plus size={13} />
                  Add relation
                </button>
                {#if openMenu === "relation"}{@render menuPanel(relationOptions)}{/if}
              </div>
            {/if}
          </div>
        {/if}

        <div class="meta-section flex flex-col gap-1 text-caption text-[var(--text-faint)]">
          <span>Created <TimeAgo date={issue.created_at.replace(" ", "T")} class="text-[var(--text-muted)]" /></span>
          <span>Updated <TimeAgo date={issue.updated_at.replace(" ", "T")} class="text-[var(--text-muted)]" /></span>
        </div>
      </div>
    {/if}
  {/snippet}
</DocumentDetail>

{#if issue}
  <IssuePickerModal
    bind:open={relationPickerOpen}
    projectId={issue.project_id}
    {projectIdentifier}
    crossProject
    title={addingRelation ? `Add “${addingRelation.kind.label}” relation` : "Link an issue"}
    onSelect={addRelation}
  />
  <MoveIssueModal bind:open={moveOpen} {issue} onMoved={handleMoved} />
{/if}

<!-- One popover shell for every picker in the view. -->
{#snippet menuPanel(options: import("svelte").Snippet)}
  <!-- svelte-ignore a11y_no_static_element_interactions, a11y_click_events_have_key_events -->
  <div class="prop-menu py-1 w-[190px]" role="menu" tabindex="-1" onclick={(e) => e.stopPropagation()}>
    {@render options()}
  </div>
{/snippet}

{#snippet statusOptions()}
  {#each STATUSES as s (s)}
    <button role="menuitem" class="menu-item" class:menu-item-on={s === issue?.status} onclick={() => setStatus(s)}>
      <StatusIcon status={s} size={14} />
      {statusLabel(s)}
    </button>
  {/each}
{/snippet}

{#snippet priorityOptions()}
  {#each PRIORITIES as p (p)}
    <button role="menuitem" class="menu-item" class:menu-item-on={p === issue?.priority} onclick={() => setPriority(p)}>
      <PriorityIcon priority={p} />
      {priorityLabel(p)}
    </button>
  {/each}
{/snippet}

{#snippet moduleOptions()}
  <button role="menuitem" class="menu-item text-[var(--text-faint)]" class:menu-item-on={issue?.module_id === null} onclick={() => setModule(null)}>
    None
  </button>
  {#each modules as mod (mod.id)}
    <button role="menuitem" class="menu-item" class:menu-item-on={mod.id === issue?.module_id} onclick={() => setModule(mod.id)}>
      {#if mod.emoji}<ProjectIcon value={mod.emoji} size={14} class="shrink-0" />{/if}
      <span class="truncate">{mod.name}</span>
    </button>
  {/each}
{/snippet}

{#snippet relationOptions()}
  {#each RELATION_KINDS as kind (kind.field)}
    <button role="menuitem" class="menu-item" onclick={() => startAddRelation(kind)}>{kind.label}</button>
  {/each}
{/snippet}

<script lang="ts" module>
  function relationIdClass(field: string): string {
    switch (field) {
      case "blocked_by": return "text-[var(--error)]";
      case "blocks": return "text-[var(--accent)]";
      default: return "text-[var(--text-muted)]";
    }
  }

  function priorityTextClass(priority: string): string {
    switch (priority) {
      case "urgent": return "text-[var(--error)]";
      case "high": return "text-[var(--warn)]";
      case "medium": return "text-[var(--accent)]";
      default: return "text-[var(--text)]";
    }
  }
</script>

<style>
  /* Properties read as label / value rows. */
  .issue-props {
    display: flex;
    flex-direction: column;
    gap: 0.125rem;
  }
  .issue-props :global(> .issue-meta-field) {
    display: grid;
    grid-template-columns: 4.75rem minmax(0, 1fr);
    align-items: start;
    gap: 0.5rem;
  }
  .issue-props :global(> .issue-meta-field > .issue-meta-field-label) {
    padding-top: 0.4rem;
    text-transform: none;
    letter-spacing: 0;
    font-size: var(--text-caption);
    font-weight: 500;
    color: var(--text-muted);
  }
  .meta-section {
    border-top: 1px solid var(--border);
    margin: 0 -1.25rem;
    padding: 1.125rem 1.25rem 0;
  }
  .prop-trigger {
    display: flex;
    align-items: center;
    gap: 0.5rem;
    width: 100%;
    min-width: 0;
    text-align: left;
    font-size: var(--text-body-sm);
    border-radius: 0.375rem;
    padding: 0.25rem 0.5rem;
    margin: 0 -0.5rem;
    cursor: default;
    transition: background-color 120ms;
  }
  .prop-trigger-edit {
    cursor: pointer;
  }
  .prop-trigger-edit:hover,
  .prop-trigger-edit[aria-expanded="true"] {
    background: var(--bg-subtle);
  }
  .prop-menu {
    position: absolute;
    left: -0.5rem;
    top: 100%;
    margin-top: 0.25rem;
    z-index: 30;
    background: var(--surface);
    border: 1px solid var(--border);
    border-radius: 0.5rem;
    box-shadow: 0 10px 24px -8px rgb(0 0 0 / 0.35);
    animation: popover-in 140ms var(--ease-out-expo);
  }
  .menu-item {
    width: 100%;
    display: flex;
    align-items: center;
    gap: 0.5rem;
    padding: 0.375rem 0.75rem;
    text-align: left;
    font-size: var(--text-body-sm);
    color: var(--text);
    transition: background-color 100ms;
  }
  .menu-item:hover {
    background: var(--bg-subtle);
  }
  .menu-item-on {
    color: var(--accent);
    background: var(--accent-subtle);
  }
  .strip-chip {
    display: inline-flex;
    align-items: center;
    gap: 0.35rem;
    height: 1.6rem;
    padding: 0 0.55rem;
    border-radius: 999px;
    border: 1px solid var(--border);
    font-size: var(--text-caption);
    color: var(--text-muted);
    max-width: 14rem;
    overflow: hidden;
    white-space: nowrap;
    text-overflow: ellipsis;
  }
</style>

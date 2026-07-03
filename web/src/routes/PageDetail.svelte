<script lang="ts">
  import {
    getPage,
    updatePage,
    deletePage,
    downloadPageExport,
    listPageComments,
    createPageComment,
    listLabels,
    listPageActivity,
    type Page,
    type Comment,
    type Label,
    type Activity,
  } from "../lib/api";
  import DocumentDetail from "../lib/DocumentDetail.svelte";
  import LabelEditor from "../lib/LabelEditor.svelte";
  import Select from "../lib/Select.svelte";
  import { formatDate } from "../lib/format";
  import { recordRecent } from "../lib/home/recents"; // LIF-237
  import { startAutoRefresh } from "../lib/autoRefresh.svelte";
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
    editable = true,
  }: {
    navigate: (path: string) => void;
    projectIdentifier: string;
    pageId: number;
    editable?: boolean;
  } = $props();

  let page = $state<Page | null>(null);
  let comments = $state<Comment[]>([]);
  let activity = $state<Activity[]>([]);
  // LIF-105: project labels available for attachment. Stays empty for
  // workspace pages (project_id === null) — labels are project-scoped.
  let labels = $state<Label[]>([]);
  let loading = $state(true);
  let error = $state("");

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
    const [res, actRes] = await Promise.all([
      getPage(pageId),
      listPageActivity(pageId),
    ]);
    if (gen !== loadGen) return; // navigated away mid-flight — discard
    if (res.ok) page = res.data;
    if (actRes.ok) activity = actRes.data.items;
  }

  $effect(() =>
    startAutoRefresh({
      refresh: refreshPage,
      // Also skip while a navigation load is running (loading) so a focus
      // event can't fire a redundant fetch on top of the mount load.
      isBusy: () => bodyMode === "edit" || saving || loading,
      // Focus-only — no background interval for the page editor.
      intervalMs: 0,
    }),
  );

  async function loadPage(id: number) {
    const gen = ++loadGen;
    loading = true;
    error = "";
    comments = [];
    labels = [];
    const res = await getPage(id);
    if (gen !== loadGen) return; // a newer navigation superseded this load
    if (!res.ok) { error = res.error; loading = false; return; }
    page = res.data;
    recordRecent({ type: "page", routeId: String(page.id), identifier: page.identifier, title: page.title, project: projectIdentifier }); // LIF-237

    // Load page comments and (project) labels in parallel. Workspace
    // pages skip the labels fetch — they can't carry any (LIF-105).
    const tasks: Promise<unknown>[] = [
      listPageComments(page.id).then((r) => { if (gen === loadGen && r.ok) comments = r.data; }),
      listPageActivity(page.id).then((r) => { if (gen === loadGen && r.ok) activity = r.data.items; }),
    ];
    if (page.project_id !== null) {
      tasks.push(
        listLabels(page.project_id).then((r) => { if (gen === loadGen && r.ok) labels = r.data; }),
      );
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

  async function handleNewComment(content: string) {
    if (!page) return null;
    const res = await createPageComment(page.id, content);
    if (!res.ok) return null;
    comments = [...comments, res.data];
    return res.data;
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
    return false;
  }

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
  {editable}
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
  {activity}
  {paletteActions}
  layout="wide"
  bind:bodyMode
>
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

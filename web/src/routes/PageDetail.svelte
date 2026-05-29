<script lang="ts">
  import {
    getPage,
    updatePage,
    deletePage,
    downloadPageExport,
    listPageComments,
    createPageComment,
    listLabels,
    type Page,
    type Comment,
    type Label,
  } from "../lib/api";
  import DocumentDetail from "../lib/DocumentDetail.svelte";
  import LabelEditor from "../lib/LabelEditor.svelte";
  import { formatDate } from "../lib/format";

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

  $effect(() => {
    const id = pageId;
    lastSaved = null;
    loadPage(id);
  });

  async function loadPage(id: number) {
    loading = true;
    error = "";
    comments = [];
    labels = [];
    const res = await getPage(id);
    if (!res.ok) { error = res.error; loading = false; return; }
    page = res.data;

    // Load page comments and (project) labels in parallel. Workspace
    // pages skip the labels fetch — they can't carry any (LIF-105).
    const tasks: Promise<unknown>[] = [
      listPageComments(page.id).then((r) => { if (r.ok) comments = r.data; }),
    ];
    if (page.project_id !== null) {
      tasks.push(
        listLabels(page.project_id).then((r) => { if (r.ok) labels = r.data; }),
      );
    }
    await Promise.all(tasks);

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
</script>

<DocumentDetail
  {navigate}
  {loading}
  {error}
  identifier={page?.identifier ?? ""}
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
  layout="wide"
>
  {#snippet belowTitle()}
    <!-- LIF-105: labels strip. Sits between title and body, mirroring
         the issue sidebar's chip+picker UX but laid out horizontally
         since pages have no sidebar. Workspace pages skip it entirely —
         labels are project-scoped. -->
    {#if page && page.project_id !== null}
      <div class="mb-6">
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
      </div>
    {/if}
  {/snippet}

  {#snippet metaFooter()}
    {#if page}
      <div class="mt-10 pt-6 border-t border-[var(--border)] flex gap-8">
        <div>
          <span class="block text-[0.6875rem] font-semibold uppercase tracking-widest text-[var(--text-faint)] mb-0.5">
            Created
          </span>
          <span class="text-[0.8125rem] text-[var(--text-muted)]">
            {formatDate(page.created_at)}
          </span>
        </div>
        <div>
          <span class="block text-[0.6875rem] font-semibold uppercase tracking-widest text-[var(--text-faint)] mb-0.5">
            Updated
          </span>
          <span class="text-[0.8125rem] text-[var(--text-muted)]">
            {formatDate(page.updated_at)}
          </span>
        </div>
      </div>
    {/if}
  {/snippet}
</DocumentDetail>

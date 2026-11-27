<script lang="ts">
  // LIF-524: a project's Conventions tab. With text, it is the conventions
  // page itself (editor, history, comments); before that, the templates to
  // start from. The page's row comes from the project's read model, so the
  // tab follows a page created, filled in or deleted anywhere.

  import { listProjects, type Project } from "../lib/api";
  import {
    cacheProjects,
    cachedProject,
    ensureProjectModel,
    setActiveProject,
    type ProjectReadModel,
  } from "../lib/sync/readModel.svelte";
  import { projectRole, loadProjectRole } from "../lib/projectRole.svelte";
  import ErrorState from "../lib/ErrorState.svelte";
  import Skeleton from "../lib/Skeleton.svelte";
  import ConventionsChooser from "../lib/pages/ConventionsChooser.svelte";
  import { conventionsState } from "../lib/pages/conventions";
  import PageDetail from "./PageDetail.svelte";

  let {
    navigate,
    projectIdentifier,
  }: {
    navigate: (path: string) => void;
    projectIdentifier: string;
  } = $props();

  let project = $state<Project | null>(null);
  let model = $state<ProjectReadModel | null>(null);
  let error = $state("");

  /** The page on screen. Once the editor shows a page it stays for this
   *  visit, even if its text is cleared while someone types; it goes only
   *  when the page itself leaves the project. */
  let editingId = $state<number | null>(null);
  let editingSeen = false;

  const conv = $derived(conventionsState(model?.pageList ?? []));
  const ready = $derived(model?.status === "ready");

  $effect(() => {
    const ident = projectIdentifier;
    editingId = null;
    editingSeen = false;
    void load(ident);
  });

  async function load(ident: string) {
    error = "";
    const cached = cachedProject(ident);
    if (cached) attach(cached);
    else {
      project = null;
      model = null;
    }
    const res = await listProjects();
    if (ident !== projectIdentifier) return;
    if (!res.ok) {
      if (!cached) error = res.error;
      return;
    }
    cacheProjects(res.data);
    const found = res.data.find((p) => p.identifier.toLowerCase() === ident.toLowerCase());
    if (!found) {
      error = `Project ${ident} not found`;
      return;
    }
    attach(found);
  }

  function attach(found: Project) {
    project = found;
    model = ensureProjectModel(found.id);
    setActiveProject(found.id);
    loadProjectRole(found.id);
  }

  $effect(() => {
    if (conv.kind === "page" && editingId !== conv.page.id) {
      editingId = conv.page.id;
      editingSeen = true;
      return;
    }
    if (editingId === null || !ready) return;
    const present = conv.kind !== "none" && conv.page.id === editingId;
    if (present) editingSeen = true;
    else if (editingSeen) {
      // Deleted (here or elsewhere): back to the templates.
      editingId = null;
      editingSeen = false;
    }
  });

  // A model still loading may not hold the page yet; wait rather than
  // offer templates for a page that exists.
  const waiting = $derived(!error && (model === null || (!ready && conv.kind === "none")));
</script>

{#if error}
  <ErrorState title="Couldn't load conventions" message={error}>
    <button
      class="text-body-sm font-medium text-[var(--btn-success-text)] bg-[var(--btn-success)] px-3 py-1.5 rounded-md hover:bg-[var(--btn-success-hover)] transition-colors"
      onclick={() => void load(projectIdentifier)}
    >
      Try again
    </button>
  </ErrorState>
{:else if editingId !== null}
  <PageDetail {navigate} {projectIdentifier} pageId={editingId} conventionsTab />
{:else if waiting || !project}
  <div class="max-w-[960px] mx-auto px-4 py-6 sm:px-10 sm:py-8 flex flex-col gap-4" aria-busy="true">
    <Skeleton variant="bar" class="h-7 w-48" />
    <Skeleton variant="bar" class="h-3.5 w-full max-w-[560px]" />
    <Skeleton variant="bar" class="h-48 w-full mt-2" />
  </div>
{:else if conv.kind !== "page"}
  <ConventionsChooser
    state={conv}
    projectId={project.id}
    {projectIdentifier}
    canEdit={projectRole.canEdit}
    onStarted={(id) => {
      editingId = id;
      editingSeen = false;
    }}
  />
{/if}

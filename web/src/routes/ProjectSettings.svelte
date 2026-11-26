<script lang="ts">
  // Project settings: everything that changes a project rather than
  // describing it, moved off the overview so neither page buries the
  // other. A section index on the left follows the scroll; each section
  // keeps the panel it always had (labels, members, publishing, archive,
  // import), and the irreversible actions sit together at the end.
  import {
    listProjects,
    listIssues,
    listUsers,
    me,
    getInstance,
    updateProject,
    deleteProject,
    archiveProject,
    downloadProjectExport,
    listProjectGroups,
    assignProjectGroup,
    type Project,
    type ProjectGroup,
    type Issue,
    type UserSummary,
  } from "../lib/api";
  import {
    RECENT_AUTH_ERROR,
    needsReauth,
    reauthenticateWithoutPassword,
    reauthenticateWithPassword,
    retryOnceAfterReauth,
  } from "../lib/reauth";
  import IconPicker from "../lib/IconPicker.svelte";
  import LabelManager from "../lib/LabelManager.svelte";
  import ProjectMembers from "../lib/ProjectMembers.svelte";
  import ImportPanel from "../lib/ImportPanel.svelte";
  import PublishPanel from "../lib/PublishPanel.svelte"; // LIF-465
  import ArchiveTransferPanel from "../lib/ArchiveTransferPanel.svelte";
  import ProjectTopbar from "../lib/overview/ProjectTopbar.svelte";
  import { loadListState, saveListState } from "../lib/issues/persistence";
  import ProjectIcon from "../lib/ProjectIcon.svelte";
  import CopyIdButton from "../lib/CopyIdButton.svelte";
  import { Check, Download, AlertTriangle, Lock } from "lucide-svelte";
  import ErrorState from "../lib/ErrorState.svelte";
  import Skeleton from "../lib/Skeleton.svelte";
  import { getContext, tick } from "svelte";
  // LIF-234: role-aware affordance gating. `canManage` = lead/admin (or
  // enforcement off): identity edits, members, import and the danger zone.
  // `canEdit` = maintainer/admin: label management.
  import { projectRole, loadProjectRole, deriveCanPublish } from "../lib/projectRole.svelte";
  import { toast } from "../lib/toast/toast.svelte";

  const topbarCtx = getContext<{
    set: (s: import("svelte").Snippet | undefined) => void;
  } | undefined>("lific:topbar");
  $effect(() => {
    topbarCtx?.set(topbarContent);
    return () => topbarCtx?.set(undefined);
  });

  let {
    navigate,
    projectIdentifier,
    section = null,
    onProjectChange,
  }: {
    navigate: (path: string) => void;
    projectIdentifier: string;
    /** Section to scroll to once loaded (`?section=labels`). */
    section?: string | null;
    onProjectChange?: () => void;
  } = $props();

  let project = $state<Project | null>(null);
  let loading = $state(true);
  let error = $state("");
  let issues = $state<Issue[]>([]);
  let users = $state<UserSummary[]>([]);

  let draftName = $state("");
  let draftDesc = $state("");
  let savedAt = $state(0);

  // Danger zone
  let newIdent = $state("");
  let identError = $state("");
  let renaming = $state(false);
  let leadValue = $state("");
  let lastLead = $state<number | null>(null);
  let showDeleteSection = $state(false);
  let deleteConfirmText = $state("");
  let deleting = $state(false);
  let deleteError = $state("");
  let showArchiveSection = $state(false);
  let archiving = $state(false);
  let archiveError = $state("");
  let exportError = $state("");
  let exporting = $state(false);

  $effect(() => {
    const id = projectIdentifier;
    loadAll(id);
  });

  async function loadAll(ident: string) {
    loading = true;
    error = "";
    const projRes = await listProjects();
    if (!projRes.ok) { error = projRes.error; loading = false; return; }
    const found = projRes.data.find((p: Project) => p.identifier === ident);
    if (!found) { error = `Project ${ident} not found`; loading = false; return; }
    project = found;
    draftName = found.name;
    draftDesc = found.description;
    // LIF-234: prime the shared role store so `canManage`/`canEdit` are
    // right for this project (Layout primes it too; the store dedupes).
    loadProjectRole(found.id);
    leadValue = found.lead_user_id == null ? "" : String(found.lead_user_id);
    lastLead = found.lead_user_id;

    const [issuesRes, usersRes, meRes, instanceRes] = await Promise.all([
      listIssues({ project_id: found.id, limit: 1000 }),
      listUsers(),
      // Who is signed in, and whether this instance signs in without a
      // password: both decide how a refused lead assignment recovers.
      me(),
      getInstance(),
    ]);
    if (meRes.ok) currentUserId = meRes.data.id;
    if (instanceRes.ok) webAutoLogin = instanceRes.data.web_auto_login;
    if (issuesRes.ok) issues = issuesRes.data;
    if (usersRes.ok) users = usersRes.data;
    await loadGroups();
    loading = false;
    if (section) {
      await tick();
      document.getElementById(`settings-${section}`)?.scrollIntoView({ block: "start" });
    }
  }

  // LIF-357: LabelManager only needs usage counts refreshed, and those come
  // from `issues` alone. Reloading everything would flash the skeleton
  // under an open label editor.
  async function refreshLabelUsage() {
    if (!project) return;
    const res = await listIssues({ project_id: project.id, limit: 1000 });
    if (res.ok) issues = res.data;
  }

  function markSaved() {
    savedAt = Date.now();
    window.setTimeout(() => { if (Date.now() - savedAt >= 1900) savedAt = 0; }, 2000);
  }

  // ── Field-level autosave (no Save button) ────────────
  async function saveField(field: string, value: unknown) {
    if (!project) return false;
    const res = await updateProject(project.id, { [field]: value });
    if (res.ok) {
      project = res.data;
      onProjectChange?.();
      markSaved();
      return true;
    }
    toast(`Couldn't save changes: ${res.error}`, { kind: "error" });
    return false;
  }

  function commitName() {
    const v = draftName.trim();
    if (!project) return;
    if (!v) { draftName = project.name; return; }
    if (v !== project.name) saveField("name", v);
  }
  function commitDesc() {
    const v = draftDesc.trim();
    if (project && v !== project.description) saveField("description", v);
  }

  // ── Lead change ─────────────────────────────────────────
  //
  // Naming a lead grants that person a `lead` membership on the project, so
  // the server requires a sign-in from the last 15 minutes, exactly as adding
  // a member does. Clearing the lead grants nothing and is never gated.
  //
  // The pending operation is a snapshot: `leadValue` is a live selector and
  // `project` can be replaced by a reload, so a retry must not read either.
  type PendingLead = {
    projectId: number;
    leadUserId: number;
    previousLead: number | null;
    userId: number;
  };
  let pendingLead = $state<PendingLead | null>(null);
  let leadPassword = $state("");
  let leadBusy = $state(false);
  let leadError = $state("");
  let autoLeadNote = $state("");
  let webAutoLogin = $state(false);
  let currentUserId = $state<number | null>(null);

  /** Put the selector, and the effect's own guard, back to the stored lead.
   *  Restoring only `leadValue` would leave `lastLead` holding the value that
   *  was refused, so the effect would see them differ and immediately re-fire
   *  the write it just abandoned. */
  function restoreLead(previous: number | null) {
    lastLead = previous;
    leadValue = previous == null ? "" : String(previous);
  }

  async function autoLeadReauth() {
    if (!webAutoLogin || currentUserId == null) {
      return { ok: false as const, error: RECENT_AUTH_ERROR, recoverable: true };
    }
    const auto = await reauthenticateWithoutPassword(currentUserId);
    if (!auto.ok) autoLeadNote = auto.error;
    return auto;
  }

  async function assignLead(pending: PendingLead, recover = true): Promise<boolean> {
    const res = await retryOnceAfterReauth(
      () => updateProject(pending.projectId, { lead_user_id: pending.leadUserId }),
      () =>
        recover
          ? autoLeadReauth()
          : Promise.resolve({ ok: false as const, error: RECENT_AUTH_ERROR, recoverable: false }),
    );
    if (res.ok) {
      if (project?.id === pending.projectId) {
        project = res.data;
        onProjectChange?.();
        markSaved();
      }
      return true;
    }
    if (needsReauth(res)) {
      pendingLead = pending;
      return false;
    }
    toast(`Couldn't save changes: ${res.error}`, { kind: "error" });
    restoreLead(pending.previousLead);
    return false;
  }

  /** Verify, then retry the interrupted assignment exactly once. */
  async function submitLeadReauth() {
    const pending = pendingLead;
    if (!pending || leadBusy || !leadPassword || currentUserId == null) return;
    if (pending.projectId !== project?.id || pending.userId !== currentUserId) {
      leadError = "That change was interrupted and was not applied.";
      pendingLead = null;
      leadPassword = "";
      return;
    }
    leadBusy = true;
    leadError = "";
    const refreshed = await reauthenticateWithPassword(leadPassword, pending.userId);
    leadPassword = "";
    if (!refreshed.ok) {
      leadError = refreshed.error;
      leadBusy = false;
      return;
    }
    autoLeadNote = "";
    if (await assignLead(pending, false)) {
      pendingLead = null;
    } else if (!leadError) {
      leadError = "That still was not accepted. Sign out and sign back in, then try again.";
      restoreLead(pending.previousLead);
      pendingLead = null;
    }
    leadBusy = false;
  }

  function cancelLeadReauth() {
    if (pendingLead) restoreLead(pendingLead.previousLead);
    pendingLead = null;
    leadPassword = "";
    leadError = "";
    autoLeadNote = "";
  }

  // Autosaves on select; guarded so the initial hydrate doesn't fire a write.
  $effect(() => {
    const v = leadValue;
    if (!project) return;
    if (pendingLead || leadBusy) return;
    const next = v === "" ? null : Number(v);
    if (next === lastLead) return;
    const previous = lastLead;
    lastLead = next;
    if (next == null) {
      // Clearing the lead is a reduction: no grant, no prompt.
      const projectId = project.id;
      leadBusy = true;
      void saveField("lead_user_id", null)
        .then((saved) => {
          if (!saved && project?.id === projectId) restoreLead(previous);
        })
        .finally(() => { leadBusy = false; });
      return;
    }
    leadBusy = true;
    void assignLead({
      projectId: project.id,
      leadUserId: next,
      previousLead: previous,
      userId: currentUserId ?? -1,
    }).finally(() => { leadBusy = false; });
  });

  // ── Sidebar group ────────────────────────────────────────
  // Not a saveField: the group is per-user and rides its own endpoint, not
  // updateProject. Unlike Lead this autosaves on change rather than through
  // an effect, so a rejected write can put the select back where it was.
  let groups = $state<ProjectGroup[]>([]);
  let selectedGroupId = $state<number | null>(null);

  function currentGroupId(): number | null {
    if (!project) return null;
    const id = project.id;
    return groups.find((g) => g.project_ids.includes(id))?.id ?? null;
  }

  async function loadGroups() {
    const res = await listProjectGroups();
    if (!res.ok) return;
    groups = res.data;
    selectedGroupId = currentGroupId();
  }

  async function saveGroup(groupId: number | null) {
    if (!project) return;
    const res = await assignProjectGroup(project.id, groupId);
    if (!res.ok) {
      toast(`Couldn't save changes: ${res.error}`, { kind: "error" });
      // Put the select back so it never shows a value the server rejected.
      selectedGroupId = currentGroupId();
      return;
    }
    const latest = await listProjectGroups();
    if (latest.ok) groups = latest.data;
    onProjectChange?.();
    markSaved();
  }

  const canManage = $derived(projectRole.canManage);

  // LIF-465: publication is NOT a `canManage` affordance. `canManage` is true
  // for everyone while `authz_enforced` is off, but the server's publication
  // gate is the Lead gate, the one check legacy mode still enforces. Derived
  // from the real answer instead: instance admin, a `lead` membership, or
  // the project's own lead pointer (which a pre-LIF-195 project can have
  // without a membership row).
  const canPublish = $derived(
    deriveCanPublish({
      role: projectRole.role,
      enforced: projectRole.enforced,
      isAdmin: projectRole.isAdmin,
      isLead: project != null && currentUserId != null && project.lead_user_id === currentUserId,
    }),
  );

  // #1: open the issue list pre-filtered to a label. The list hydrates its
  // filters from persisted per-project state on mount, so stamp the label
  // filter there (merging with whatever else is stored) before navigating.
  function openLabelInIssues(name: string) {
    const s = loadListState(projectIdentifier);
    saveListState(projectIdentifier, { ...s, filterLabel: [name] });
    navigate(`/${projectIdentifier}/issues`);
  }

  let leadOptions = $derived([
    { value: "", label: "No lead" },
    ...users.map((u) => ({ value: String(u.id), label: u.display_name || u.username })),
  ]);

  async function renameIdentifier() {
    if (!project) return;
    const nid = newIdent.trim().toUpperCase();
    if (!nid || nid === project.identifier) return;
    renaming = true; identError = "";
    const res = await updateProject(project.id, { identifier: nid });
    if (res.ok) {
      project = res.data;
      onProjectChange?.();
      newIdent = "";
      renaming = false;
      navigate(`/${res.data.identifier}/settings`);
    } else {
      identError = res.error;
      renaming = false;
    }
  }

  const total = $derived(issues.length);
  let deleteReady = $derived(project != null && deleteConfirmText === project.identifier);
  async function handleDelete() {
    if (!project || !deleteReady) return;
    deleting = true; deleteError = "";
    const res = await deleteProject(project.id);
    if (res.ok) navigate("/");
    else { deleteError = res.error; deleting = false; }
  }

  async function handleArchive() {
    if (!project || archiving) return;
    archiving = true; archiveError = "";
    const res = await archiveProject(project.id);
    if (res.ok) {
      toast(`Archived ${res.data.identifier}. An admin can unarchive it from Settings, Instance.`, { kind: "success" });
      navigate("/");
    } else {
      archiveError = res.error;
      archiving = false;
    }
  }

  async function exportProject() {
    if (!project || exporting) return;
    exporting = true; exportError = "";
    const res = await downloadProjectExport(project.identifier);
    if (!res.ok) exportError = res.error;
    exporting = false;
  }

  // ── Section index with scroll-spy ────────────────────────
  const sections = $derived(
    [
      { id: "general", label: "General", show: true },
      { id: "labels", label: "Labels", show: true },
      { id: "members", label: "Members", show: canManage },
      { id: "sharing", label: "Public access", show: canPublish },
      { id: "data", label: "Import and export", show: true },
      { id: "danger", label: "Danger zone", show: canManage },
    ].filter((s) => s.show),
  );
  let activeSection = $state("general");
  let scroller = $state<HTMLElement | null>(null);

  $effect(() => {
    if (loading || !scroller) return;
    void sections;
    const root = scroller;
    const update = () => {
      // The last section whose top has passed a line a little below the
      // top of the scroller is the one being read. At the very bottom, the
      // last section wins even when it is too short to reach that line.
      const line = root.getBoundingClientRect().top + 96;
      let current = sections[0]?.id ?? "general";
      for (const s of sections) {
        const el = document.getElementById(`settings-${s.id}`);
        if (el && el.getBoundingClientRect().top <= line) current = s.id;
      }
      if (root.scrollTop + root.clientHeight >= root.scrollHeight - 4) {
        current = sections[sections.length - 1]?.id ?? current;
      }
      activeSection = current;
    };
    update();
    root.addEventListener("scroll", update, { passive: true });
    return () => root.removeEventListener("scroll", update);
  });

  function jump(id: string) {
    activeSection = id;
    document.getElementById(`settings-${id}`)?.scrollIntoView({ behavior: "smooth", block: "start" });
  }

  const inputClass =
    "w-full text-body-sm rounded-md border border-[var(--border)] bg-[var(--bg)] text-[var(--text)] " +
    "px-2.5 py-1.5 outline-none transition-colors focus:border-[var(--accent)] " +
    "focus-visible:ring-2 focus-visible:ring-[var(--accent-subtle)] disabled:opacity-60";
</script>

{#snippet topbarContent()}
  <ProjectTopbar identifier={projectIdentifier} current="settings" {navigate}>
    {#snippet actions()}
      {#if savedAt}
        <span class="inline-flex items-center gap-1 text-caption text-[var(--success)]" aria-live="polite">
          <Check size={12} /> Saved
        </span>
      {/if}
    {/snippet}
  </ProjectTopbar>
{/snippet}

{#snippet row(label: string, help: string, control: import("svelte").Snippet, forId?: string)}
  <div class="flex flex-col sm:flex-row sm:items-start gap-2 sm:gap-8 px-4 py-4 border-t border-[var(--border)] first:border-t-0">
    <div class="sm:w-52 shrink-0">
      {#if forId}
        <label for={forId} class="block text-body-sm font-medium text-[var(--text)]">{label}</label>
      {:else}
        <p class="text-body-sm font-medium text-[var(--text)]">{label}</p>
      {/if}
      <p class="text-caption text-[var(--text-muted)] mt-0.5 leading-relaxed">{help}</p>
    </div>
    <div class="flex-1 min-w-0">{@render control()}</div>
  </div>
{/snippet}

{#snippet sectionHead(id: string, title: string, note?: string)}
  <div class="mb-3">
    <h2 id="settings-{id}-heading" class="text-[1.15rem] font-semibold tracking-tight text-[var(--text)]">{title}</h2>
    {#if note}<p class="text-body-sm text-[var(--text-muted)] mt-0.5">{note}</p>{/if}
  </div>
{/snippet}

{#if loading}
  <div class="h-full overflow-y-auto">
    <div class="max-w-[1100px] mx-auto px-4 md:px-10 py-8 flex gap-10">
      <div class="hidden lg:flex w-44 shrink-0 flex-col gap-3 pt-1">
        {#each [0, 1, 2, 3, 4] as i (i)}<Skeleton variant="bar" class="h-3.5 w-24" />{/each}
      </div>
      <div class="flex-1 min-w-0 flex flex-col gap-3">
        <Skeleton variant="bar" class="h-5 w-28" />
        <Skeleton variant="block" class="h-[320px] w-full rounded-xl" />
      </div>
    </div>
  </div>
{:else if !project}
  <ErrorState title="Couldn't load this project" message={error}>
    <button
      class="text-body-sm font-medium text-[var(--btn-success-text)] bg-[var(--btn-success)] px-3 py-1.5 rounded-md hover:bg-[var(--btn-success-hover)] transition-colors"
      onclick={() => loadAll(projectIdentifier)}
    >
      Try again
    </button>
    <button
      class="text-body-sm text-[var(--text-muted)] border border-[var(--border)] px-3 py-1.5 rounded-md hover:bg-[var(--bg-subtle)] transition-colors"
      onclick={() => navigate("/")}
    >
      Back to home
    </button>
  </ErrorState>
{:else}
  <div class="h-full overflow-y-auto" bind:this={scroller}>
    <div class="max-w-[1100px] mx-auto px-4 md:px-10 pt-8 pb-24 lg:flex lg:gap-12">
      <!-- Section index: a sticky column on wide screens, a scrolling row
           of chips above the content on narrow ones. -->
      <nav aria-label="Settings sections" class="settings-nav lg:w-44 lg:shrink-0">
        <ul class="flex lg:flex-col gap-1 overflow-x-auto lg:overflow-visible pb-3 lg:pb-0 -mx-1 px-1
                   [scrollbar-width:none] [&::-webkit-scrollbar]:hidden">
          {#each sections as s (s.id)}
            <li class="shrink-0">
              <button
                class="w-full text-left whitespace-nowrap px-2.5 py-1.5 rounded-md text-body-sm transition-colors
                       {activeSection === s.id
                  ? 'bg-[var(--bg-subtle)] text-[var(--text)] font-medium'
                  : 'text-[var(--text-muted)] hover:text-[var(--text)] hover:bg-[var(--bg-subtle)]'}
                       {s.id === 'danger' ? 'lg:mt-3' : ''}"
                aria-current={activeSection === s.id ? "true" : undefined}
                onclick={() => jump(s.id)}
              >
                <span class={s.id === "danger" ? "text-[var(--error)]" : ""}>{s.label}</span>
              </button>
            </li>
          {/each}
        </ul>
      </nav>

      <div class="flex-1 min-w-0 max-w-[780px] flex flex-col gap-12">
        {#if !canManage && projectRole.enforced}
          <p class="flex items-center gap-2 text-body-sm text-[var(--text-muted)] rounded-lg bg-[var(--bg-subtle)] px-3.5 py-2.5">
            <Lock size={14} class="shrink-0" />
            Only a project lead or an admin can change these settings. Your sidebar group is yours to set.
          </p>
        {/if}

        <!-- ── GENERAL ─────────────────────────────────────── -->
        <section id="settings-general" class="scroll-mt-6" aria-labelledby="settings-general-heading">
          {@render sectionHead("general", "General", "How the project shows up everywhere in Lific.")}
          <div class="rounded-xl bg-[var(--surface)] shadow-[0_1px_2px_rgba(0,0,0,0.06)]">
            {#snippet iconControl()}
              {#if canManage}
                <IconPicker value={project!.emoji ?? ""} onchange={(v) => saveField("emoji", v || null)} />
              {:else}
                <div class="size-12 grid place-items-center rounded-xl bg-[var(--bg-subtle)]">
                  {#if project!.emoji}
                    <ProjectIcon value={project!.emoji} size={22} />
                  {:else}
                    <span class="text-body-sm font-mono font-semibold text-[var(--text-muted)]">{project!.identifier.slice(0, 2)}</span>
                  {/if}
                </div>
              {/if}
            {/snippet}
            {@render row("Icon", "Shown in the sidebar and the overview.", iconControl)}

            {#snippet nameControl()}
              <input
                id="project-name"
                bind:value={draftName}
                disabled={!canManage}
                class="{inputClass} max-w-[36ch]"
                onblur={commitName}
                onkeydown={(e) => {
                  if (e.key === "Enter") (e.currentTarget as HTMLInputElement).blur();
                  if (e.key === "Escape") { draftName = project!.name; (e.currentTarget as HTMLInputElement).blur(); }
                }}
              />
            {/snippet}
            {@render row("Name", "Saved when you leave the field.", nameControl, "project-name")}

            {#snippet descControl()}
              <textarea
                id="project-description"
                bind:value={draftDesc}
                disabled={!canManage}
                rows="3"
                placeholder="What is this project for?"
                class="{inputClass} resize-y leading-relaxed"
                onblur={commitDesc}
                onkeydown={(e) => {
                  if (e.key === "Escape") { draftDesc = project!.description; (e.currentTarget as HTMLTextAreaElement).blur(); }
                  if (e.key === "Enter" && (e.metaKey || e.ctrlKey)) (e.currentTarget as HTMLTextAreaElement).blur();
                }}
              ></textarea>
            {/snippet}
            {@render row("Description", "One or two sentences on the overview, under the name.", descControl, "project-description")}

            {#snippet identControl()}
              <div class="flex items-center gap-3 flex-wrap">
                <CopyIdButton
                  value={project!.identifier}
                  label={project!.identifier}
                  iconSize={12}
                  class="inline-flex items-center gap-1.5 text-body-sm font-mono font-semibold px-2 py-1 rounded-md
                         bg-[var(--bg-subtle)] text-[var(--text)] hover:text-[var(--accent)] transition-colors"
                >
                  {project!.identifier}
                </CopyIdButton>
                {#if canManage}
                  <button class="text-caption text-[var(--text-muted)] hover:text-[var(--text)] underline underline-offset-2"
                          onclick={() => jump("danger")}>
                    Change it in the danger zone
                  </button>
                {/if}
              </div>
            {/snippet}
            {@render row("Identifier", `Prefix for every issue, page and plan, like ${project.identifier}-42.`, identControl)}

            {#snippet leadControl()}
              <select
                id="project-lead"
                bind:value={leadValue}
                disabled={!canManage || pendingLead !== null || leadBusy}
                class="{inputClass} max-w-[36ch]"
              >
                {#each leadOptions as o (o.value)}
                  <option value={o.value}>{o.label}</option>
                {/each}
              </select>
              <!-- Naming a lead grants that person a lead membership, so the
                   server wants a recent sign-in. Confirm inline, right under
                   the control that was refused. -->
              {#if pendingLead}
                <div class="mt-3 flex flex-col gap-2 rounded-md bg-[var(--bg-subtle)] px-3 py-2.5">
                  <p class="text-caption text-[var(--text)] leading-relaxed">
                    Verify it's you to make that person the project lead. It grants them lead
                    access to this project, and you have been signed in for a while.
                  </p>
                  {#if autoLeadNote}
                    <p class="text-caption text-[var(--text-muted)]" role="status">
                      Signing you in automatically did not work ({autoLeadNote}).
                    </p>
                  {/if}
                  {#if leadError}
                    <p class="text-caption text-[var(--error)]" role="alert">{leadError}</p>
                  {/if}
                  <div class="flex items-center gap-2 flex-wrap">
                    <label class="flex-1 min-w-[170px]">
                      <span class="sr-only">Your current password</span>
                      <input
                        bind:value={leadPassword}
                        type="password"
                        placeholder="your current password"
                        autocomplete="current-password"
                        disabled={leadBusy}
                        class={inputClass}
                        onkeydown={(e) => { if (e.key === "Enter") submitLeadReauth(); }}
                      />
                    </label>
                    <button
                      class="text-body-sm font-medium text-[var(--btn-success-text)] bg-[var(--btn-success)]
                             px-3 py-1.5 rounded-md hover:bg-[var(--btn-success-hover)] transition-colors
                             disabled:opacity-40 disabled:cursor-not-allowed shrink-0"
                      disabled={leadBusy || !leadPassword}
                      onclick={submitLeadReauth}
                    >
                      {leadBusy ? "Verifying…" : "Confirm and continue"}
                    </button>
                    <button
                      class="text-body-sm text-[var(--text-muted)] px-3 py-1.5 rounded-md hover:bg-[var(--surface)] transition-colors disabled:opacity-50 shrink-0"
                      disabled={leadBusy}
                      onclick={cancelLeadReauth}
                    >
                      Cancel
                    </button>
                  </div>
                </div>
              {/if}
            {/snippet}
            {@render row("Lead", "Who owns this project. Naming someone makes them a lead member.", leadControl, "project-lead")}

            <!-- Personal to the signed-in user, so it needs no role gate:
                 nobody else's sidebar changes. -->
            {#snippet groupControl()}
              <select
                id="project-group"
                value={selectedGroupId === null ? "" : String(selectedGroupId)}
                onchange={(e) => {
                  const v = e.currentTarget.value;
                  selectedGroupId = v === "" ? null : Number(v);
                  saveGroup(selectedGroupId);
                }}
                class="{inputClass} max-w-[36ch]"
              >
                <option value="">No group</option>
                {#each groups as group (group.id)}
                  <option value={String(group.id)}>{group.name}</option>
                {/each}
              </select>
            {/snippet}
            {@render row("Sidebar group", "Where this project sits in your sidebar. Only you see it.", groupControl, "project-group")}
          </div>
        </section>

        <!-- ── LABELS ──────────────────────────────────────── -->
        <!-- LIF-234: label management is a maintainer-level structure edit;
             a viewer sees labels read-only. -->
        <section id="settings-labels" class="scroll-mt-6" aria-labelledby="settings-labels-heading">
          {@render sectionHead("labels", "Labels", "Shared by issues and pages. Click a count to see what carries it.")}
          <LabelManager
            showHeading={false}
            projectId={project.id}
            {issues}
            canEdit={projectRole.canEdit}
            onChange={refreshLabelUsage}
            onOpenLabel={openLabelInIssues}
          />
        </section>

        <!-- ── MEMBERS (LIF-200) ───────────────────────────── -->
        {#if canManage}
          <section id="settings-members" class="scroll-mt-6" aria-labelledby="settings-members-heading">
            {@render sectionHead("members", "Members", "Who can see this project, and what each person may change.")}
            <ProjectMembers projectId={project.id} showHeading={false} />
          </section>
        {/if}

        <!-- ── PUBLIC VIEW (LIF-465) ───────────────────────── -->
        <!-- Gated on `canPublish`, deliberately NOT on `canManage`: see the
             derivation above. Publishing is a deliberate, normal act, not a
             destructive one, so it stays out of the danger zone. -->
        {#if canPublish}
          <section id="settings-sharing" class="scroll-mt-6 flex flex-col gap-8">
            <PublishPanel
              {project}
              onChange={(updated) => { project = updated; }}
            />
          </section>
        {/if}

        <!-- ── IMPORT AND EXPORT ───────────────────────────── -->
        <section id="settings-data" class="scroll-mt-6 flex flex-col gap-8" aria-labelledby="settings-data-heading">
          <div>
            {@render sectionHead("data", "Import and export")}
            <div class="rounded-xl bg-[var(--surface)] shadow-[0_1px_2px_rgba(0,0,0,0.06)]">
              {#snippet exportControl()}
                <div class="flex items-center gap-3 flex-wrap">
                  <button class="toolbar-pill" onclick={exportProject} disabled={exporting}>
                    <Download size={14} />
                    {exporting ? "Exporting…" : "Export as Markdown"}
                  </button>
                  {#if exportError}<span class="text-caption text-[var(--error)]" role="alert">{exportError}</span>{/if}
                </div>
              {/snippet}
              {@render row("Markdown export", "Every issue and page as Markdown files in one download. Leaves the project untouched.", exportControl)}
            </div>
          </div>
          {#if canPublish && project.identifier === projectIdentifier}
            <ArchiveTransferPanel projectId={project.id} identifier={project.identifier} />
          {/if}
          {#if canManage}
            <ImportPanel projectId={project.id} onImported={() => loadAll(projectIdentifier)} />
          {/if}
        </section>

        <!-- ── DANGER ZONE ─────────────────────────────────── -->
        <!-- LIF-234: lead/admin-only. Hidden entirely for everyone else. -->
        {#if canManage}
          <section id="settings-danger" class="scroll-mt-6" aria-labelledby="settings-danger-heading">
            <div class="mb-3 flex items-center gap-2">
              <AlertTriangle size={16} class="text-[var(--error)]" />
              <h2 id="settings-danger-heading" class="text-[1.15rem] font-semibold tracking-tight text-[var(--error)]">Danger zone</h2>
            </div>
            <div class="danger rounded-xl border overflow-hidden">
              <!-- Change identifier -->
              <div class="px-4 py-4 flex flex-col sm:flex-row sm:items-start gap-3 sm:gap-8">
                <div class="sm:w-52 shrink-0">
                  <p class="text-body-sm font-medium text-[var(--text)]">Change identifier</p>
                  <p class="text-caption text-[var(--text-muted)] mt-0.5 leading-relaxed">
                    Re-keys every issue, page and plan. References to <span class="font-mono">{project.identifier}-NNN</span>
                    written inside other issues and pages stop resolving. It can't be undone automatically.
                  </p>
                </div>
                <div class="flex-1 min-w-0">
                  <div class="flex items-center gap-2">
                    <input
                      bind:value={newIdent}
                      aria-label="New identifier"
                      placeholder={project.identifier}
                      class="w-[130px] px-2.5 py-1.5 text-body-sm font-mono uppercase rounded-md
                             border border-[var(--border)] bg-[var(--bg)] text-[var(--text)]
                             outline-none focus-visible:ring-2 focus-visible:ring-[var(--error)]"
                      onkeydown={(e) => { if (e.key === "Enter") renameIdentifier(); }}
                    />
                    <button
                      class="text-body-sm font-medium text-[var(--error-text)] bg-[var(--error)] px-3 py-1.5 rounded-md
                             hover:opacity-90 transition-opacity disabled:opacity-40 disabled:cursor-not-allowed"
                      disabled={renaming || !newIdent.trim() || newIdent.trim().toUpperCase() === project.identifier}
                      onclick={renameIdentifier}
                    >
                      {renaming ? "Renaming…" : "Rename"}
                    </button>
                  </div>
                  {#if newIdent.trim() && newIdent.trim().toUpperCase() !== project.identifier}
                    <p class="text-caption text-[var(--text-muted)] mt-1.5 font-mono">
                      {project.identifier}-1 becomes {newIdent.trim().toUpperCase()}-1
                    </p>
                  {/if}
                  {#if identError}<p class="text-caption text-[var(--error)] mt-1.5" role="alert">{identError}</p>{/if}
                </div>
              </div>

              <!-- Archive -->
              <div class="danger-rule px-4 py-4 flex flex-col sm:flex-row sm:items-start gap-3 sm:gap-8">
                <div class="sm:w-52 shrink-0">
                  <p class="text-body-sm font-medium text-[var(--text)]">Archive project</p>
                  <p class="text-caption text-[var(--text-muted)] mt-0.5 leading-relaxed">
                    Packs the project into one file on the server and removes it from Lific. An instance admin can
                    bring it back from Settings, Instance.
                  </p>
                </div>
                <div class="flex-1 min-w-0">
                  {#if !showArchiveSection}
                    <button
                      class="text-body-sm text-[var(--error)] border border-[var(--error)] px-3 py-1.5 rounded-md hover:bg-[var(--error-bg)] transition-colors"
                      onclick={() => { showArchiveSection = true; }}
                    >
                      Archive this project
                    </button>
                  {:else}
                    <p class="text-caption text-[var(--text-muted)] mb-2 leading-relaxed">
                      <strong class="font-mono">{project.identifier}</strong> and its <strong>{total}</strong> issue{total !== 1 ? "s" : ""},
                      pages, plans and attachments disappear for everyone until it is unarchived. It comes back private, with its
                      members and history. Page revision history, saved views and repository bindings are not kept.
                    </p>
                    <div class="flex items-center gap-2">
                      <button
                        class="text-body-sm font-medium text-[var(--error-text)] bg-[var(--error)] px-3 py-1.5 rounded-md
                               hover:opacity-90 transition-opacity disabled:opacity-40 disabled:cursor-not-allowed"
                        disabled={archiving}
                        onclick={handleArchive}
                      >
                        {archiving ? "Archiving…" : "Archive"}
                      </button>
                      <button class="text-body-sm text-[var(--text-muted)] px-2 py-1.5 rounded-md hover:bg-[var(--bg-subtle)] transition-colors"
                              disabled={archiving}
                              onclick={() => { showArchiveSection = false; archiveError = ""; }}>
                        Cancel
                      </button>
                    </div>
                  {/if}
                  {#if archiveError}<p class="text-caption text-[var(--error)] mt-1.5" role="alert">{archiveError}</p>{/if}
                </div>
              </div>

              <!-- Delete -->
              <div class="danger-rule px-4 py-4 flex flex-col sm:flex-row sm:items-start gap-3 sm:gap-8">
                <div class="sm:w-52 shrink-0">
                  <p class="text-body-sm font-medium text-[var(--text)]">Delete project</p>
                  <p class="text-caption text-[var(--text-muted)] mt-0.5 leading-relaxed">
                    Permanently deletes the project and everything in it. There is no undo.
                  </p>
                </div>
                <div class="flex-1 min-w-0">
                  {#if !showDeleteSection}
                    <button
                      class="text-body-sm text-[var(--error)] border border-[var(--error)] px-3 py-1.5 rounded-md hover:bg-[var(--error-bg)] transition-colors"
                      onclick={() => { showDeleteSection = true; }}
                    >
                      Delete this project
                    </button>
                  {:else}
                    <p class="text-caption text-[var(--text-muted)] mb-2 leading-relaxed">
                      Deletes all <strong>{total}</strong> issue{total !== 1 ? "s" : ""}, modules, labels, folders, pages and plans.
                      Type <strong class="font-mono">{project.identifier}</strong> to confirm.
                    </p>
                    <div class="flex items-center gap-2 flex-wrap">
                      <input
                        bind:value={deleteConfirmText}
                        aria-label="Type the project identifier to confirm"
                        placeholder={project.identifier}
                        class="w-[140px] px-2.5 py-1.5 text-body-sm font-mono rounded-md border border-[var(--error)]
                               bg-[var(--bg)] text-[var(--text)] outline-none focus-visible:ring-2 focus-visible:ring-[var(--error)]"
                      />
                      <button
                        class="text-body-sm font-medium text-[var(--error-text)] bg-[var(--error)] px-3 py-1.5 rounded-md
                               hover:opacity-90 transition-opacity disabled:opacity-40 disabled:cursor-not-allowed"
                        disabled={!deleteReady || deleting}
                        onclick={handleDelete}
                      >
                        {deleting ? "Deleting…" : "Delete permanently"}
                      </button>
                      <button class="text-body-sm text-[var(--text-muted)] px-2 py-1.5 rounded-md hover:bg-[var(--bg-subtle)] transition-colors"
                              onclick={() => { showDeleteSection = false; deleteConfirmText = ""; deleteError = ""; }}>
                        Cancel
                      </button>
                    </div>
                    {#if deleteError}<p class="text-caption text-[var(--error)] mt-1.5" role="alert">{deleteError}</p>{/if}
                  {/if}
                </div>
              </div>
            </div>
          </section>
        {/if}
      </div>
    </div>
  </div>
{/if}

<style>
  .danger {
    border-color: color-mix(in oklab, var(--error) 35%, transparent);
  }
  .danger-rule {
    border-top: 1px solid color-mix(in oklab, var(--error) 18%, transparent);
  }
  @media (min-width: 1024px) {
    .settings-nav ul {
      position: sticky;
      top: 2rem;
    }
  }
</style>

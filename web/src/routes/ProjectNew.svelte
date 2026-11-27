<script lang="ts">
  import {
    createProject,
    assignProjectGroup,
    getInstance,
    getArchiveCapabilities,
    me,
    type CreateProjectInput,
    type ConventionTemplateKey,
  } from "../lib/api";
  import {
    RECENT_AUTH_ERROR,
    needsReauth,
    reauthenticateWithPassword,
    reauthenticateWithoutPassword,
    retryOnceAfterReauth,
  } from "../lib/reauth";
  import { toast } from "../lib/toast/toast.svelte";
  import ProjectForm from "../lib/ProjectForm.svelte";
  import ProjectIcon from "../lib/ProjectIcon.svelte";
  import StatusIcon from "../lib/StatusIcon.svelte";
  import { ArrowLeft, ArrowRight, ChevronRight, CircleAlert, FileText, Lock, PackageOpen } from "lucide-svelte";
  import { getContext, onMount } from "svelte";

  let { navigate }: { navigate: (path: string) => void } = $props();

  const topbarCtx = getContext<{
    set: (s: import("svelte").Snippet | undefined) => void;
  } | undefined>("lific:topbar");

  $effect(() => {
    topbarCtx?.set(topbarContent);
    return () => topbarCtx?.set(undefined);
  });

  let name = $state("");
  let identifier = $state("");
  let description = $state("");
  let emoji = $state("");
  let leadUserId = $state<number | null>(null);
  let groupId = $state<number | null>(null);
  let conventions = $state<ConventionTemplateKey>("empty");
  let saving = $state(false);
  let error = $state("");
  let currentUserId = $state<number | null>(null);
  let webAutoLogin = $state(false);
  let canImportArchive = $state(false);

  onMount(() => {
    let live = true;
    getArchiveCapabilities().then((res) => {
      if (live) canImportArchive = res.ok && res.data.can_import;
    });
    return () => { live = false; };
  });

  type PendingCreate = {
    input: CreateProjectInput;
    groupId: number | null;
    userId: number;
  };
  let pendingCreate = $state<PendingCreate | null>(null);
  let reauthPassword = $state("");
  let reauthBusy = $state(false);
  let reauthError = $state("");
  let autoReauthNote = $state("");

  let identifierError = $state<string | null>(null);
  let canSave = $derived(name.trim().length > 0 && identifier.trim().length > 0 && !identifierError);
  let shownId = $derived(identifier.trim().toUpperCase() || "PRO");
  const modKey = /Mac|iPhone|iPad/.test(navigator.platform) ? "⌘" : "Ctrl";

  function onKeydown(e: KeyboardEvent) {
    if (e.key === "Enter" && (e.metaKey || e.ctrlKey) && !e.defaultPrevented) {
      e.preventDefault();
      save();
    }
  }

  onMount(async () => {
    const [user, instance] = await Promise.all([me(), getInstance()]);
    if (user.ok) currentUserId = user.data.id;
    if (instance.ok) webAutoLogin = instance.data.web_auto_login;
  });

  async function autoReauth(userId: number) {
    if (!webAutoLogin) {
      return { ok: false as const, error: RECENT_AUTH_ERROR, recoverable: true };
    }
    const refreshed = await reauthenticateWithoutPassword(userId);
    if (!refreshed.ok) autoReauthNote = refreshed.error;
    return refreshed;
  }

  async function attemptCreate(pending: PendingCreate, recover = true) {
    const res = await retryOnceAfterReauth(
      () => createProject(pending.input),
      () =>
        recover
          ? autoReauth(pending.userId)
          : Promise.resolve({
              ok: false as const,
              error: RECENT_AUTH_ERROR,
              recoverable: false,
            }),
    );

    if (res.ok) {
      // The group rides a separate endpoint, so it can only be filed once the
      // project has an id. If that call fails the project still exists — say
      // so rather than landing the user on the overview with their choice
      // silently dropped.
      if (pending.groupId !== null) {
        const assigned = await assignProjectGroup(res.data.id, pending.groupId);
        if (!assigned.ok) {
          toast(`Project created, but it wasn't added to the group: ${assigned.error}`, {
            kind: "error",
          });
        }
      }
      pendingCreate = null;
      navigate(`/${res.data.identifier}/overview`);
      return;
    }
    if (needsReauth(res)) {
      pendingCreate = pending;
      reauthError = "";
      saving = false;
      return;
    }
    pendingCreate = null;
    error = res.error;
    saving = false;
  }

  async function save() {
    if (!canSave || saving || pendingCreate) return;
    saving = true;
    error = "";
    if (currentUserId == null) {
      const user = await me();
      if (user.ok) currentUserId = user.data.id;
    }
    if (currentUserId == null) {
      error = "Couldn't verify the current user. Reload and try again.";
      saving = false;
      return;
    }
    await attemptCreate({
      input: {
        name: name.trim(),
        identifier: identifier.trim().toUpperCase(),
        description: description.trim() || undefined,
        emoji: emoji.trim() || undefined,
        lead_user_id: leadUserId ?? undefined,
        conventions,
      },
      groupId,
      userId: currentUserId,
    });
  }

  async function submitReauth() {
    const pending = pendingCreate;
    if (!pending || reauthBusy || !reauthPassword || currentUserId !== pending.userId) return;
    reauthBusy = true;
    reauthError = "";
    const refreshed = await reauthenticateWithPassword(reauthPassword, pending.userId);
    reauthPassword = "";
    if (!refreshed.ok) {
      reauthError = refreshed.error;
      reauthBusy = false;
      return;
    }
    autoReauthNote = "";
    saving = true;
    await attemptCreate(pending, false);
    if (pendingCreate && !error && !reauthError) {
      reauthError = "That still was not accepted. Sign out and back in, then try again.";
    }
    reauthBusy = false;
  }

  function cancelReauth() {
    pendingCreate = null;
    reauthPassword = "";
    reauthError = "";
    autoReauthNote = "";
    saving = false;
  }
</script>

<svelte:window onkeydown={onKeydown} />

<div class="new-project h-full overflow-y-auto">
  <div class="np-split max-w-[1180px] mx-auto px-4 md:px-10 pt-8 md:pt-12 pb-16">
    <div class="min-w-0 max-w-[760px] flex flex-col gap-6">
      <ProjectForm
        bind:name
        bind:identifier
        bind:description
        bind:emoji
        bind:leadUserId
        bind:groupId
        bind:conventions
        bind:identifierError
      />
      {#if error}
        <p class="flex items-start gap-2 text-body-sm text-[var(--error)] rounded-lg bg-[var(--error-bg)] px-3.5 py-2.5" role="alert">
          <CircleAlert size={15} class="shrink-0 mt-0.5" />
          <span>Couldn't create the project: {error}</span>
        </p>
      {/if}
    {#if pendingCreate}
      <div>
        <div class="rounded-lg border border-[var(--border)] bg-[var(--bg-subtle)] p-4 flex flex-col gap-3">
          <div class="flex items-start gap-2 text-body-sm text-[var(--text)]">
            <Lock size={15} class="shrink-0 mt-0.5 text-[var(--text-muted)]" />
            <p>
              Verify it's you to create this project with the selected lead. Granting another
              person access requires a recent sign-in.
            </p>
          </div>
          {#if autoReauthNote}
            <p class="text-caption text-[var(--text-muted)]" role="status">{autoReauthNote}</p>
          {/if}
          <input
            bind:value={reauthPassword}
            type="password"
            autocomplete="current-password"
            placeholder="Current password"
            class="w-full px-3 py-2 text-body-sm rounded-md border border-[var(--border)] bg-[var(--surface)] text-[var(--text)] outline-none focus-visible:ring-2 focus-visible:ring-[var(--accent)]"
            onkeydown={(e) => { if (e.key === "Enter") submitReauth(); }}
          />
          {#if reauthError}
            <p class="text-caption text-[var(--error)]" role="alert">{reauthError}</p>
          {/if}
          <div class="flex gap-2">
            <button
              class="text-body-sm font-medium text-[var(--accent-text)] bg-[var(--accent)] px-3 py-1.5 rounded-md disabled:opacity-40"
              disabled={reauthBusy || !reauthPassword}
              onclick={submitReauth}
            >{reauthBusy ? "Verifying..." : "Verify and create"}</button>
            <button
              class="text-body-sm text-[var(--text-muted)] px-3 py-1.5 rounded-md hover:bg-[var(--surface)]"
              disabled={reauthBusy}
              onclick={cancelReauth}
            >Cancel</button>
          </div>
        </div>
      </div>
    {/if}
    </div>

    <!-- ── ASIDE: where the project will show up ─────────────── -->
    <aside class="np-aside flex flex-col gap-6" aria-label="Preview">
      <div class="hidden xl:block">
        <p class="text-caption font-medium uppercase tracking-wider text-[var(--text-faint)] mb-2">Preview</p>
        <div class="rounded-xl bg-[var(--surface)] shadow-[0_1px_2px_rgba(0,0,0,0.06)] p-3 flex flex-col gap-3">
          <div>
            <p class="text-caption text-[var(--text-muted)] px-1 mb-1.5">In the sidebar</p>
            <div class="flex items-center gap-2.5 rounded-lg px-2 py-1.5 bg-[var(--chart-hover)]">
              {#if emoji}
                <span class="size-6 grid place-items-center shrink-0"><ProjectIcon value={emoji} size={18} /></span>
              {:else}
                <span class="size-6 grid place-items-center shrink-0 rounded-md bg-[var(--chart-tile)] text-caption font-medium tracking-tight text-[var(--text-muted)]">{shownId.slice(0, 2)}</span>
              {/if}
              <span class="truncate flex-1 text-body-sm {name.trim() ? 'text-[var(--text)]' : 'text-[var(--text-faint)]'}">{name.trim() || "Untitled project"}</span>
              <ChevronRight size={14} class="text-[var(--text-faint)]" />
            </div>
          </div>
          <div class="border-t border-[var(--border)] pt-3">
            <p class="text-caption text-[var(--text-muted)] px-1 mb-1.5">Its links</p>
            <div class="flex items-center gap-2.5 px-2 py-1.5">
              <StatusIcon status="todo" size={14} />
              <span class="font-mono text-caption font-semibold text-[var(--text-muted)] shrink-0">{shownId}-1</span>
              <span class="h-2 flex-1 rounded-full bg-[var(--chart-tile)]"></span>
            </div>
            <div class="flex items-center gap-2.5 px-2 py-1.5">
              <FileText size={14} class="text-[var(--text-faint)] shrink-0" />
              <span class="font-mono text-caption font-semibold text-[var(--text-muted)] shrink-0">{shownId}-DOC-1</span>
              <span class="truncate text-caption text-[var(--text-muted)]">Conventions</span>
            </div>
          </div>
        </div>
      </div>

      {#if canImportArchive}
        <div class="flex items-start gap-3 px-3.5 py-3">
          <span class="size-8 grid place-items-center rounded-lg bg-[var(--chart-tile)] text-[var(--text-muted)] shrink-0">
            <PackageOpen size={16} />
          </span>
          <div class="min-w-0 flex-1">
            <p class="text-body-sm text-[var(--text-muted)]">Moving from another Lific instance?</p>
            <button
              class="inline-flex items-center gap-1 text-body-sm font-medium text-[var(--text)] hover:text-[var(--accent)] rounded transition-colors"
              onclick={() => navigate("/projects/import")}
            >
              Import a project archive <ArrowRight size={13} aria-hidden="true" />
            </button>
          </div>
        </div>
      {/if}
    </aside>
  </div>
</div>

{#snippet topbarContent()}
  <div class="flex items-center gap-3 px-6 py-2 w-full">
    <div class="flex items-center gap-1.5 shrink-0">
      <button
        class="flex items-center gap-1.5 text-body-sm text-[var(--text-muted)]
               hover:text-[var(--text)] transition-colors rounded px-1.5 py-0.5
               hover:bg-[var(--bg-subtle)]"
        onclick={() => navigate("/settings")}
      >
        <ArrowLeft size={14} />
        Back
      </button>
      <span class="text-[var(--text-faint)]">/</span>
      <span class="text-body-sm font-medium text-[var(--text)]">New project</span>
    </div>

    <div class="ml-auto flex items-center gap-2 shrink-0">
      {#if error}
        <span class="text-body-sm text-[var(--error)] max-w-[min(280px,30vw)] truncate" title={error}>
          {error}
        </span>
      {/if}
      <kbd class="hidden md:inline text-caption font-mono text-[var(--text-faint)]" aria-hidden="true">{modKey}+Enter</kbd>
      <button
        class="text-body-sm text-[var(--text-muted)] px-2.5 py-1
               rounded-md hover:bg-[var(--bg-subtle)] transition-colors"
        onclick={() => navigate("/settings")}
      >
        Cancel
      </button>
      <button
        class="text-body-sm font-medium text-[var(--accent-text)]
               bg-[var(--accent)] px-2.5 py-1 rounded-md
               hover:bg-[var(--accent-hover)] transition-colors
               disabled:opacity-40 disabled:cursor-not-allowed"
        disabled={!canSave || saving || pendingCreate !== null}
        title="{modKey}+Enter"
        onclick={save}
      >
        {saving ? "Creating..." : "Create project"}
      </button>
    </div>
  </div>
{/snippet}

<style>
  .new-project {
    --chart-hover: color-mix(in oklab, var(--text) 4.5%, transparent);
    --chart-tile: color-mix(in oklab, var(--text) 7.5%, transparent);
  }
  .np-split {
    display: grid;
    grid-template-columns: minmax(0, 1fr);
    row-gap: 2.5rem;
  }
  @media (min-width: 1280px) {
    .np-split {
      grid-template-columns: minmax(0, 1fr) 280px;
      column-gap: 3rem;
    }
    .np-aside {
      position: sticky;
      top: 3rem;
      align-self: start;
    }
  }
</style>

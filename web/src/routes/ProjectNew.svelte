<script lang="ts">
  import {
    createProject,
    assignProjectGroup,
    getInstance,
    me,
    type CreateProjectInput,
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
  import { ArrowLeft, Lock } from "lucide-svelte";
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
  let saving = $state(false);
  let error = $state("");
  let currentUserId = $state<number | null>(null);
  let webAutoLogin = $state(false);

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

  let canSave = $derived(name.trim().length > 0 && identifier.trim().length > 0);

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

<div class="h-full flex flex-col">
  <!-- Form -->
  <div class="flex-1 overflow-y-auto">
    <ProjectForm
      bind:name
      bind:identifier
      bind:description
      bind:emoji
      bind:leadUserId
      bind:groupId
      mode="create"
    />
    {#if pendingCreate}
      <div class="mx-auto mb-8 w-full max-w-[680px] px-6">
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
        onclick={save}
      >
        {saving ? "Creating..." : "Create project"}
      </button>
    </div>
  </div>
{/snippet}

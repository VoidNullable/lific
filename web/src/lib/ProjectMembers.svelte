<script lang="ts">
  // LIF-200: project members management. Mirrors LabelManager's section
  // shape (header + rounded card + create row + list) so Overview reads as
  // one consistent settings page rather than a bolt-on.
  //
  // Username → user_id resolution (see LIF-200 report): `GET /api/users`
  // (api.ts `listUsers`) is NOT admin-gated — any authenticated user can
  // list {id, username, display_name, is_admin, created_at} for every
  // human account. So the "add member" control is a proper name-driven
  // Select built from that list (filtered to non-members), not a raw
  // numeric user_id input. No backend gap here.
  import {
    me,
    listUsers,
    listProjectMembers,
    getInstance,
    addProjectMember,
    changeProjectMemberRole,
    removeProjectMember,
    type AuthUser,
    type UserSummary,
    type ProjectMember,
    type ProjectRole,
  } from "./api";
  import {
    RECENT_AUTH_ERROR,
    needsReauth,
    reauthenticateWithoutPassword,
    reauthenticateWithPassword,
    retryOnceAfterReauth,
  } from "./reauth";
  import { UsersRound, UserPlus, Trash2, Lock } from "lucide-svelte";
  import Select from "./Select.svelte";
  import Skeleton from "./Skeleton.svelte";
  import { formatDate } from "./format";

  let { projectId }: { projectId: number } = $props();

  let currentUser = $state<AuthUser | null>(null);
  let members = $state<ProjectMember[]>([]);
  let allUsers = $state<UserSummary[]>([]);
  let loading = $state(true);
  let loadError = $state("");

  // Am I allowed to manage membership? Instance admin, or a `lead` row for
  // this project. Read-only for everyone else (viewer/maintainer/non-member
  // who can still see the list while enforcement is off).
  let amLead = $derived(
    !!currentUser?.is_admin ||
      members.some((m) => m.user_id === currentUser?.id && m.role === "lead"),
  );

  $effect(() => {
    const id = projectId;
    load(id);
  });

  async function load(id: number) {
    loading = true;
    loadError = "";
    const [meRes, membersRes] = await Promise.all([me(), listProjectMembers(id)]);
    if (meRes.ok) currentUser = meRes.data;
    if (!membersRes.ok) {
      loadError = membersRes.error;
      loading = false;
      return;
    }
    members = membersRes.data;
    loading = false;
  }

  // Users list only matters for the add-member picker, and only a lead
  // needs it — fetch lazily once we know that.
  let usersLoaded = $state(false);
  $effect(() => {
    if (amLead && !usersLoaded) {
      usersLoaded = true;
      listUsers().then((res) => { if (res.ok) allUsers = res.data; });
      // Whether this instance signs in without a password decides how a stale
      // session is recovered when a grant is refused.
      getInstance().then((res) => { if (res.ok) webAutoLogin = res.data.web_auto_login; });
    }
  });

  const ROLE_LABEL: Record<ProjectRole, string> = {
    lead: "Lead",
    maintainer: "Maintainer",
    viewer: "Viewer",
  };
  const ROLE_BADGE: Record<ProjectRole, string> = {
    lead: "text-[var(--success)] bg-[var(--success-bg)]",
    maintainer: "text-[var(--accent)] bg-[var(--accent-subtle)]",
    viewer: "text-[var(--text-muted)] bg-[var(--bg-subtle)]",
  };
  const ROLE_OPTIONS: { value: ProjectRole; label: string }[] = [
    { value: "viewer", label: "Viewer" },
    { value: "maintainer", label: "Maintainer" },
    { value: "lead", label: "Lead" },
  ];

  function initials(name: string): string {
    return name.split(/[\s_-]+/).slice(0, 2).map((w) => w[0]?.toUpperCase() ?? "").join("");
  }

  // ── Add member ──────────────────────────────────────────────
  let addUserId = $state<number | null>(null);
  let addRole = $state<ProjectRole>("viewer");
  let adding = $state(false);
  let addError = $state("");

  let eligibleUsers = $derived(
    allUsers
      // LIF-214: a deactivated account can't sign in, so it has no business
      // being offered as someone to hand a project role to.
      .filter((u) => u.is_active)
      .filter((u) => !members.some((m) => m.user_id === u.id))
      .map((u) => ({ value: u.id, label: u.display_name || u.username, username: u.username })),
  );

  // ── Re-authentication for grants ────────────────────────────
  //
  // Adding a member, or raising an existing member's role, hands out standing
  // access to the project, so the server wants a sign-in from the last 15
  // minutes. Lowering a role and removing a member are reductions and are
  // never gated, so containment stays one click.
  //
  // One prompt serves both grants and remembers exactly which was interrupted,
  // so confirming resumes that operation with its own arguments rather than
  // whatever the pickers happen to show by then.
  // Snapshots, not references. `projectId` is a prop that changes when the
  // user navigates, and `member.role` is mutated optimistically, so a retry
  // that read either back off the live state could act on the wrong project or
  // the wrong previous role.
  type PendingGrant =
    | { kind: "add"; projectId: number; userId: number; role: ProjectRole }
    | {
        kind: "role";
        projectId: number;
        userId: number;
        username: string;
        role: ProjectRole;
        previousRole: ProjectRole;
      };
  let pendingGrant = $state<PendingGrant | null>(null);
  let grantPassword = $state("");
  let grantBusy = $state(false);
  let grantError = $state("");
  let autoGrantNote = $state("");
  let webAutoLogin = $state(false);

  /** The automatic route out of a staleness refusal, where one exists.
   *  A failure or a session belonging to another account is *recoverable*:
   *  note it and let the password prompt take over. */
  async function autoReauth() {
    if (!webAutoLogin || !currentUser) {
      return { ok: false as const, error: RECENT_AUTH_ERROR, recoverable: true };
    }
    const auto = await reauthenticateWithoutPassword(currentUser.id);
    if (!auto.ok) autoGrantNote = auto.error;
    return auto;
  }

  function noRecovery() {
    return Promise.resolve({
      ok: false as const,
      error: RECENT_AUTH_ERROR,
      recoverable: false,
    });
  }

  async function attemptAdd(
    target: number,
    userId: number,
    role: ProjectRole,
    recover = true,
  ) {
    const res = await retryOnceAfterReauth(
      () => addProjectMember(target, { user_id: userId, role }),
      () => (recover ? autoReauth() : noRecovery()),
    );
    if (res.ok) {
      // POST returns the bare ProjectMember row (no joined username/
      // display_name), so reload to render the joined identity. Only if the
      // user is still looking at the project this landed on.
      if (target === projectId) {
        await load(projectId);
        addUserId = null;
        addRole = "viewer";
      }
      return true;
    }
    if (needsReauth(res)) {
      // Hold the arguments, and leave the picker showing them.
      pendingGrant = { kind: "add", projectId: target, userId, role };
      return false;
    }
    addError = res.error;
    return false;
  }

  async function addMember() {
    if (
      addUserId == null ||
      adding ||
      roleBusy !== null ||
      pendingGrant !== null ||
      grantBusy
    ) return;
    adding = true;
    addError = "";
    grantError = "";
    await attemptAdd(projectId, addUserId, addRole);
    adding = false;
  }

  // ── Change role ─────────────────────────────────────────────
  let roleError = $state<{ userId: number; message: string } | null>(null);
  let roleBusy = $state<number | null>(null);
  // Bumped on a failed change so the {#key} below forces the Select to
  // remount and re-read `m.role` — it optimistically shows the picked
  // option locally, so a rejected change (e.g. last-lead 409) needs an
  // explicit nudge to snap back rather than staying stuck on the choice.
  let roleResyncTick = $state(0);

  async function attemptSetRole(
    grant: Extract<PendingGrant, { kind: "role" }>,
    recover = true,
  ) {
    const res = await retryOnceAfterReauth(
      () => changeProjectMemberRole(grant.projectId, grant.userId, grant.role),
      () => (recover ? autoReauth() : noRecovery()),
    );
    if (res.ok) {
      if (grant.projectId === projectId) {
        members = members.map((x) =>
          x.user_id === grant.userId ? { ...x, role: grant.role } : x,
        );
      }
      return true;
    }
    if (needsReauth(res)) {
      // The Select is showing the requested role optimistically. Leave it
      // there while the prompt is up so the pending change stays visible; it
      // is snapped back only if the operation is abandoned or genuinely fails.
      pendingGrant = grant;
      return false;
    }
    roleError = { userId: grant.userId, message: res.error };
    roleResyncTick++;
    return false;
  }

  async function setRole(m: ProjectMember, role: ProjectRole) {
    if (
      role === m.role ||
      roleBusy != null ||
      adding ||
      pendingGrant !== null ||
      grantBusy
    ) return;
    roleBusy = m.user_id;
    roleError = null;
    grantError = "";
    await attemptSetRole({
      kind: "role",
      projectId,
      userId: m.user_id,
      username: m.username,
      role,
      previousRole: m.role,
    });
    roleBusy = null;
  }

  /** Verify, then resume the interrupted grant exactly once. */
  async function submitGrantReauth() {
    const target = pendingGrant;
    if (!target || !currentUser || grantBusy || !grantPassword) return;
    grantBusy = true;
    grantError = "";
    const refreshed = await reauthenticateWithPassword(grantPassword, currentUser.id);
    grantPassword = "";
    if (!refreshed.ok) {
      grantError = refreshed.error;
      grantBusy = false;
      return;
    }
    autoGrantNote = "";
    if (target.projectId !== projectId) {
      // They navigated away while the prompt was up. Retrying would act on a
      // project that is no longer on screen, with no way to show the result.
      grantError = "You moved to another project, so that change was not applied.";
      pendingGrant = null;
      grantBusy = false;
      return;
    }
    const landed =
      target.kind === "add"
        ? await attemptAdd(target.projectId, target.userId, target.role, false)
        : await attemptSetRole(target, false);
    if (landed) pendingGrant = null;
    else if (!grantError) {
      grantError = "That still was not accepted. Sign out and sign back in, then try again.";
    }
    grantBusy = false;
  }

  function cancelGrantReauth() {
    // An abandoned role change must not leave the Select showing a role the
    // server never accepted: remount it so it re-reads the stored role.
    if (pendingGrant?.kind === "role") roleResyncTick++;
    pendingGrant = null;
    grantPassword = "";
    grantError = "";
    autoGrantNote = "";
  }

  // ── Remove ──────────────────────────────────────────────────
  let confirmingRemove = $state<number | null>(null);
  let removeBusy = $state<number | null>(null);
  let removeError = $state<{ userId: number; message: string } | null>(null);

  async function removeMember(m: ProjectMember) {
    removeBusy = m.user_id;
    removeError = null;
    const res = await removeProjectMember(projectId, m.user_id);
    removeBusy = null;
    if (res.ok) {
      members = members.filter((x) => x.user_id !== m.user_id);
      confirmingRemove = null;
    } else {
      removeError = { userId: m.user_id, message: res.error };
      confirmingRemove = null;
    }
  }
</script>

<section>
  <div class="flex items-center gap-1.5 mb-3">
    <UsersRound size={14} class="text-[var(--text-muted)]" />
    <h2 class="text-body-sm font-semibold text-[var(--text)]">Members</h2>
    {#if members.length > 0}
      <span class="text-caption font-normal text-[var(--text-faint)] tabular-nums">{members.length}</span>
    {/if}
  </div>

  <div class="rounded-xl bg-[var(--surface)] shadow-[0_1px_2px_rgba(0,0,0,0.06)] overflow-hidden">
    {#if amLead}
      <!-- Add member row -->
      <div class="flex items-center gap-2 px-4 py-3 border-b border-[var(--border)] flex-wrap">
        <Select
          options={eligibleUsers}
          bind:value={addUserId}
          placeholder={eligibleUsers.length === 0 && usersLoaded ? "No one left to add" : "Choose a person…"}
          size="sm"
          class="min-w-[190px] flex-1"
          disabled={pendingGrant !== null || grantBusy || roleBusy !== null || adding}
        >
          {#snippet renderOption(opt, isSelected)}
            <span class="flex flex-col text-body-sm {isSelected ? 'text-[var(--accent)] font-medium' : 'text-[var(--text)]'}">
              {opt.label}
              <span class="text-caption text-[var(--text-faint)]">@{opt.username}</span>
            </span>
          {/snippet}
        </Select>
        <Select
          options={ROLE_OPTIONS}
          bind:value={addRole}
          size="sm"
          class="w-[130px] shrink-0"
          disabled={pendingGrant !== null || grantBusy || roleBusy !== null || adding}
        />
        <button
          class="flex items-center gap-1.5 text-body-sm font-medium text-[var(--btn-success-text)]
                 bg-[var(--btn-success)] px-3 py-1.5 rounded-md hover:bg-[var(--btn-success-hover)]
                 transition-colors disabled:opacity-40 disabled:cursor-not-allowed shrink-0"
          disabled={adding || roleBusy !== null || addUserId == null || pendingGrant !== null || grantBusy}
          onclick={addMember}
        >
          <UserPlus size={14} />
          {adding ? "Adding…" : "Add"}
        </button>
      </div>
      <!-- Shared "verify it's you" prompt for the two granting actions. It
           names which one is waiting, so confirming is never ambiguous. -->
      {#if pendingGrant}
        <div class="px-4 py-3 border-b border-[var(--border)] bg-[var(--bg-subtle)] flex flex-col gap-2">
          <div class="flex items-start gap-2 text-caption text-[var(--text)]">
            <Lock size={13} class="shrink-0 mt-0.5 text-[var(--text-muted)]" />
            <p class="leading-relaxed">
              Verify it's you to
              {#if pendingGrant.kind === "add"}
                add this person as {ROLE_LABEL[pendingGrant.role].toLowerCase()}.
              {:else}
                make <span class="font-mono">@{pendingGrant.username}</span>
                {ROLE_LABEL[pendingGrant.role].toLowerCase()}.
              {/if}
              Granting project access needs a recent sign-in, and you have been signed in for a
              while.
            </p>
          </div>
          {#if autoGrantNote}
            <p class="text-caption text-[var(--text-muted)]" role="status">
              Signing you in automatically did not work ({autoGrantNote}).
            </p>
          {/if}
          {#if grantError}
            <p class="text-caption text-[var(--error)]" role="alert">{grantError}</p>
          {/if}
          <div class="flex items-center gap-2 flex-wrap">
            <label class="flex-1 min-w-[180px]">
              <span class="sr-only">Your current password</span>
              <input
                bind:value={grantPassword}
                type="password"
                placeholder="your current password"
                autocomplete="current-password"
                disabled={grantBusy}
                class="w-full px-3 py-1.5 text-body-sm rounded-md border border-[var(--border)]
                       bg-[var(--bg)] text-[var(--text)] outline-none focus-visible:ring-2
                       focus-visible:ring-[var(--accent)] disabled:opacity-50"
                onkeydown={(e) => { if (e.key === 'Enter') submitGrantReauth(); }}
              />
            </label>
            <button
              class="text-body-sm font-medium text-[var(--btn-success-text)] bg-[var(--btn-success)]
                     px-3 py-1.5 rounded-md hover:bg-[var(--btn-success-hover)] transition-colors
                     disabled:opacity-40 disabled:cursor-not-allowed shrink-0"
              disabled={grantBusy || !grantPassword}
              onclick={submitGrantReauth}
            >
              {grantBusy ? "Verifying…" : "Confirm and continue"}
            </button>
            <button
              class="text-body-sm text-[var(--text-muted)] px-3 py-1.5 rounded-md hover:bg-[var(--surface)] transition-colors disabled:opacity-50 shrink-0"
              disabled={grantBusy}
              onclick={cancelGrantReauth}
            >
              Cancel
            </button>
          </div>
        </div>
      {/if}
      {#if addError}
        <div class="px-4 py-2 text-caption text-[var(--error)] bg-[var(--error-bg)]" role="alert">{addError}</div>
      {/if}
    {/if}

    {#if loading}
      <!-- LIF-281: structural skeleton replacing the centered spinner. The
           section header + rounded card already render around this branch, so
           here we mirror the loaded member rows (avatar + name/username +
           role badge + date, px-4 py-2.5 with top borders) so the card body
           keeps its height and the rows don't snap in. The add-member row is
           amLead-gated and not part of the default view, so it's omitted. -->
      {#each [0, 1, 2] as row (row)}
        <div class="flex items-center gap-3 px-4 py-2.5 {row > 0 ? 'border-t border-[var(--border)]' : ''}">
          <Skeleton variant="circle" class="size-8 shrink-0" />
          <div class="flex-1 min-w-0 flex flex-col gap-1.5">
            <Skeleton variant="bar" class="h-3.5 w-40" />
            <Skeleton variant="bar" class="h-3 w-24" />
          </div>
          <Skeleton variant="bar" class="h-5 w-20 rounded-full shrink-0" />
          <Skeleton variant="bar" class="hidden sm:block h-3 w-[8.5rem] shrink-0" />
        </div>
      {/each}
    {:else if loadError}
      <div class="px-4 py-4 text-body-sm text-[var(--error)]">{loadError}</div>
    {:else if members.length === 0}
      <div class="px-4 py-6 text-center text-body-sm text-[var(--text-faint)]">No members yet.</div>
    {:else}
      {#each members as m, idx (m.user_id)}
        <div class="flex items-center gap-3 px-4 py-2.5 {idx > 0 ? 'border-t border-[var(--border)]' : ''}">
          <div class="size-8 shrink-0 rounded-full bg-[var(--accent)] text-[var(--accent-text)] grid place-items-center text-micro font-semibold tracking-wide">
            {initials(m.display_name || m.username)}
          </div>
          <div class="flex-1 min-w-0">
            <div class="text-body-sm text-[var(--text)] truncate leading-tight">
              {m.display_name || m.username}
              {#if m.user_id === currentUser?.id}
                <span class="text-caption text-[var(--text-faint)]">(you)</span>
              {/if}
            </div>
            <div class="text-caption font-mono text-[var(--text-faint)] truncate leading-tight mt-0.5">@{m.username}</div>
          </div>

          {#if amLead}
            {#key `${m.user_id}:${m.role}:${roleResyncTick}`}
              <Select
                options={ROLE_OPTIONS}
                value={m.role}
                onchange={(opt) => setRole(m, opt.value as ProjectRole)}
                size="sm"
                class="w-[130px] shrink-0"
                disabled={pendingGrant !== null || grantBusy || adding || roleBusy !== null}
              >
                {#snippet renderSelected(opt)}
                  <span class="text-caption font-semibold uppercase tracking-wide px-1.5 py-0.5 rounded-full {ROLE_BADGE[opt.value as ProjectRole]}">
                    {opt.label}
                  </span>
                {/snippet}
              </Select>
            {/key}
          {:else}
            <span class="text-micro font-semibold uppercase tracking-wide px-1.5 py-0.5 rounded-full shrink-0 {ROLE_BADGE[m.role]}">
              {ROLE_LABEL[m.role]}
            </span>
          {/if}

          <span class="hidden sm:block text-caption text-[var(--text-faint)] tabular-nums shrink-0 w-[8.5rem] text-right">
            {formatDate(m.created_at)}
          </span>

          {#if amLead}
            {#if confirmingRemove === m.user_id}
              <div class="flex items-center gap-1.5 shrink-0">
                <button
                  class="text-caption font-medium text-[var(--error-text)] bg-[var(--error)] px-2 py-1 rounded-md
                         hover:opacity-90 transition-opacity disabled:opacity-40"
                  disabled={removeBusy === m.user_id}
                  onclick={() => removeMember(m)}
                >
                  {removeBusy === m.user_id ? "…" : "Remove"}
                </button>
                <button
                  class="text-caption text-[var(--text-muted)] px-2 py-1 rounded-md hover:bg-[var(--bg-subtle)] transition-colors"
                  onclick={() => { confirmingRemove = null; }}
                >
                  Cancel
                </button>
              </div>
            {:else}
              <button
                class="size-7 grid place-items-center rounded-md text-[var(--text-muted)] shrink-0
                       hover:text-[var(--error)] hover:bg-[var(--error-bg)] transition-colors"
                onclick={() => { confirmingRemove = m.user_id; }}
                aria-label="Remove {m.display_name || m.username}"
              >
                <Trash2 size={14} />
              </button>
            {/if}
          {/if}
        </div>
        {#if roleError?.userId === m.user_id}
          <div class="px-4 pb-2.5 -mt-1 text-caption text-[var(--error)]">{roleError.message}</div>
        {/if}
        {#if removeError?.userId === m.user_id}
          <div class="px-4 pb-2.5 -mt-1 text-caption text-[var(--error)]">{removeError.message}</div>
        {/if}
      {/each}
    {/if}
  </div>
  {#if !amLead && !loading && !loadError}
    <p class="text-caption text-[var(--text-faint)] mt-2">
      Read-only — only a project lead can add, change, or remove members.
    </p>
  {/if}
</section>

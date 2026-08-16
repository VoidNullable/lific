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
    addProjectMember,
    changeProjectMemberRole,
    removeProjectMember,
    type AuthUser,
    type UserSummary,
    type ProjectMember,
    type ProjectRole,
  } from "./api";
  import { UsersRound, UserPlus, Trash2 } from "lucide-svelte";
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

  async function addMember() {
    if (addUserId == null || adding) return;
    adding = true;
    addError = "";
    const res = await addProjectMember(projectId, { user_id: addUserId, role: addRole });
    adding = false;
    if (res.ok) {
      // POST returns the bare ProjectMember row (no joined username/
      // display_name) — reload so the new row renders the joined identity.
      await load(projectId);
      addUserId = null;
      addRole = "viewer";
    } else {
      addError = res.error;
    }
  }

  // ── Change role ─────────────────────────────────────────────
  let roleError = $state<{ userId: number; message: string } | null>(null);
  let roleBusy = $state<number | null>(null);
  // Bumped on a failed change so the {#key} below forces the Select to
  // remount and re-read `m.role` — it optimistically shows the picked
  // option locally, so a rejected change (e.g. last-lead 409) needs an
  // explicit nudge to snap back rather than staying stuck on the choice.
  let roleResyncTick = $state(0);

  async function setRole(m: ProjectMember, role: ProjectRole) {
    if (role === m.role || roleBusy != null) return;
    roleBusy = m.user_id;
    roleError = null;
    const res = await changeProjectMemberRole(projectId, m.user_id, role);
    roleBusy = null;
    if (res.ok) {
      members = members.map((x) => (x.user_id === m.user_id ? { ...x, role } : x));
    } else {
      roleError = { userId: m.user_id, message: res.error };
      roleResyncTick++;
    }
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
        />
        <button
          class="flex items-center gap-1.5 text-body-sm font-medium text-[var(--btn-success-text)]
                 bg-[var(--btn-success)] px-3 py-1.5 rounded-md hover:bg-[var(--btn-success-hover)]
                 transition-colors disabled:opacity-40 disabled:cursor-not-allowed shrink-0"
          disabled={adding || addUserId == null}
          onclick={addMember}
        >
          <UserPlus size={14} />
          {adding ? "Adding…" : "Add"}
        </button>
      </div>
      {#if addError}
        <div class="px-4 py-2 text-caption text-[var(--error)] bg-[var(--error-bg)]">{addError}</div>
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

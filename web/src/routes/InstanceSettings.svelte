<script lang="ts">
  // Instance settings (admin-only): edit the DB-backed, runtime instance
  // settings (LIF-210/211/212/213) and view the member roster. Non-admins who
  // reach the URL directly get a friendly gate.
  import {
    me,
    listUsers,
    getInstanceSettings,
    updateInstanceSettings,
    createUser,
    promoteUser,
    demoteUser,
    deactivateUser,
    reactivateUser,
    type AuthUser,
    type UserSummary,
    type InstanceSettings,
    type InstanceSettingsPatch,
  } from "../lib/api";
  import SettingsTabs from "../lib/SettingsTabs.svelte";
  import Skeleton from "../lib/Skeleton.svelte";
  import TimeAgo from "../lib/TimeAgo.svelte";
  import { ShieldCheck, Lock, SlidersHorizontal, Check, AlertTriangle, DoorOpen, DoorClosed, Users, ShieldPlus, ShieldMinus, UserMinus, UserPlus, RotateCcw } from "lucide-svelte";
  import { getContext, onMount } from "svelte";

  let { navigate }: { navigate: (path: string) => void } = $props();

  const topbarCtx = getContext<{
    set: (s: import("svelte").Snippet | undefined) => void;
  } | undefined>("lific:topbar");
  $effect(() => {
    topbarCtx?.set(topbarContent);
    return () => topbarCtx?.set(undefined);
  });

  const host = window.location.host;
  let user = $state<AuthUser | null>(null);
  let users = $state<UserSummary[]>([]);
  let settings = $state<InstanceSettings | null>(null);
  let loading = $state(true);

  // Editable copies.
  let fName = $state("");
  let fSignups = $state(true);
  let fDomains = $state("");
  let fSession = $state(30);
  let fMessage = $state("");
  let fAutoLogin = $state(false);
  let fAuthzEnforced = $state(false);

  let saving = $state(false);
  let saveError = $state("");
  let savedAt = $state(0);

  function hydrate(s: InstanceSettings) {
    settings = s;
    fName = s.instance_name ?? "";
    fSignups = s.allow_signup;
    fDomains = s.signup_email_domains.join(", ");
    fSession = s.session_lifetime_days;
    fMessage = s.login_message ?? "";
    fAutoLogin = s.web_auto_login;
    fAuthzEnforced = s.authz_enforced;
  }

  onMount(async () => {
    const meRes = await me();
    if (meRes.ok) user = meRes.data;
    if (user?.is_admin) {
      const [u, s] = await Promise.all([listUsers(), getInstanceSettings()]);
      if (u.ok) users = u.data;
      if (s.ok) hydrate(s.data);
    }
    loading = false;
  });

  function parseDomains(csv: string): string[] {
    return csv.split(/[,\s]+/).map((d) => d.trim()).filter(Boolean);
  }

  // ── Field-level autosave (no Save button) ───────────────
  // Mirrors ProjectSettings: every control commits its own field — text inputs
  // on blur, toggles on click. We re-sync only the fields named in the patch
  // from the normalized server response, so an in-progress edit in a different
  // field is never clobbered.
  async function commit(patch: InstanceSettingsPatch) {
    if (saving) return;
    saving = true;
    saveError = "";
    const res = await updateInstanceSettings(patch);
    saving = false;
    if (res.ok) {
      settings = res.data;
      if (patch.instance_name !== undefined) fName = res.data.instance_name ?? "";
      if (patch.signup_email_domains !== undefined)
        fDomains = res.data.signup_email_domains.join(", ");
      if (patch.session_lifetime_days !== undefined) fSession = res.data.session_lifetime_days;
      if (patch.login_message !== undefined) fMessage = res.data.login_message ?? "";
      if (patch.allow_signup !== undefined) fSignups = res.data.allow_signup;
      if (patch.web_auto_login !== undefined) fAutoLogin = res.data.web_auto_login;
      if (patch.authz_enforced !== undefined) fAuthzEnforced = res.data.authz_enforced;
      savedAt = Date.now();
      window.setTimeout(() => { if (Date.now() - savedAt >= 1900) savedAt = 0; }, 2000);
    } else {
      saveError = res.error;
    }
  }

  // Per-field commits: only write when the value actually changed, so a blur
  // with no edit (or re-clicking the already-active toggle) is a no-op.
  function commitName() {
    if (settings && fName.trim() !== (settings.instance_name ?? ""))
      commit({ instance_name: fName.trim() });
  }
  function commitDomains() {
    if (settings && parseDomains(fDomains).join(",") !== settings.signup_email_domains.join(","))
      commit({ signup_email_domains: parseDomains(fDomains) });
  }
  function commitSession() {
    // Guard against an emptied number input (NaN/null) — the server would
    // treat it as "no change" anyway, but don't bother round-tripping it.
    if (settings && Number.isFinite(fSession) && fSession !== settings.session_lifetime_days)
      commit({ session_lifetime_days: fSession });
  }
  function commitMessage() {
    if (settings && fMessage.trim() !== (settings.login_message ?? ""))
      commit({ login_message: fMessage.trim() });
  }
  function setSignups(v: boolean) {
    if (settings && v !== fSignups) {
      fSignups = v;
      commit({ allow_signup: v });
    }
  }
  function setAutoLogin(v: boolean) {
    if (settings && v !== fAutoLogin) {
      fAutoLogin = v;
      commit({ web_auto_login: v });
    }
  }
  function setAuthzEnforced(v: boolean) {
    if (settings && v !== fAuthzEnforced) {
      fAuthzEnforced = v;
      commit({ authz_enforced: v });
    }
  }

  function initials(name: string): string {
    return name.split(/[\s_-]+/).slice(0, 2).map((w) => w[0]?.toUpperCase() ?? "").join("");
  }

  const adminCount = $derived(users.filter((u) => u.is_admin && u.is_active).length);

  // ── Member management (LIF-214) ─────────────────────────
  // The instance-admin axis: who administers this instance, and whose account
  // still works. Every action here is admin-only and guard-railed on the
  // server (the last admin can't be demoted or deactivated, and none of it
  // can be pointed at a bot), so a refusal comes back as a message worth
  // showing on the row rather than something to pre-empt in the client.

  // Destructive actions confirm in place, same shape as ProjectMembers'
  // remove: the button swaps for a red confirm plus Cancel.
  type PendingAction = { id: number; kind: "demote" | "deactivate" };
  let pending = $state<PendingAction | null>(null);
  let rowBusy = $state<number | null>(null);
  let rowError = $state<{ id: number; message: string } | null>(null);

  function applyUser(updated: UserSummary) {
    users = users.map((u) => (u.id === updated.id ? updated : u));
  }

  async function runAction(
    u: UserSummary,
    action: (id: number) => ReturnType<typeof promoteUser>,
  ) {
    if (rowBusy !== null) return;
    rowBusy = u.id;
    rowError = null;
    const res = await action(u.id);
    rowBusy = null;
    pending = null;
    if (res.ok) applyUser(res.data);
    else rowError = { id: u.id, message: res.error };
  }

  // ── Create a member ─────────────────────────────────────
  // Deliberately minimal: a username and a password is everything the server
  // needs (it fills in a {username}@local address), and this is an admin
  // handing someone their first credential, not a profile editor.
  let newUsername = $state("");
  let newPassword = $state("");
  let creating = $state(false);
  let createError = $state("");
  let createdName = $state("");

  async function submitCreate() {
    const username = newUsername.trim();
    if (!username || !newPassword || creating) return;
    creating = true;
    createError = "";
    createdName = "";
    const res = await createUser({ username, password: newPassword });
    creating = false;
    if (res.ok) {
      users = [...users, res.data];
      createdName = res.data.username;
      newUsername = "";
      newPassword = "";
    } else {
      createError = res.error;
    }
  }
</script>

{#snippet topbarContent()}
  <div class="flex items-center gap-3 px-6 py-2 w-full">
    <span class="text-body-sm font-medium text-[var(--text)]">Settings</span>
  </div>
{/snippet}

<div class="flex-1 overflow-y-auto">
  <div class="w-full max-w-[1000px] mx-auto px-6 py-10 md:py-12">
    {#if loading}
      <!-- LIF-281: structural skeleton replacing the bare centered spinner.
           The tab bar (SettingsTabs: border-b + mb-8) always renders in both
           the admin and non-admin loaded branches, so it's mirrored first to
           pin the frame. Below it we mirror the admin path's default-visible
           layout — the "Instance" heading block and the settings form column
           (single max-w-[560px] column of labeled fields) — which is the
           expected case for this admin-only route. A non-admin instead sees
           the "Admins only" gate on load; that branch is intentionally not
           mirrored (unpredictable until `me()` resolves), but the tab bar
           keeps the top of the frame from shifting. -->
      <!-- Tab bar stand-in -->
      <div class="flex items-center gap-6 border-b border-[var(--border)] mb-8">
        <Skeleton variant="bar" class="h-4 w-16 mb-2.5 mt-1" />
        <Skeleton variant="bar" class="h-4 w-16 mb-2.5 mt-1" />
      </div>

      <!-- Heading block -->
      <section class="mb-8">
        <Skeleton variant="bar" class="h-7 w-40 mb-2" />
        <Skeleton variant="bar" class="h-4 w-full max-w-[52ch]" />
      </section>

      <!-- Settings form card -->
      <section class="rounded-xl bg-[var(--surface)] shadow-[0_1px_2px_rgba(0,0,0,0.06)] p-5">
        <div class="flex items-center gap-2 mb-5">
          <Skeleton variant="circle" class="size-[15px] rounded" />
          <Skeleton variant="bar" class="h-4 w-24" />
        </div>
        <div class="flex flex-col gap-6 max-w-[560px]">
          {#each [0, 1, 2] as field (field)}
            <div class="flex flex-col gap-1.5">
              <Skeleton variant="bar" class="h-3 w-32" />
              <Skeleton variant="block" class="h-10 w-full rounded-md" />
              <Skeleton variant="bar" class="h-3 w-56" />
            </div>
          {/each}
        </div>
      </section>
    {:else}
      <SettingsTabs active="instance" isAdmin={user?.is_admin ?? false} {navigate} />

      {#if !user?.is_admin}
        <div class="flex flex-col items-center text-center py-20 animate-reveal">
          <div class="size-12 rounded-full bg-[var(--bg-subtle)] grid place-items-center mb-4">
            <Lock size={20} class="text-[var(--text-faint)]" />
          </div>
          <h2 class="text-[1rem] font-semibold text-[var(--text)]">Admins only</h2>
          <p class="text-body text-[var(--text-muted)] mt-1 max-w-[36ch]">
            Instance settings are visible to administrators of this instance.
          </p>
          <button
            class="mt-5 text-body-sm font-medium text-[var(--btn-success-text)] bg-[var(--btn-success)]
                   px-3 py-1.5 rounded-md hover:bg-[var(--btn-success-hover)] transition-colors"
            onclick={() => navigate("/settings")}
          >
            Back to account
          </button>
        </div>
      {:else}
        <section class="mb-8 animate-reveal delay-100">
          <h1 class="font-display text-title tracking-tight text-[var(--text)] leading-none">Instance</h1>
          <p class="text-body text-[var(--text-muted)] mt-2">
            Settings for the Lific instance at <span class="font-mono text-[var(--text)]">{host}</span>.
            Changes apply immediately.
          </p>
        </section>

        <!-- ── SETTINGS FORM ──────────────────────────────── -->
        <section class="rounded-xl bg-[var(--surface)] shadow-[0_1px_2px_rgba(0,0,0,0.06)] p-5 animate-reveal delay-250">
          <div class="flex items-center gap-2 mb-5">
            <SlidersHorizontal size={15} class="text-[var(--text-muted)]" />
            <h2 class="text-body-lg font-semibold text-[var(--text)]">Settings</h2>
            <span class="font-mono text-micro text-[var(--text-faint)] px-1.5 py-0.5 rounded bg-[var(--bg-subtle)]">v{__APP_VERSION__}</span>
          </div>

          <div class="flex flex-col gap-6 max-w-[560px]">
            <!-- Name -->
            <label class="block">
              <span class="block text-micro font-semibold uppercase tracking-widest text-[var(--text)] mb-1.5">Instance name</span>
              <input
                bind:value={fName}
                onblur={commitName}
                placeholder={host}
                maxlength="60"
                class="w-full px-3 py-2 text-body rounded-md border border-[var(--border)] bg-[var(--bg)] text-[var(--text)]
                       outline-none focus-visible:ring-2 focus-visible:ring-[var(--accent)]"
              />
              <span class="block text-caption text-[var(--text)] mt-1.5">Shown on the sign-in screen. Leave blank to use the host.</span>
            </label>

            <!-- Signups: a real status, so each state carries its own color
                 (green = open/permissive, amber = gated) + an icon. -->
            <div>
              <span class="block text-micro font-semibold uppercase tracking-widest text-[var(--text)] mb-1.5">Sign-ups</span>
              <div class="inline-flex gap-1 p-1 rounded-xl bg-[var(--bg)] shadow-[inset_0_1px_2px_rgba(0,0,0,0.10)]">
                <button
                  type="button"
                  aria-pressed={fSignups}
                  class="flex items-center gap-2 px-4 py-2 rounded-lg text-body-sm font-semibold transition-all
                         motion-safe:active:scale-[0.98]
                         {fSignups
                    ? 'bg-[var(--success-bg)] text-[var(--success)] shadow-[0_1px_2px_rgba(0,0,0,0.10)] ring-1 ring-[color-mix(in_oklab,var(--success)_38%,transparent)]'
                    : 'text-[var(--text-muted)] hover:text-[var(--text)]'}"
                  onclick={() => setSignups(true)}
                >
                  <DoorOpen size={16} class="shrink-0" />
                  Open
                </button>
                <button
                  type="button"
                  aria-pressed={!fSignups}
                  class="flex items-center gap-2 px-4 py-2 rounded-lg text-body-sm font-semibold transition-all
                         motion-safe:active:scale-[0.98]
                         {!fSignups
                    ? 'bg-[color-mix(in_oklab,var(--warn)_15%,var(--bg))] text-[var(--warn-text)] shadow-[0_1px_2px_rgba(0,0,0,0.10)] ring-1 ring-[color-mix(in_oklab,var(--warn)_38%,transparent)]'
                    : 'text-[var(--text-muted)] hover:text-[var(--text)]'}"
                  onclick={() => setSignups(false)}
                >
                  <DoorClosed size={16} class="shrink-0" />
                  Closed
                </button>
              </div>
              <span class="block text-caption text-[var(--text)] mt-2 leading-relaxed">
                {#if fSignups}
                  Anyone can create their own account{parseDomains(fDomains).length ? " from an allowed domain" : ""}.
                {:else}
                  New accounts are created by an admin only. The sign-in screen shows a closed notice.
                {/if}
              </span>
            </div>

            <!-- Email domain allowlist -->
            <label class="block">
              <span class="block text-micro font-semibold uppercase tracking-widest text-[var(--text)] mb-1.5">Allowed signup domains</span>
              <input
                bind:value={fDomains}
                onblur={commitDomains}
                placeholder="snake.com, sub.snake.com"
                class="w-full px-3 py-2 text-body font-mono rounded-md border border-[var(--border)] bg-[var(--bg)] text-[var(--text)]
                       outline-none focus-visible:ring-2 focus-visible:ring-[var(--accent)]"
              />
              <span class="block text-caption text-[var(--text)] mt-1.5">Comma-separated. Leave blank to allow any email domain.</span>
            </label>

            <!-- Session lifetime -->
            <label class="block">
              <span class="block text-micro font-semibold uppercase tracking-widest text-[var(--text)] mb-1.5">Session lifetime</span>
              <div class="flex items-center gap-2">
                <input
                  type="number"
                  bind:value={fSession}
                  onblur={commitSession}
                  min="1"
                  max="365"
                  class="w-24 px-3 py-2 text-body rounded-md border border-[var(--border)] bg-[var(--bg)] text-[var(--text)]
                         outline-none focus-visible:ring-2 focus-visible:ring-[var(--accent)]"
                />
                <span class="text-body text-[var(--text)]">days</span>
              </div>
              <span class="block text-caption text-[var(--text)] mt-1.5">How long a sign-in stays valid before re-authenticating (1 to 365).</span>
            </label>

            <!-- Login message -->
            <label class="block">
              <span class="block text-micro font-semibold uppercase tracking-widest text-[var(--text)] mb-1.5">Login message</span>
              <textarea
                bind:value={fMessage}
                onblur={commitMessage}
                rows="2"
                maxlength="280"
                placeholder="Lific Issue tracker. Ask I.T. for access"
                class="w-full px-3 py-2 text-body rounded-md border border-[var(--border)] bg-[var(--bg)] text-[var(--text)]
                       outline-none focus-visible:ring-2 focus-visible:ring-[var(--accent)] resize-none"
              ></textarea>
              <span class="block text-caption text-[var(--text)] mt-1.5">A short note shown on the sign-in screen. Leave blank for none.</span>
            </label>

            <!-- Single-user mode (LIF-215): auto-sign-in the web UI as the
                 admin. A real auth bypass, so it's set apart with a divider and
                 a stronger (--text) section label, carries a loud warning when
                 on, and is scoped to the browser (REST/MCP still need tokens).
                 The danger is signalled by the divider + amber toggle/warning
                 box, NOT by tinting this 11px label amber — orange-600 is only
                 ~3.4:1 on the light surface and would fail AA. -->
            <div class="pt-6 mt-1 border-t border-[var(--border)]">
              <span class="block text-micro font-semibold uppercase tracking-widest text-[var(--text)] mb-1.5">Single-user mode</span>
              <div class="inline-flex gap-1 p-1 rounded-xl bg-[var(--bg)] shadow-[inset_0_1px_2px_rgba(0,0,0,0.10)]">
                <button
                  type="button"
                  aria-pressed={!fAutoLogin}
                  class="flex items-center gap-2 px-4 py-2 rounded-lg text-body-sm font-semibold transition-all
                         motion-safe:active:scale-[0.98]
                         {!fAutoLogin
                    ? 'bg-[var(--surface)] text-[var(--text)] shadow-[0_1px_2px_rgba(0,0,0,0.10)] ring-1 ring-[var(--border)]'
                    : 'text-[var(--text-muted)] hover:text-[var(--text)]'}"
                  onclick={() => setAutoLogin(false)}
                >
                  <DoorClosed size={16} class="shrink-0" />
                  Require sign-in
                </button>
                <button
                  type="button"
                  aria-pressed={fAutoLogin}
                  class="flex items-center gap-2 px-4 py-2 rounded-lg text-body-sm font-semibold transition-all
                         motion-safe:active:scale-[0.98]
                         {fAutoLogin
                    ? 'bg-[color-mix(in_oklab,var(--warn)_15%,var(--bg))] text-[var(--warn-text)] shadow-[0_1px_2px_rgba(0,0,0,0.10)] ring-1 ring-[color-mix(in_oklab,var(--warn)_38%,transparent)]'
                    : 'text-[var(--text-muted)] hover:text-[var(--text)]'}"
                  onclick={() => setAutoLogin(true)}
                >
                  <DoorOpen size={16} class="shrink-0" />
                  Skip web sign-in
                </button>
              </div>
              <span class="block text-caption text-[var(--text)] mt-2 leading-relaxed">
                {#if fAutoLogin}
                  The web UI signs in as the admin automatically — no login screen.
                {:else}
                  Everyone signs in with their account as normal.
                {/if}
              </span>
              {#if fAutoLogin}
                <div class="flex items-start gap-2 text-caption text-[var(--warn-text)] bg-[color-mix(in_oklab,var(--warn)_12%,var(--bg))] px-3 py-2 rounded-lg mt-2 max-w-[42ch]">
                  <AlertTriangle size={13} class="shrink-0 mt-0.5" />
                  <span>Anyone who can reach this site becomes admin without a password. Only enable on a private or local instance. REST and MCP are unaffected.</span>
                </div>
              {/if}
            </div>

            <!-- Project-scoped permissions (LIF-194/LIF-197): default-deny
                 membership enforcement. Same divider + amber-when-on
                 treatment as single-user mode above, since flipping this ON
                 can just as suddenly lock people out of a project. -->
            <div class="pt-6 mt-1 border-t border-[var(--border)]">
              <span class="flex items-center gap-1.5 text-micro font-semibold uppercase tracking-widest text-[var(--text)] mb-1.5">
                <Users size={12} />
                Project permissions
              </span>
              <div class="inline-flex gap-1 p-1 rounded-xl bg-[var(--bg)] shadow-[inset_0_1px_2px_rgba(0,0,0,0.10)]">
                <button
                  type="button"
                  aria-pressed={!fAuthzEnforced}
                  class="flex items-center gap-2 px-4 py-2 rounded-lg text-body-sm font-semibold transition-all
                         motion-safe:active:scale-[0.98]
                         {!fAuthzEnforced
                    ? 'bg-[var(--surface)] text-[var(--text)] shadow-[0_1px_2px_rgba(0,0,0,0.10)] ring-1 ring-[var(--border)]'
                    : 'text-[var(--text-muted)] hover:text-[var(--text)]'}"
                  onclick={() => setAuthzEnforced(false)}
                >
                  <DoorOpen size={16} class="shrink-0" />
                  Off
                </button>
                <button
                  type="button"
                  aria-pressed={fAuthzEnforced}
                  class="flex items-center gap-2 px-4 py-2 rounded-lg text-body-sm font-semibold transition-all
                         motion-safe:active:scale-[0.98]
                         {fAuthzEnforced
                    ? 'bg-[color-mix(in_oklab,var(--warn)_15%,var(--bg))] text-[var(--warn-text)] shadow-[0_1px_2px_rgba(0,0,0,0.10)] ring-1 ring-[color-mix(in_oklab,var(--warn)_38%,transparent)]'
                    : 'text-[var(--text-muted)] hover:text-[var(--text)]'}"
                  onclick={() => setAuthzEnforced(true)}
                >
                  <DoorClosed size={16} class="shrink-0" />
                  Enforced
                </button>
              </div>
              <span class="block text-caption text-[var(--text)] mt-2 leading-relaxed max-w-[42ch]">
                When on, only project members can see or edit a project. Add yourself as lead to your projects before enabling.
              </span>
              {#if fAuthzEnforced}
                <div class="flex items-start gap-2 text-caption text-[var(--warn-text)] bg-[color-mix(in_oklab,var(--warn)_12%,var(--bg))] px-3 py-2 rounded-lg mt-2 max-w-[42ch]">
                  <AlertTriangle size={13} class="shrink-0 mt-0.5" />
                  <span>Anyone not added as a project member (via that project's Settings → Members) loses access to it immediately, including you if you aren't a lead yet.</span>
                </div>
              {/if}
            </div>
          </div>

          {#if saveError}
            <p class="text-caption text-[var(--error)] mt-4 flex items-center gap-1"><AlertTriangle size={12} /> {saveError}</p>
          {/if}

          <!-- Autosave status (no Save button — each field commits on change). -->
          <div class="flex items-center gap-2 mt-5 h-5 text-body-sm" aria-live="polite">
            {#if saving}
              <span class="inline-flex items-center gap-1.5 text-[var(--text-muted)]">
                <span class="size-3 rounded-full border-2 border-[var(--border)] border-t-[var(--accent)] animate-spin"></span>
                Saving…
              </span>
            {:else if savedAt}
              <span class="inline-flex items-center gap-1 text-[var(--success)]"><Check size={13} /> Saved</span>
            {:else if !saveError}
              <span class="text-[var(--text-muted)]">Changes save automatically.</span>
            {/if}
          </div>
        </section>

        <!-- ── MEMBERS ────────────────────────────────────── -->
        <section class="mt-10 animate-reveal delay-250">
          <div class="flex items-center gap-2 mb-1">
            <ShieldCheck size={16} class="text-[var(--text-muted)]" />
            <h2 class="text-[1rem] font-semibold text-[var(--text)]">Members</h2>
          </div>
          <p class="text-body text-[var(--text-muted)] mb-5 leading-relaxed">
            {users.length} {users.length === 1 ? "person" : "people"} on this instance · {adminCount} admin.
          </p>

          <div class="rounded-xl bg-[var(--surface)] shadow-[0_1px_2px_rgba(0,0,0,0.06)] overflow-hidden">
            <!-- Create a member: username + password is all the server needs. -->
            <div class="flex items-center gap-2 px-4 py-3 border-b border-[var(--border)] flex-wrap">
              <input
                bind:value={newUsername}
                placeholder="username"
                autocomplete="off"
                class="flex-1 min-w-[150px] px-3 py-1.5 text-body-sm font-mono rounded-md border border-[var(--border)]
                       bg-[var(--bg)] text-[var(--text)] outline-none focus-visible:ring-2 focus-visible:ring-[var(--accent)]"
              />
              <input
                bind:value={newPassword}
                type="password"
                placeholder="password"
                autocomplete="new-password"
                class="flex-1 min-w-[150px] px-3 py-1.5 text-body-sm rounded-md border border-[var(--border)]
                       bg-[var(--bg)] text-[var(--text)] outline-none focus-visible:ring-2 focus-visible:ring-[var(--accent)]"
              />
              <button
                class="flex items-center gap-1.5 text-body-sm font-medium text-[var(--btn-success-text)]
                       bg-[var(--btn-success)] px-3 py-1.5 rounded-md hover:bg-[var(--btn-success-hover)]
                       transition-colors disabled:opacity-40 disabled:cursor-not-allowed shrink-0"
                disabled={creating || !newUsername.trim() || !newPassword}
                onclick={submitCreate}
              >
                <UserPlus size={14} />
                {creating ? "Creating…" : "Create"}
              </button>
            </div>
            {#if createError}
              <div class="px-4 py-2 text-caption text-[var(--error)] bg-[var(--error-bg)]">{createError}</div>
            {:else if createdName}
              <div class="px-4 py-2 text-caption text-[var(--success)]">
                Created <span class="font-mono">@{createdName}</span>. Share the password with them; they can change it in their settings.
              </div>
            {/if}

            {#each users as u, i (u.id)}
              <div class="flex items-center gap-3 px-4 py-3 {i > 0 ? 'border-t border-[var(--border)]' : ''} {u.is_active ? '' : 'opacity-60'}">
                <div class="size-8 shrink-0 rounded-full bg-[var(--accent)] text-[var(--accent-text)] grid place-items-center text-micro font-semibold tracking-wide">
                  {initials(u.display_name || u.username)}
                </div>
                <div class="flex-1 min-w-0">
                  <div class="text-body text-[var(--text)] truncate leading-tight">
                    {u.display_name || u.username}
                    {#if u.id === user?.id}
                      <span class="text-caption text-[var(--text-faint)]">(you)</span>
                    {/if}
                  </div>
                  <div class="text-caption font-mono text-[var(--text-faint)] truncate leading-tight mt-0.5">@{u.username}</div>
                </div>
                {#if !u.is_active}
                  <span class="text-micro font-semibold uppercase tracking-wide px-1.5 py-0.5 rounded-full shrink-0 text-[var(--warn-text)] bg-[color-mix(in_oklab,var(--warn)_15%,var(--bg))]">
                    Deactivated
                  </span>
                {/if}
                <span
                  class="text-micro font-semibold uppercase tracking-wide px-1.5 py-0.5 rounded-full shrink-0
                         {u.is_admin
                    ? 'text-[var(--accent)] bg-[var(--accent-subtle)]'
                    : 'text-[var(--text-muted)] bg-[var(--bg-subtle)]'}"
                >
                  {u.is_admin ? "Admin" : "Member"}
                </span>
                <span class="hidden sm:block text-caption text-[var(--text-faint)] tabular-nums shrink-0 w-[5.5rem] text-right">
                  <TimeAgo date={u.created_at} />
                </span>

                <!-- Actions. Never on your own row: demoting or deactivating
                     yourself from the page you are standing on is a footgun,
                     and another admin (or the CLI) can always do it. -->
                {#if u.id !== user?.id}
                  {#if pending?.id === u.id}
                    <div class="flex items-center gap-1.5 shrink-0">
                      <button
                        class="text-caption font-medium text-[var(--error-text)] bg-[var(--error)] px-2 py-1 rounded-md
                               hover:opacity-90 transition-opacity disabled:opacity-40"
                        disabled={rowBusy === u.id}
                        onclick={() =>
                          runAction(u, pending?.kind === "demote" ? demoteUser : deactivateUser)}
                      >
                        {#if rowBusy === u.id}
                          …
                        {:else}
                          {pending.kind === "demote" ? "Demote" : "Deactivate"}
                        {/if}
                      </button>
                      <button
                        class="text-caption text-[var(--text-muted)] px-2 py-1 rounded-md hover:bg-[var(--bg-subtle)] transition-colors"
                        onclick={() => { pending = null; }}
                      >
                        Cancel
                      </button>
                    </div>
                  {:else}
                    <div class="flex items-center gap-1 shrink-0">
                      {#if u.is_admin}
                        <button
                          class="size-7 grid place-items-center rounded-md text-[var(--text-muted)]
                                 hover:text-[var(--warn-text)] hover:bg-[var(--bg-subtle)] transition-colors"
                          onclick={() => { pending = { id: u.id, kind: "demote" }; rowError = null; }}
                          title="Remove instance admin"
                          aria-label="Remove instance admin from {u.display_name || u.username}"
                        >
                          <ShieldMinus size={14} />
                        </button>
                      {:else}
                        <button
                          class="size-7 grid place-items-center rounded-md text-[var(--text-muted)]
                                 hover:text-[var(--accent)] hover:bg-[var(--accent-subtle)] transition-colors
                                 disabled:opacity-40 disabled:cursor-not-allowed"
                          disabled={rowBusy === u.id}
                          onclick={() => runAction(u, promoteUser)}
                          title="Make instance admin"
                          aria-label="Make {u.display_name || u.username} an instance admin"
                        >
                          <ShieldPlus size={14} />
                        </button>
                      {/if}
                      {#if u.is_active}
                        <button
                          class="size-7 grid place-items-center rounded-md text-[var(--text-muted)]
                                 hover:text-[var(--error)] hover:bg-[var(--error-bg)] transition-colors"
                          onclick={() => { pending = { id: u.id, kind: "deactivate" }; rowError = null; }}
                          title="Deactivate account"
                          aria-label="Deactivate {u.display_name || u.username}"
                        >
                          <UserMinus size={14} />
                        </button>
                      {:else}
                        <button
                          class="size-7 grid place-items-center rounded-md text-[var(--text-muted)]
                                 hover:text-[var(--success)] hover:bg-[var(--success-bg)] transition-colors
                                 disabled:opacity-40 disabled:cursor-not-allowed"
                          disabled={rowBusy === u.id}
                          onclick={() => runAction(u, reactivateUser)}
                          title="Restore account"
                          aria-label="Restore {u.display_name || u.username}"
                        >
                          <RotateCcw size={14} />
                        </button>
                      {/if}
                    </div>
                  {/if}
                {/if}
              </div>
              {#if rowError?.id === u.id}
                <div class="px-4 pb-2.5 -mt-1 text-caption text-[var(--error)]">{rowError.message}</div>
              {/if}
            {/each}
          </div>
          <p class="text-caption text-[var(--text-faint)] mt-2 max-w-[60ch] leading-relaxed">
            Deactivating an account ends its sessions and revokes its API keys and tokens. Nothing it wrote is
            removed. The last admin who can still sign in cannot be demoted or deactivated.
          </p>
        </section>
      {/if}
    {/if}
  </div>
</div>

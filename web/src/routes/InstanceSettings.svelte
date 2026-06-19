<script lang="ts">
  // Instance settings (admin-only): edit the DB-backed, runtime instance
  // settings (LIF-210/211/212/213) and view the member roster. Non-admins who
  // reach the URL directly get a friendly gate.
  import {
    me,
    listUsers,
    getInstanceSettings,
    updateInstanceSettings,
    type AuthUser,
    type UserSummary,
    type InstanceSettings,
    type InstanceSettingsPatch,
  } from "../lib/api";
  import SettingsTabs from "../lib/SettingsTabs.svelte";
  import { formatRelative } from "../lib/format";
  import { ShieldCheck, Lock, SlidersHorizontal, Check, AlertTriangle, DoorOpen, DoorClosed } from "lucide-svelte";
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

  function initials(name: string): string {
    return name.split(/[\s_-]+/).slice(0, 2).map((w) => w[0]?.toUpperCase() ?? "").join("");
  }

  const adminCount = $derived(users.filter((u) => u.is_admin).length);
</script>

{#snippet topbarContent()}
  <div class="flex items-center gap-3 px-6 py-2 w-full">
    <span class="text-body-sm font-medium text-[var(--text)]">Settings</span>
  </div>
{/snippet}

<div class="flex-1 overflow-y-auto">
  <div class="w-full max-w-[1000px] mx-auto px-6 py-10 md:py-12">
    {#if loading}
      <div class="flex items-center justify-center py-20">
        <div class="size-6 rounded-full border-2 border-[var(--border)] border-t-[var(--accent)] animate-spin"></div>
      </div>
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
          <h1 class="font-display text-[1.5rem] tracking-tight text-[var(--text)] leading-none">Instance</h1>
          <p class="text-body text-[var(--text-muted)] mt-2">
            Settings for the Lific instance at <span class="font-mono text-[var(--text)]">{host}</span>.
            Changes apply immediately.
          </p>
        </section>

        <!-- ── SETTINGS FORM ──────────────────────────────── -->
        <section class="rounded-xl bg-[var(--surface)] shadow-[0_1px_2px_rgba(0,0,0,0.06)] p-5 animate-reveal delay-250">
          <div class="flex items-center gap-2 mb-5">
            <SlidersHorizontal size={15} class="text-[var(--text-muted)]" />
            <h2 class="text-[0.9375rem] font-semibold text-[var(--text)]">Settings</h2>
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
            {#each users as u, i (u.id)}
              <div class="flex items-center gap-3 px-4 py-3 {i > 0 ? 'border-t border-[var(--border)]' : ''}">
                <div class="size-8 shrink-0 rounded-full bg-[var(--accent)] text-[var(--accent-text)] grid place-items-center text-micro font-semibold tracking-wide">
                  {initials(u.display_name || u.username)}
                </div>
                <div class="flex-1 min-w-0">
                  <div class="text-body text-[var(--text)] truncate leading-tight">{u.display_name || u.username}</div>
                  <div class="text-caption font-mono text-[var(--text-faint)] truncate leading-tight mt-0.5">@{u.username}</div>
                </div>
                <span
                  class="text-micro font-semibold uppercase tracking-wide px-1.5 py-0.5 rounded-full shrink-0
                         {u.is_admin
                    ? 'text-[var(--accent)] bg-[var(--accent-subtle)]'
                    : 'text-[var(--text-muted)] bg-[var(--bg-subtle)]'}"
                >
                  {u.is_admin ? "Admin" : "Member"}
                </span>
                <span class="hidden sm:block text-caption text-[var(--text-faint)] tabular-nums shrink-0 w-[5.5rem] text-right">
                  {formatRelative(u.created_at)}
                </span>
              </div>
            {/each}
          </div>
        </section>
      {/if}
    {/if}
  </div>
</div>

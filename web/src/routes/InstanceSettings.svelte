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

  const dirty = $derived.by(() => {
    if (!settings) return false;
    return (
      fName.trim() !== (settings.instance_name ?? "") ||
      fSignups !== settings.allow_signup ||
      parseDomains(fDomains).join(",") !== settings.signup_email_domains.join(",") ||
      fSession !== settings.session_lifetime_days ||
      fMessage.trim() !== (settings.login_message ?? "")
    );
  });

  async function save() {
    if (saving || !dirty) return;
    saving = true;
    saveError = "";
    const patch: InstanceSettingsPatch = {
      instance_name: fName.trim(),
      allow_signup: fSignups,
      signup_email_domains: parseDomains(fDomains),
      session_lifetime_days: fSession,
      login_message: fMessage.trim(),
    };
    const res = await updateInstanceSettings(patch);
    saving = false;
    if (res.ok) {
      hydrate(res.data);
      savedAt = Date.now();
      window.setTimeout(() => { if (Date.now() - savedAt >= 1900) savedAt = 0; }, 2000);
    } else {
      saveError = res.error;
    }
  }

  function initials(name: string): string {
    return name.split(/[\s_-]+/).slice(0, 2).map((w) => w[0]?.toUpperCase() ?? "").join("");
  }

  const adminCount = $derived(users.filter((u) => u.is_admin).length);
</script>

{#snippet topbarContent()}
  <div class="flex items-center gap-3 px-6 py-2 w-full">
    <span class="text-[0.8125rem] font-medium text-[var(--text)]">Settings</span>
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
          <p class="text-[0.875rem] text-[var(--text-muted)] mt-1 max-w-[36ch]">
            Instance settings are visible to administrators of this instance.
          </p>
          <button
            class="mt-5 text-[0.8125rem] font-medium text-[var(--btn-success-text)] bg-[var(--btn-success)]
                   px-3 py-1.5 rounded-md hover:bg-[var(--btn-success-hover)] transition-colors"
            onclick={() => navigate("/settings")}
          >
            Back to account
          </button>
        </div>
      {:else}
        <section class="mb-8 animate-reveal delay-100">
          <h1 class="font-display text-[1.5rem] tracking-tight text-[var(--text)] leading-none">Instance</h1>
          <p class="text-[0.875rem] text-[var(--text-muted)] mt-2">
            Settings for the Lific instance at <span class="font-mono text-[var(--text)]">{host}</span>.
            Changes apply immediately.
          </p>
        </section>

        <!-- ── SETTINGS FORM ──────────────────────────────── -->
        <section class="rounded-xl bg-[var(--surface)] shadow-[0_1px_2px_rgba(0,0,0,0.06)] p-5 animate-reveal delay-250">
          <div class="flex items-center gap-2 mb-5">
            <SlidersHorizontal size={15} class="text-[var(--text-muted)]" />
            <h2 class="text-[0.9375rem] font-semibold text-[var(--text)]">Settings</h2>
            <span class="font-mono text-[0.625rem] text-[var(--text-faint)] px-1.5 py-0.5 rounded bg-[var(--bg-subtle)]">v{__APP_VERSION__}</span>
          </div>

          <div class="flex flex-col gap-6 max-w-[560px]">
            <!-- Name -->
            <label class="block">
              <span class="block text-[0.6875rem] font-semibold uppercase tracking-widest text-[var(--text-faint)] mb-1.5">Instance name</span>
              <input
                bind:value={fName}
                placeholder={host}
                maxlength="60"
                class="w-full px-3 py-2 text-[0.875rem] rounded-md border border-[var(--border)] bg-[var(--bg)] text-[var(--text)]
                       outline-none focus-visible:ring-2 focus-visible:ring-[var(--accent)]"
              />
              <span class="block text-[0.75rem] text-[var(--text-muted)] mt-1.5">Shown on the sign-in screen. Leave blank to use the host.</span>
            </label>

            <!-- Signups: a real status, so each state carries its own color
                 (green = open/permissive, amber = gated) + an icon. -->
            <div>
              <span class="block text-[0.6875rem] font-semibold uppercase tracking-widest text-[var(--text-faint)] mb-1.5">Sign-ups</span>
              <div class="inline-flex gap-1 p-1 rounded-xl bg-[var(--bg)] shadow-[inset_0_1px_2px_rgba(0,0,0,0.10)]">
                <button
                  type="button"
                  aria-pressed={fSignups}
                  class="flex items-center gap-2 px-4 py-2 rounded-lg text-[0.8125rem] font-semibold transition-all
                         motion-safe:active:scale-[0.98]
                         {fSignups
                    ? 'bg-[var(--success-bg)] text-[var(--success)] shadow-[0_1px_2px_rgba(0,0,0,0.10)] ring-1 ring-[color-mix(in_oklab,var(--success)_38%,transparent)]'
                    : 'text-[var(--text-faint)] hover:text-[var(--text-muted)]'}"
                  onclick={() => (fSignups = true)}
                >
                  <DoorOpen size={16} class="shrink-0" />
                  Open
                </button>
                <button
                  type="button"
                  aria-pressed={!fSignups}
                  class="flex items-center gap-2 px-4 py-2 rounded-lg text-[0.8125rem] font-semibold transition-all
                         motion-safe:active:scale-[0.98]
                         {!fSignups
                    ? 'bg-[color-mix(in_oklab,var(--warn)_15%,var(--bg))] text-[var(--warn)] shadow-[0_1px_2px_rgba(0,0,0,0.10)] ring-1 ring-[color-mix(in_oklab,var(--warn)_38%,transparent)]'
                    : 'text-[var(--text-faint)] hover:text-[var(--text-muted)]'}"
                  onclick={() => (fSignups = false)}
                >
                  <DoorClosed size={16} class="shrink-0" />
                  Closed
                </button>
              </div>
              <span class="block text-[0.75rem] text-[var(--text-muted)] mt-2 leading-relaxed">
                {#if fSignups}
                  Anyone can create their own account{parseDomains(fDomains).length ? " from an allowed domain" : ""}.
                {:else}
                  New accounts are created by an admin only. The sign-in screen shows a closed notice.
                {/if}
              </span>
            </div>

            <!-- Email domain allowlist -->
            <label class="block">
              <span class="block text-[0.6875rem] font-semibold uppercase tracking-widest text-[var(--text-faint)] mb-1.5">Allowed signup domains</span>
              <input
                bind:value={fDomains}
                placeholder="acme.com, sub.acme.com"
                class="w-full px-3 py-2 text-[0.875rem] font-mono rounded-md border border-[var(--border)] bg-[var(--bg)] text-[var(--text)]
                       outline-none focus-visible:ring-2 focus-visible:ring-[var(--accent)]"
              />
              <span class="block text-[0.75rem] text-[var(--text-muted)] mt-1.5">Comma-separated. Leave blank to allow any email domain.</span>
            </label>

            <!-- Session lifetime -->
            <label class="block">
              <span class="block text-[0.6875rem] font-semibold uppercase tracking-widest text-[var(--text-faint)] mb-1.5">Session lifetime</span>
              <div class="flex items-center gap-2">
                <input
                  type="number"
                  bind:value={fSession}
                  min="1"
                  max="365"
                  class="w-24 px-3 py-2 text-[0.875rem] rounded-md border border-[var(--border)] bg-[var(--bg)] text-[var(--text)]
                         outline-none focus-visible:ring-2 focus-visible:ring-[var(--accent)]"
                />
                <span class="text-[0.875rem] text-[var(--text-muted)]">days</span>
              </div>
              <span class="block text-[0.75rem] text-[var(--text-muted)] mt-1.5">How long a sign-in stays valid before re-authenticating (1 to 365).</span>
            </label>

            <!-- Login message -->
            <label class="block">
              <span class="block text-[0.6875rem] font-semibold uppercase tracking-widest text-[var(--text-faint)] mb-1.5">Login message</span>
              <textarea
                bind:value={fMessage}
                rows="2"
                maxlength="280"
                placeholder="e.g. Acme's tracker. Ask #it for access."
                class="w-full px-3 py-2 text-[0.875rem] rounded-md border border-[var(--border)] bg-[var(--bg)] text-[var(--text)]
                       outline-none focus-visible:ring-2 focus-visible:ring-[var(--accent)] resize-none"
              ></textarea>
              <span class="block text-[0.75rem] text-[var(--text-muted)] mt-1.5">A short note shown on the sign-in screen. Leave blank for none.</span>
            </label>
          </div>

          {#if saveError}
            <p class="text-[0.75rem] text-[var(--error)] mt-4 flex items-center gap-1"><AlertTriangle size={12} /> {saveError}</p>
          {/if}

          <div class="flex items-center gap-3 mt-5">
            <button
              class="text-[0.8125rem] font-medium text-[var(--btn-success-text)] bg-[var(--btn-success)] px-3 py-1.5 rounded-md
                     hover:bg-[var(--btn-success-hover)] transition-colors disabled:opacity-40 disabled:cursor-not-allowed"
              disabled={!dirty || saving}
              onclick={save}
            >
              {saving ? "Saving…" : "Save changes"}
            </button>
            {#if savedAt}
              <span class="inline-flex items-center gap-1 text-[0.8125rem] text-[var(--success)]" aria-live="polite"><Check size={13} /> Saved</span>
            {/if}
          </div>
        </section>

        <!-- ── MEMBERS ────────────────────────────────────── -->
        <section class="mt-10 animate-reveal delay-250">
          <div class="flex items-center gap-2 mb-1">
            <ShieldCheck size={16} class="text-[var(--text-muted)]" />
            <h2 class="text-[1rem] font-semibold text-[var(--text)]">Members</h2>
          </div>
          <p class="text-[0.875rem] text-[var(--text-muted)] mb-5 leading-relaxed">
            {users.length} {users.length === 1 ? "person" : "people"} on this instance · {adminCount} admin.
          </p>

          <div class="rounded-xl bg-[var(--surface)] shadow-[0_1px_2px_rgba(0,0,0,0.06)] overflow-hidden">
            {#each users as u, i (u.id)}
              <div class="flex items-center gap-3 px-4 py-3 {i > 0 ? 'border-t border-[var(--border)]' : ''}">
                <div class="size-8 shrink-0 rounded-full bg-[var(--accent)] text-[var(--accent-text)] grid place-items-center text-[0.625rem] font-semibold tracking-wide">
                  {initials(u.display_name || u.username)}
                </div>
                <div class="flex-1 min-w-0">
                  <div class="text-[0.875rem] text-[var(--text)] truncate leading-tight">{u.display_name || u.username}</div>
                  <div class="text-[0.75rem] font-mono text-[var(--text-faint)] truncate leading-tight mt-0.5">@{u.username}</div>
                </div>
                <span
                  class="text-[0.625rem] font-semibold uppercase tracking-wide px-1.5 py-0.5 rounded-full shrink-0
                         {u.is_admin
                    ? 'text-[var(--accent)] bg-[var(--accent-subtle)]'
                    : 'text-[var(--text-muted)] bg-[var(--bg-subtle)]'}"
                >
                  {u.is_admin ? "Admin" : "Member"}
                </span>
                <span class="hidden sm:block text-[0.75rem] text-[var(--text-faint)] tabular-nums shrink-0 w-[5.5rem] text-right">
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

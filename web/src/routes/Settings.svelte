<script lang="ts">
  import {
    me,
    listBots,
    updateProfile,
    changePassword,
    revokeAllSessions,
    logout,
    sessionKey,
    getInstance,
    type Bot,
  } from "../lib/api";
  import SettingsTabs from "../lib/SettingsTabs.svelte";
  import Skeleton from "../lib/Skeleton.svelte";
  import AgentAccounts from "../lib/settings/AgentAccounts.svelte";
  import {
    themePreference, setPreference, type ThemePreference,
    getAccent, setAccent, type AccentPreset,
    getDensity, setDensity, type Density,
    getFontScale, setFontScale, type FontScale,
    getMotionPreference, setMotionPreference, type MotionPreference,
  } from "../lib/theme";
  import { ACCENT_PRESETS } from "../lib/appearance/presets";
  import { Check, AlertTriangle, Sun, Moon, Monitor, LogOut, Rows3, Rows2, Zap, ZapOff } from "lucide-svelte";
  import { getContext } from "svelte";
  import { currentUser, getUserRevision, publishUser } from "../lib/userState";

  let { navigate }: { navigate: (path: string) => void } = $props();

  const topbarCtx = getContext<{
    set: (s: import("svelte").Snippet | undefined) => void;
  } | undefined>("lific:topbar");
  $effect(() => {
    topbarCtx?.set(topbarContent);
    return () => topbarCtx?.set(undefined);
  });

  let user = $derived($currentUser);
  let bots = $state<Bot[]>([]);
  let loading = $state(true);
  let webAutoLogin = $state(false);

  // Profile: a labeled form, saved together, separate from the read-only
  // identity header above it.
  let profileName = $state("");
  let profileEmail = $state("");
  let profileSaving = $state(false);
  let profileError = $state("");
  let profileSavedAt = $state(0);

  // Appearance
  let accentPref = $state<AccentPreset>("indigo");
  let densityPref = $state<Density>("comfortable");
  let fontScalePref = $state<FontScale>("md");
  let motionPref = $state<MotionPreference>("system");

  // Security
  let curPw = $state("");
  let newPw = $state("");
  let pwError = $state("");
  let pwSaving = $state(false);
  let pwSuccess = $state(false);
  let signingOut = $state(false);
  // Two-step inline confirm for signing out everywhere. It revokes every
  // credential the account owns, not just this browser's session.
  let confirmingSignOutAll = $state(false);
  let signOutAllError = $state("");
  let loggingOut = $state(false);
  let logoutError = $state("");

  $effect(() => {
    accentPref = getAccent();
    densityPref = getDensity();
    fontScalePref = getFontScale();
    motionPref = getMotionPreference();
    loadUser();
  });

  let hasProfileChanges = $derived(
    user != null && (
      profileName.trim() !== user.display_name ||
      profileEmail.trim().toLowerCase() !== user.email
    ),
  );

  async function saveProfile() {
    if (!user || !hasProfileChanges || profileSaving) return;
    const revision = getUserRevision();
    const session = sessionKey();
    profileSaving = true;
    profileError = "";
    const input: { display_name?: string; email?: string } = {};
    if (profileName.trim() !== user.display_name) input.display_name = profileName.trim();
    if (profileEmail.trim().toLowerCase() !== user.email) input.email = profileEmail.trim();
    const res = await updateProfile(input);
    profileSaving = false;
    if (session !== sessionKey()) return;
    if (res.ok) {
      if (!publishUser(res.data, revision)) return;
      profileName = res.data.display_name;
      profileEmail = res.data.email;
      profileSavedAt = Date.now();
      window.setTimeout(() => { if (Date.now() - profileSavedAt >= 1900) profileSavedAt = 0; }, 2000);
    } else {
      profileError = res.error;
    }
  }

  function pickTheme(p: ThemePreference) {
    setPreference(p);
  }
  function pickAccent(p: AccentPreset) {
    accentPref = p;
    setAccent(p);
  }
  function pickDensity(p: Density) {
    densityPref = p;
    setDensity(p);
  }
  function pickFontScale(p: FontScale) {
    fontScalePref = p;
    setFontScale(p);
  }
  function pickMotion(p: MotionPreference) {
    motionPref = p;
    setMotionPreference(p);
  }

  async function submitPassword() {
    if (pwSaving) return;
    pwError = ""; pwSuccess = false;
    if (newPw.length < 8) { pwError = "The new password needs at least 8 characters."; return; }
    pwSaving = true;
    // `changePassword` adopts the replacement session the server returns;
    // the old one is dead the moment the request succeeds.
    const res = await changePassword({ current_password: curPw, new_password: newPw });
    pwSaving = false;
    if (res.ok) {
      pwSuccess = true;
      curPw = ""; newPw = "";
      // Agent keys were revoked with everything else, so re-read them rather
      // than leaving the list claiming they still work.
      await loadBots();
      window.setTimeout(() => { pwSuccess = false; }, 6000);
    } else {
      pwError = res.error;
    }
  }

  async function signOutAll() {
    if (signingOut) return;
    signingOut = true;
    signOutAllError = "";
    const res = await revokeAllSessions();
    signingOut = false;
    if (res.ok) {
      confirmingSignOutAll = false;
      currentUser.set(null);
      navigate("/login");
      return;
    }
    // The session is still valid (the helper only clears it on success),
    // so stay put, keep the confirm open, and let them try again.
    signOutAllError = res.error;
  }

  async function logoutNow() {
    if (loggingOut) return;
    loggingOut = true;
    logoutError = "";
    const res = await logout();
    loggingOut = false;
    if (res.ok) {
      currentUser.set(null);
      navigate("/login");
      return;
    }
    // The cookie is still valid (the helper only clears the session once the
    // server confirms), so stay signed in and let them try again.
    logoutError = `Couldn't sign out: ${res.error}`;
  }

  async function loadUser() {
    const revision = getUserRevision();
    const session = sessionKey();
    // Whether this instance signs in without a password decides how a stale
    // session is recovered when an agent key is created.
    const instance = await getInstance();
    if (instance.ok) webAutoLogin = instance.data.web_auto_login;
    const result = await me();
    if (session !== sessionKey()) return;
    if (result.ok) {
      publishUser(result.data, revision);
      profileName = user?.display_name ?? "";
      profileEmail = user?.email ?? "";
      await loadBots();
    }
    loading = false;
  }
  async function loadBots() {
    const result = await listBots();
    if (result.ok) bots = result.data;
  }

  function initials(name: string): string {
    return name.split(/[\s_-]+/).slice(0, 2).map((w) => w[0]?.toUpperCase() ?? "").join("");
  }

  // ── Section index with scroll-spy ────────────────────────
  const sections = [
    { id: "profile", label: "Profile" },
    { id: "appearance", label: "Appearance" },
    { id: "agents", label: "Agents" },
    { id: "security", label: "Security" },
  ];
  let activeSection = $state("profile");
  let scroller = $state<HTMLElement | null>(null);

  $effect(() => {
    if (loading || !scroller) return;
    const root = scroller;
    const update = () => {
      const line = root.getBoundingClientRect().top + 96;
      let current = sections[0].id;
      for (const s of sections) {
        const el = document.getElementById(`settings-${s.id}`);
        if (el && el.getBoundingClientRect().top <= line) current = s.id;
      }
      if (root.scrollTop + root.clientHeight >= root.scrollHeight - 4) current = sections[sections.length - 1].id;
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
    "px-2.5 py-1.5 outline-none transition-colors placeholder:text-[var(--text-faint)] " +
    "focus:border-[var(--accent)] focus-visible:ring-2 focus-visible:ring-[var(--accent-subtle)]";
  const segTrack = "inline-flex flex-wrap p-0.5 rounded-lg bg-[var(--bg)] shadow-[inset_0_1px_2px_rgba(0,0,0,0.10)]";
  const segButton = "flex items-center gap-1.5 px-3 py-1.5 rounded-md text-body-sm font-medium transition-all";
  const segOn = "bg-[var(--surface)] text-[var(--text)] shadow-[0_1px_2px_rgba(0,0,0,0.12)]";
  const segOff = "text-[var(--text-muted)] hover:text-[var(--text)]";
</script>

{#snippet topbarContent()}
  <div class="flex items-center gap-3 px-6 py-2 w-full">
    <span class="text-body-sm font-medium text-[var(--text)]">Settings</span>
  </div>
{/snippet}

{#snippet row(label: string, help: string, control: import("svelte").Snippet)}
  <div class="flex flex-col sm:flex-row sm:items-start gap-2 sm:gap-8 px-4 py-4 border-t border-[var(--border)] first:border-t-0">
    <div class="sm:w-48 shrink-0">
      <p class="text-body-sm font-medium text-[var(--text)]">{label}</p>
      {#if help}<p class="text-caption text-[var(--text-muted)] mt-0.5 leading-relaxed">{help}</p>{/if}
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

<div class="flex-1 h-full overflow-y-auto" bind:this={scroller}>
  <div class="w-full max-w-[1100px] mx-auto px-4 md:px-10 pt-8 md:pt-10 pb-24">
    {#if loading}
      <div class="flex items-center gap-6 border-b border-[var(--border)] mb-8">
        <Skeleton variant="bar" class="h-4 w-16 mb-2.5 mt-1" />
        <Skeleton variant="bar" class="h-4 w-16 mb-2.5 mt-1" />
      </div>
      <section class="flex items-center gap-4 mb-10">
        <Skeleton variant="circle" class="size-14 shrink-0" />
        <div class="min-w-0 flex flex-col gap-2">
          <Skeleton variant="bar" class="h-6 w-48" />
          <Skeleton variant="bar" class="h-3.5 w-64" />
        </div>
      </section>
      <div class="lg:pl-56 flex flex-col gap-3">
        <Skeleton variant="bar" class="h-5 w-28" />
        <Skeleton variant="block" class="h-[220px] w-full max-w-[780px] rounded-xl" />
      </div>
    {:else if user}
      <SettingsTabs active="account" isAdmin={user.is_admin} {navigate} />

      <!-- ── IDENTITY ───────────────────────────────────────── -->
      <header class="flex items-center gap-4 mb-10">
        <div class="size-14 shrink-0 rounded-full bg-[var(--accent)] text-[var(--accent-text)] grid place-items-center font-display text-title tracking-tight">
          {initials(user.display_name || user.username)}
        </div>
        <div class="min-w-0">
          <h1 class="text-[1.6rem] font-semibold tracking-tight text-[var(--text)] leading-tight truncate">
            {user.display_name || user.username}
          </h1>
          <div class="flex items-center gap-x-2 gap-y-1 mt-1 flex-wrap text-body-sm">
            <span class="font-mono text-[var(--text-muted)]">@{user.username}</span>
            <span class="text-[var(--text-faint)]" aria-hidden="true">·</span>
            <span class="text-[var(--text-muted)] break-all">{user.email}</span>
            <span class="text-caption px-2 py-px rounded-full {user.is_admin ? 'text-[var(--accent)] bg-[var(--accent-subtle)]' : 'text-[var(--text-muted)] bg-[var(--bg-subtle)]'}">
              {user.is_admin ? "Admin" : "Member"}
            </span>
          </div>
        </div>
      </header>

      <div class="lg:flex lg:gap-12">
        <nav aria-label="Settings sections" class="lg:w-44 lg:shrink-0">
          <ul class="flex lg:flex-col gap-1 overflow-x-auto lg:overflow-visible pb-3 lg:pb-0 -mx-1 px-1 lg:sticky lg:top-6
                     [scrollbar-width:none] [&::-webkit-scrollbar]:hidden">
            {#each sections as s (s.id)}
              <li class="shrink-0">
                <button
                  class="w-full text-left whitespace-nowrap px-2.5 py-1.5 rounded-md text-body-sm transition-colors
                         {activeSection === s.id
                    ? 'bg-[var(--bg-subtle)] text-[var(--text)] font-medium'
                    : 'text-[var(--text-muted)] hover:text-[var(--text)] hover:bg-[var(--bg-subtle)]'}"
                  aria-current={activeSection === s.id ? "true" : undefined}
                  onclick={() => jump(s.id)}
                >
                  {s.label}
                </button>
              </li>
            {/each}
          </ul>
        </nav>

        <div class="flex-1 min-w-0 max-w-[780px] flex flex-col gap-12 pt-2 lg:pt-0">
          <!-- ── PROFILE ─────────────────────────────────────── -->
          <section id="settings-profile" class="scroll-mt-6" aria-labelledby="settings-profile-heading">
            {@render sectionHead("profile", "Profile")}
            <div class="rounded-xl bg-[var(--surface)] shadow-[0_1px_2px_rgba(0,0,0,0.06)]">
              {#snippet nameControl()}
                <label class="block max-w-[36ch]">
                  <span class="sr-only">Display name</span>
                  <input bind:value={profileName} class={inputClass} autocomplete="name" />
                </label>
              {/snippet}
              {@render row("Display name", "Shown on your issues and comments.", nameControl)}

              {#snippet emailControl()}
                <label class="block max-w-[36ch]">
                  <span class="sr-only">Email</span>
                  <input bind:value={profileEmail} type="email" class={inputClass} autocomplete="email" />
                </label>
              {/snippet}
              {@render row("Email", "You can sign in with it instead of your username.", emailControl)}

              {#snippet usernameControl()}
                <p class="text-body-sm font-mono text-[var(--text)] py-1.5">@{user!.username}</p>
              {/snippet}
              {@render row("Username", "Fixed. Your agent accounts are named after it.", usernameControl)}

              <div class="flex flex-wrap items-center justify-end gap-3 px-4 py-3 border-t border-[var(--border)]">
                {#if profileError}
                  <p class="mr-auto text-caption text-[var(--error)] flex items-center gap-1" role="alert"><AlertTriangle size={12} /> {profileError}</p>
                {/if}
                {#if profileSavedAt}
                  <span class="inline-flex items-center gap-1 text-body-sm text-[var(--success)]" aria-live="polite"><Check size={13} /> Saved</span>
                {/if}
                <button
                  class="text-body-sm font-medium text-[var(--accent-text)] bg-[var(--accent)] px-3 py-1.5 rounded-md
                         hover:bg-[var(--accent-hover)] transition-colors disabled:opacity-40 disabled:cursor-not-allowed"
                  disabled={!hasProfileChanges || profileSaving}
                  onclick={saveProfile}
                >
                  {profileSaving ? "Saving…" : "Save changes"}
                </button>
              </div>
            </div>
          </section>

          <!-- ── APPEARANCE ──────────────────────────────────── -->
          <section id="settings-appearance" class="scroll-mt-6" aria-labelledby="settings-appearance-heading">
            {@render sectionHead("appearance", "Appearance", "Kept in this browser.")}
            <div class="rounded-xl bg-[var(--surface)] shadow-[0_1px_2px_rgba(0,0,0,0.06)]">
              {#snippet themeControl()}
                <div class={segTrack}>
                  {#each [["light", "Light", Sun], ["dark", "Dark", Moon], ["system", "System", Monitor]] as [val, label, Icon]}
                    {@const IconComp = Icon as typeof Sun}
                    <button class="{segButton} {$themePreference === val ? segOn : segOff}" aria-pressed={$themePreference === val} onclick={() => pickTheme(val as ThemePreference)}>
                      <IconComp size={14} />
                      {label}
                    </button>
                  {/each}
                </div>
              {/snippet}
              {@render row("Theme", "System follows your OS.", themeControl)}

              {#snippet accentControl()}
                <div class="flex items-center gap-2.5 flex-wrap py-0.5">
                  {#each ACCENT_PRESETS as preset (preset.id)}
                    <button
                      type="button"
                      class="size-7 shrink-0 rounded-full transition-transform motion-safe:active:scale-90
                             {accentPref === preset.id ? 'ring-2 ring-offset-2 ring-offset-[var(--surface)] ring-[var(--text)]' : 'hover:scale-110'}"
                      style:background-color={preset.swatch}
                      onclick={() => pickAccent(preset.id)}
                      title={preset.label}
                      aria-label="Accent: {preset.label}"
                      aria-pressed={accentPref === preset.id}
                    >
                      {#if accentPref === preset.id}
                        <Check size={13} class="mx-auto text-white drop-shadow-[0_1px_1px_rgba(0,0,0,0.4)]" />
                      {/if}
                    </button>
                  {/each}
                </div>
              {/snippet}
              {@render row("Accent", "Buttons, links and the current selection.", accentControl)}

              {#snippet densityControl()}
                <div class={segTrack}>
                  {#each [["comfortable", "Comfortable", Rows3], ["compact", "Compact", Rows2]] as [val, label, Icon]}
                    {@const IconComp = Icon as typeof Rows3}
                    <button class="{segButton} {densityPref === val ? segOn : segOff}" aria-pressed={densityPref === val} onclick={() => pickDensity(val as Density)}>
                      <IconComp size={14} />
                      {label}
                    </button>
                  {/each}
                </div>
              {/snippet}
              {@render row("Density", "How tightly lists and boards are packed.", densityControl)}

              {#snippet sizeControl()}
                <div class={segTrack}>
                  {#each [["sm", "S"], ["md", "M"], ["lg", "L"]] as [val, label]}
                    <button class="px-3.5 py-1.5 rounded-md text-body-sm font-medium transition-all {fontScalePref === val ? segOn : segOff}" aria-pressed={fontScalePref === val} onclick={() => pickFontScale(val as FontScale)}>
                      {label}
                    </button>
                  {/each}
                </div>
              {/snippet}
              {@render row("Text size", "Scales the whole interface.", sizeControl)}

              {#snippet motionControl()}
                <div class={segTrack}>
                  {#each [["system", "System", Monitor], ["reduced", "Reduced", ZapOff], ["full", "Full", Zap]] as [val, label, Icon]}
                    {@const IconComp = Icon as typeof Monitor}
                    <button class="{segButton} {motionPref === val ? segOn : segOff}" aria-pressed={motionPref === val} onclick={() => pickMotion(val as MotionPreference)}>
                      <IconComp size={14} />
                      {label}
                    </button>
                  {/each}
                </div>
              {/snippet}
              {@render row("Motion", "System follows your OS's reduced-motion setting.", motionControl)}
            </div>
          </section>

          <!-- ── AGENTS ──────────────────────────────────────── -->
          <section id="settings-agents" class="scroll-mt-6" aria-labelledby="settings-agents-heading">
            <AgentAccounts
              {bots}
              userId={user.id}
              username={user.username}
              {webAutoLogin}
              reload={loadBots}
              {navigate}
            />
          </section>

          <!-- ── SECURITY ────────────────────────────────────── -->
          <section id="settings-security" class="scroll-mt-6" aria-labelledby="settings-security-heading">
            {@render sectionHead("security", "Security")}
            <div class="rounded-xl bg-[var(--surface)] shadow-[0_1px_2px_rgba(0,0,0,0.06)]">
              {#snippet passwordControl()}
                <div class="max-w-[36ch] flex flex-col gap-2">
                  <label class="block">
                    <span class="sr-only">Current password</span>
                    <input type="password" bind:value={curPw} placeholder="Current password" autocomplete="current-password" class={inputClass} />
                  </label>
                  <label class="block">
                    <span class="sr-only">New password</span>
                    <input
                      type="password" bind:value={newPw} placeholder="New password, 8 characters or more" autocomplete="new-password" class={inputClass}
                      onkeydown={(e) => { if (e.key === "Enter") submitPassword(); }}
                    />
                  </label>
                  {#if pwError}
                    <p class="text-caption text-[var(--error)] flex items-center gap-1" role="alert"><AlertTriangle size={12} /> {pwError}</p>
                  {/if}
                  <div class="flex flex-wrap items-center gap-3 mt-1">
                    <button
                      class="text-body-sm font-medium text-[var(--text)] border border-[var(--border)] px-3 py-1.5 rounded-md
                             hover:bg-[var(--bg-subtle)] transition-colors disabled:opacity-40 disabled:cursor-not-allowed"
                      disabled={pwSaving || !curPw || !newPw}
                      onclick={submitPassword}
                    >
                      {pwSaving ? "Changing…" : "Change password"}
                    </button>
                  </div>
                  {#if pwSuccess}
                    <p class="text-caption text-[var(--success)] flex items-start gap-1" role="status">
                      <Check size={13} class="shrink-0 mt-px" /> Password changed. Your agents need new keys.
                    </p>
                  {/if}
                </div>
              {/snippet}
              {@render row(
                "Password",
                "Changing it signs you out on other devices and revokes every API key and OAuth sign-in on your account, your agents' included. This browser stays signed in.",
                passwordControl,
              )}

              {#snippet signOutControl()}
                <button
                  class="inline-flex items-center gap-1.5 text-body-sm font-medium text-[var(--text)] border border-[var(--border)] px-3 py-1.5 rounded-md
                         hover:bg-[var(--bg-subtle)] transition-colors disabled:opacity-50"
                  disabled={loggingOut}
                  onclick={logoutNow}
                >
                  <LogOut size={14} />
                  {loggingOut ? "Signing out" : "Sign out"}
                </button>
                {#if logoutError}
                  <p class="mt-2 text-caption text-[var(--error)] flex items-center gap-1" role="alert"><AlertTriangle size={12} /> {logoutError}</p>
                {/if}
              {/snippet}
              {@render row("This browser", "Ends the session here. Everything else keeps working.", signOutControl)}

              {#snippet everywhereControl()}
                <button
                  class="text-body-sm text-[var(--error)] border border-[color-mix(in_oklab,var(--error)_45%,transparent)] px-3 py-1.5 rounded-md
                         hover:bg-[var(--error-bg)] transition-colors disabled:opacity-50"
                  disabled={signingOut}
                  aria-expanded={confirmingSignOutAll}
                  onclick={() => { confirmingSignOutAll = !confirmingSignOutAll; signOutAllError = ""; }}
                >
                  Sign out everywhere
                </button>
                {#if confirmingSignOutAll}
                  <div class="mt-3 flex flex-col gap-2.5 rounded-lg bg-[var(--error-bg)] px-3.5 py-3" role="alert">
                    <p class="text-body-sm text-[var(--text)] leading-relaxed">
                      Every device signs out, this one included, and every API key and OAuth sign-in on
                      your account and your agents stops working. Agents need new keys afterwards.
                    </p>
                    {#if signOutAllError}
                      <p class="text-caption text-[var(--error)] flex items-center gap-1"><AlertTriangle size={12} /> {signOutAllError}</p>
                    {/if}
                    <div class="flex items-center gap-2">
                      <button
                        class="text-body-sm font-medium text-[var(--error-text)] bg-[var(--error)] px-3 py-1.5 rounded-md hover:opacity-90 transition-opacity disabled:opacity-50 disabled:cursor-not-allowed"
                        disabled={signingOut}
                        onclick={signOutAll}
                      >
                        {signingOut ? "Revoking…" : "Yes, revoke everything"}
                      </button>
                      <button
                        class="text-body-sm text-[var(--text-muted)] px-3 py-1.5 rounded-md hover:bg-[var(--bg-subtle)] transition-colors disabled:opacity-50"
                        disabled={signingOut}
                        onclick={() => { confirmingSignOutAll = false; signOutAllError = ""; }}
                      >
                        Cancel
                      </button>
                    </div>
                  </div>
                {/if}
              {/snippet}
              {@render row("Everywhere", "For a lost device or a leaked key.", everywhereControl)}
            </div>
          </section>
        </div>
      </div>
    {/if}
  </div>
</div>

<script lang="ts">
  import {
    me,
    listBots,
    createBot,
    disconnectBot,
    deleteBot,
    updateProfile,
    changePassword,
    revokeAllSessions,
    logout,
    clearSession,
    TOOL_TEMPLATES,
    type AuthUser,
    type Bot,
    type ToolTemplate,
  } from "../lib/api";
  import ToolIcon from "../lib/ToolIcon.svelte";
  import SettingsTabs from "../lib/SettingsTabs.svelte";
  import {
    getPreference, setPreference, type ThemePreference,
  } from "../lib/theme";
  import {
    Plug, Check, Copy, X, AlertTriangle, Sun, Moon, Monitor,
    Palette, Lock, LogOut, Eye, EyeOff, KeyRound, FileCode2, Terminal,
  } from "lucide-svelte";
  import { getContext } from "svelte";

  let { navigate }: { navigate: (path: string) => void } = $props();

  const topbarCtx = getContext<{
    set: (s: import("svelte").Snippet | undefined) => void;
  } | undefined>("lific:topbar");
  $effect(() => {
    topbarCtx?.set(topbarContent);
    return () => topbarCtx?.set(undefined);
  });

  let user = $state<AuthUser | null>(null);
  let bots = $state<Bot[]>([]);
  let loading = $state(true);

  // Connect modal
  let connectTool = $state<ToolTemplate | null>(null);
  let connectKey = $state<string | null>(null);
  let connecting = $state(false);
  let connectError = $state("");
  let copied = $state(false);
  let keyCopied = $state(false);
  let exportCopied = $state(false);
  // Which note-step command chip was just copied (by index), so only that
  // chip flashes "copied".
  let noteCopiedIdx = $state<number | null>(null);
  // The minted key is sensitive — hidden by default, revealed on demand.
  let keyRevealed = $state(false);

  // Masked rendering: keep the non-secret prefix (e.g. "lific_sk-live-")
  // legible so the user can sanity-check the key shape, dot out the rest.
  let maskedKey = $derived.by(() => {
    if (!connectKey) return "";
    const m = connectKey.match(/^(lific_sk-live-|lific_sk-test-|lific_sk-|sk-live-|sk-)/);
    const prefix = m ? m[0] : connectKey.slice(0, 6);
    const hiddenLen = Math.max(connectKey.length - prefix.length, 0);
    return prefix + "•".repeat(Math.min(hiddenLen, 40));
  });
  let displayKey = $derived(keyRevealed ? (connectKey ?? "") : maskedKey);

  // OS selector for the config-path display. Best-effort default from the
  // browser; the user can override since they may be configuring a tool on a
  // different machine than the one viewing this page.
  type Os = "linux" | "mac" | "windows";
  const OS_LABELS: Record<Os, string> = { linux: "Linux", mac: "macOS", windows: "Windows" };
  function detectOs(): Os {
    const p = `${navigator.platform} ${navigator.userAgent}`.toLowerCase();
    if (p.includes("win")) return "windows";
    if (p.includes("mac")) return "mac";
    return "linux";
  }
  let selectedOs = $state<Os>(detectOs());

  // Group OSes that share an identical config path so the selector merges
  // them into one button (e.g. "Linux / macOS" when only Windows differs).
  // Order is stable: Linux, macOS, Windows.
  type OsGroup = { oses: Os[]; label: string };
  let osGroups = $derived.by<OsGroup[]>(() => {
    if (!connectTool) return [];
    const order: Os[] = ["linux", "mac", "windows"];
    const groups: OsGroup[] = [];
    for (const os of order) {
      const path = connectTool.configPath[os];
      // Skip OSes the tool doesn't support (null path) — e.g. Claude
      // Desktop on Linux, which Anthropic doesn't ship.
      if (!path) continue;
      const existing = groups.find(
        (g) => connectTool!.configPath[g.oses[0]] === path,
      );
      if (existing) existing.oses.push(os);
      else groups.push({ oses: [os], label: "" });
    }
    return groups.map((g) => ({
      oses: g.oses,
      label: g.oses.map((o) => OS_LABELS[o]).join(" / "),
    }));
  });
  // True when the tool has more than one distinct path across OSes.
  let pathsDiffer = $derived(osGroups.length > 1);
  let activePath = $derived(connectTool ? connectTool.configPath[selectedOs] : "");

  // The command that sets the env var. macOS and Linux share the same POSIX
  // `export` syntax (both use POSIX shells; the only difference is which
  // rc-file persists it). Windows uses `setx`, which differs meaningfully:
  // it writes to the registry for FUTURE shells and does NOT affect the
  // current one.
  let exportLine = $derived.by(() => {
    if (!connectTool?.usesEnvKey || !connectKey) return "";
    const v = connectTool.envVar ?? "LIFIC_API_KEY";
    return selectedOs === "windows"
      ? `setx ${v} "${connectKey}"`
      : `export ${v}="${connectKey}"`;
  });

  // Where to put the line so it survives new terminals (OS/shell-specific).
  let persistHint = $derived.by(() => {
    switch (selectedOs) {
      case "mac":
        return "Runs in the current shell. To persist it, add the line to ~/.zshrc (the default macOS shell).";
      case "linux":
        return "Runs in the current shell. To persist it, add the line to ~/.bashrc or ~/.profile.";
      case "windows":
        return "setx applies to new terminals, not the current one. Reopen your terminal (or Pi) after running it.";
    }
  });

  let busyId = $state<number | null>(null);

  // Profile edit (a proper labeled form, separate from the read-only
  // identity header — name/email aren't casual click-to-edit fields).
  let profileName = $state("");
  let profileEmail = $state("");
  let profileSaving = $state(false);
  let profileError = $state("");
  let profileSavedAt = $state(0);

  // Appearance
  let themePref = $state<ThemePreference>("system");

  // Security
  let curPw = $state("");
  let newPw = $state("");
  let pwError = $state("");
  let pwSaving = $state(false);
  let pwSuccess = $state(false);
  let signingOut = $state(false);

  $effect(() => {
    themePref = getPreference();
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
    profileSaving = true;
    profileError = "";
    const input: { display_name?: string; email?: string } = {};
    if (profileName.trim() !== user.display_name) input.display_name = profileName.trim();
    if (profileEmail.trim().toLowerCase() !== user.email) input.email = profileEmail.trim();
    const res = await updateProfile(input);
    profileSaving = false;
    if (res.ok) {
      user = res.data;
      profileName = res.data.display_name;
      profileEmail = res.data.email;
      profileSavedAt = Date.now();
      window.setTimeout(() => { if (Date.now() - profileSavedAt >= 1900) profileSavedAt = 0; }, 2000);
    } else {
      profileError = res.error;
    }
  }

  function pickTheme(p: ThemePreference) {
    themePref = p;
    setPreference(p);
  }

  async function submitPassword() {
    if (pwSaving) return;
    pwError = ""; pwSuccess = false;
    if (newPw.length < 8) { pwError = "New password must be at least 8 characters."; return; }
    pwSaving = true;
    const res = await changePassword({ current_password: curPw, new_password: newPw });
    pwSaving = false;
    if (res.ok) {
      pwSuccess = true;
      curPw = ""; newPw = "";
      window.setTimeout(() => { pwSuccess = false; }, 2500);
    } else {
      pwError = res.error;
    }
  }

  async function signOutAll() {
    if (signingOut) return;
    signingOut = true;
    await revokeAllSessions();
    navigate("/login");
  }

  async function logoutNow() {
    await logout();
    clearSession();
    navigate("/login");
  }

  async function loadUser() {
    const result = await me();
    if (result.ok) {
      user = result.data;
      profileName = result.data.display_name;
      profileEmail = result.data.email;
      await loadBots();
    }
    loading = false;
  }
  async function loadBots() {
    const result = await listBots();
    if (result.ok) bots = result.data;
  }

  function getToolBot(toolId: string): Bot | undefined {
    if (!user) return undefined;
    return bots.find((b) => b.username === `${toolId}-${user!.username}`);
  }
  function toolState(toolId: string): "connected" | "disconnected" | "none" {
    const bot = getToolBot(toolId);
    if (!bot) return "none";
    return bot.has_active_key ? "connected" : "disconnected";
  }

  // Open the modal and mint credentials in one step (no extra confirm).
  async function openConnect(template: ToolTemplate) {
    connectTool = template;
    connectKey = null;
    connectError = "";
    copied = false;
    keyCopied = false;
    exportCopied = false;
    noteCopiedIdx = null;
    keyRevealed = false;
    // Default to the viewer's OS, but if the tool has no config for it
    // (e.g. Claude Desktop on Linux), fall back to the first OS it does
    // support so the modal never opens on an empty path.
    {
      const detected = detectOs();
      selectedOs = template.configPath[detected]
        ? detected
        : ((["linux", "mac", "windows"] as Os[]).find(
            (o) => template.configPath[o],
          ) ?? "mac");
    }
    connecting = true;
    const res = await createBot(template.id);
    if (res.ok) {
      connectKey = res.data.key;
      await loadBots();
    } else {
      connectError = res.error;
    }
    connecting = false;
  }
  function closeConnect() {
    connectTool = null;
    connectKey = null;
    connectError = "";
    connecting = false;
  }

  function configText(): string {
    if (!connectTool || !connectKey) return "";
    return connectTool.generateConfig(window.location.origin + "/mcp", connectKey);
  }
  // What's rendered in the config block. Masks the embedded key to match
  // Step 1's hide-by-default behavior; copy always uses the real configText().
  let displayConfig = $derived.by(() => {
    const text = configText();
    if (keyRevealed || !connectKey) return text;
    return text.split(connectKey).join(maskedKey);
  });
  async function copyToClipboard(text: string, flag: (v: boolean) => void) {
    try {
      await navigator.clipboard.writeText(text);
      flag(true);
      window.setTimeout(() => flag(false), 1500);
    } catch { /* clipboard blocked */ }
  }
  const copyConfig = () => copyToClipboard(configText(), (v) => (copied = v));
  const copyKey = () => connectKey && copyToClipboard(connectKey, (v) => (keyCopied = v));
  const copyExport = () => copyToClipboard(exportLine, (v) => (exportCopied = v));
  async function copyNote(idx: number, command: string) {
    await copyToClipboard(command, (v) => (noteCopiedIdx = v ? idx : null));
  }

  async function handleDisconnect(id: number) {
    busyId = id;
    await disconnectBot(id);
    await loadBots();
    busyId = null;
  }
  async function handleRemove(id: number) {
    busyId = id;
    await deleteBot(id);
    await loadBots();
    busyId = null;
  }

  function initials(name: string): string {
    return name.split(/[\s_-]+/).slice(0, 2).map((w) => w[0]?.toUpperCase() ?? "").join("");
  }
</script>

<svelte:window onkeydown={(e) => { if (e.key === "Escape" && connectTool) closeConnect(); }} />

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
    {:else if user}
      <SettingsTabs active="account" isAdmin={user.is_admin} {navigate} />

      <!-- ── IDENTITY HERO (read-only) ────────────────── -->
      <section class="flex items-center gap-4 mb-8 animate-reveal delay-100">
        <div class="size-14 shrink-0 rounded-full bg-[var(--accent)] text-[var(--accent-text)] grid place-items-center font-display text-[1.25rem] tracking-tight">
          {initials(user.display_name || user.username)}
        </div>
        <div class="min-w-0">
          <h1 class="font-display text-[1.5rem] tracking-tight text-[var(--text)] leading-none truncate">
            {user.display_name || user.username}
          </h1>
          <div class="flex items-center gap-2 mt-1.5 flex-wrap text-body-sm">
            <span class="font-mono text-[var(--text-muted)]">@{user.username}</span>
            <span class="text-[var(--text-faint)]">·</span>
            <span class="text-[var(--text-muted)]">{user.email}</span>
            <span class="text-micro font-semibold uppercase tracking-wide px-1.5 py-0.5 rounded-full
                         {user.is_admin ? 'text-[var(--accent)] bg-[var(--accent-subtle)]' : 'text-[var(--text-muted)] bg-[var(--bg-subtle)]'}">
              {user.is_admin ? "Admin" : "Member"}
            </span>
          </div>
        </div>
      </section>

      <!-- ── APPEARANCE ───────────────────────────────── -->
      <section class="rounded-xl bg-[var(--surface)] shadow-[0_1px_2px_rgba(0,0,0,0.06)] p-5 animate-reveal delay-250">
        <div class="flex items-center gap-2 mb-1">
          <Palette size={15} class="text-[var(--text-muted)]" />
          <h2 class="text-[0.9375rem] font-semibold text-[var(--text)]">Appearance</h2>
        </div>
        <p class="text-body-sm text-[var(--text-muted)] mb-3.5">System follows your OS.</p>
        <div class="inline-flex p-0.5 rounded-lg bg-[var(--bg)] shadow-[inset_0_1px_2px_rgba(0,0,0,0.10)]">
          {#each [["light", "Light", Sun], ["dark", "Dark", Moon], ["system", "System", Monitor]] as [val, label, Icon]}
            {@const IconComp = Icon as typeof Sun}
            <button
              class="flex items-center gap-1.5 px-3 py-1.5 rounded-md text-body-sm font-medium transition-all
                     {themePref === val
                ? 'bg-[var(--surface)] text-[var(--text)] shadow-[0_1px_2px_rgba(0,0,0,0.12)]'
                : 'text-[var(--text-muted)] hover:text-[var(--text)]'}"
              onclick={() => pickTheme(val as ThemePreference)}
            >
              <IconComp size={14} />
              {label}
            </button>
          {/each}
        </div>
      </section>

      <!-- ── CONNECTED TOOLS (full width) ─────────────── -->
      <section class="mt-10 animate-reveal delay-250">
        <div class="flex items-center gap-2 mb-1">
          <Plug size={16} class="text-[var(--text-muted)]" />
          <h2 class="text-[1rem] font-semibold text-[var(--text)]">Connected tools</h2>
        </div>
        <p class="text-body text-[var(--text-muted)] mb-5 leading-relaxed">
          Link an AI coding tool to Lific over MCP. Each connection mints a bot identity that acts on your behalf; disconnect any time.
        </p>

        <div class="grid sm:grid-cols-2 gap-2.5">
          {#each TOOL_TEMPLATES as template (template.id)}
            {@const st = toolState(template.id)}
            {@const bot = getToolBot(template.id)}
            <div class="flex items-center gap-3.5 p-3.5 rounded-xl bg-[var(--surface)] shadow-[0_1px_2px_rgba(0,0,0,0.06)]">
              <div class="size-10 shrink-0 rounded-lg bg-[var(--bg-subtle)] grid place-items-center text-[var(--text)]">
                <ToolIcon tool={template.id} size={20} />
              </div>
              <div class="flex-1 min-w-0">
                <div class="flex items-center gap-2">
                  <span class="text-body font-medium text-[var(--text)]">{template.name}</span>
                  {#if st === "connected"}
                    <span class="inline-flex items-center gap-1 text-micro font-semibold uppercase tracking-wide
                                 text-[var(--success)] bg-[var(--success-bg)] px-1.5 py-0.5 rounded-full">
                      <span class="size-1.5 rounded-full bg-[var(--success)]"></span> Connected
                    </span>
                  {:else if st === "disconnected"}
                    <span class="text-micro font-semibold uppercase tracking-wide text-[var(--warn)] bg-[color-mix(in_oklab,var(--warn)_14%,transparent)] px-1.5 py-0.5 rounded-full">
                      Disconnected
                    </span>
                  {/if}
                </div>
                <p class="text-caption text-[var(--text-muted)] truncate mt-0.5">{template.description}</p>
              </div>
              <div class="shrink-0 flex items-center gap-1.5">
                {#if st === "connected" && bot}
                  <button
                    class="text-body-sm text-[var(--text-muted)] hover:text-[var(--error)] px-2.5 py-1.5 rounded-md hover:bg-[var(--error-bg)] transition-colors disabled:opacity-50"
                    disabled={busyId === bot.id}
                    onclick={() => handleDisconnect(bot.id)}
                  >
                    {busyId === bot.id ? "…" : "Disconnect"}
                  </button>
                {:else if st === "disconnected" && bot}
                  <button
                    class="text-body-sm text-[var(--text-faint)] hover:text-[var(--error)] px-2 py-1.5 rounded-md hover:bg-[var(--error-bg)] transition-colors disabled:opacity-50"
                    disabled={busyId === bot.id}
                    onclick={() => handleRemove(bot.id)}
                  >
                    Remove
                  </button>
                  <button
                    class="flex items-center gap-1.5 text-body-sm font-medium text-[var(--btn-success-text)]
                           bg-[var(--btn-success)] px-3 py-1.5 rounded-md hover:bg-[var(--btn-success-hover)] transition-colors"
                    onclick={() => openConnect(template)}
                  >
                    Reconnect
                  </button>
                {:else}
                  <button
                    class="flex items-center gap-1.5 text-body-sm font-medium text-[var(--btn-success-text)]
                           bg-[var(--btn-success)] px-3 py-1.5 rounded-md hover:bg-[var(--btn-success-hover)] transition-colors
                           motion-safe:active:scale-[0.97]"
                    onclick={() => openConnect(template)}
                  >
                    Connect
                  </button>
                {/if}
              </div>
            </div>
          {/each}
        </div>
      </section>

      <!-- ── ACCOUNT (profile + security, bottom of page) ─── -->
      <section class="mt-10 pt-8 border-t border-[var(--border)] animate-reveal delay-250">
        <h2 class="text-[1rem] font-semibold text-[var(--text)] mb-1">Account</h2>
        <p class="text-body text-[var(--text-muted)] mb-6 leading-relaxed">
          Manage your profile, password, and sessions.
        </p>

        <!-- Profile (proper labeled form) -->
        <div class="max-w-[480px] flex flex-col gap-3.5">
          <label class="block">
            <span class="block text-micro font-semibold uppercase tracking-widest text-[var(--text-faint)] mb-1.5">Display name</span>
            <input
              bind:value={profileName}
              class="w-full px-3 py-2 text-body rounded-md border border-[var(--border)] bg-[var(--bg)] text-[var(--text)]
                     outline-none focus-visible:ring-2 focus-visible:ring-[var(--accent)]"
            />
          </label>
          <label class="block">
            <span class="block text-micro font-semibold uppercase tracking-widest text-[var(--text-faint)] mb-1.5">Email</span>
            <input
              bind:value={profileEmail}
              type="email"
              class="w-full px-3 py-2 text-body rounded-md border border-[var(--border)] bg-[var(--bg)] text-[var(--text)]
                     outline-none focus-visible:ring-2 focus-visible:ring-[var(--accent)]"
            />
          </label>
          <div>
            <span class="block text-micro font-semibold uppercase tracking-widest text-[var(--text-faint)] mb-1.5">Username</span>
            <p class="text-body font-mono text-[var(--text-muted)]">@{user.username}</p>
          </div>
        </div>
        {#if profileError}
          <p class="text-caption text-[var(--error)] mt-2.5 flex items-center gap-1"><AlertTriangle size={12} /> {profileError}</p>
        {/if}
        <div class="flex items-center gap-3 mt-4">
          <button
            class="text-body-sm font-medium text-[var(--btn-success-text)] bg-[var(--btn-success)] px-3 py-1.5 rounded-md
                   hover:bg-[var(--btn-success-hover)] transition-colors disabled:opacity-40 disabled:cursor-not-allowed"
            disabled={!hasProfileChanges || profileSaving}
            onclick={saveProfile}
          >
            {profileSaving ? "Saving…" : "Save changes"}
          </button>
          {#if profileSavedAt}
            <span class="inline-flex items-center gap-1 text-body-sm text-[var(--success)]" aria-live="polite"><Check size={13} /> Saved</span>
          {/if}
        </div>

        <!-- Password -->
        <div class="mt-8 pt-6 border-t border-[var(--border)]">
          <div class="flex items-center gap-2 mb-3.5">
            <Lock size={15} class="text-[var(--text-muted)]" />
            <h3 class="text-[0.9375rem] font-semibold text-[var(--text)]">Password</h3>
          </div>
          <div class="max-w-[480px] flex flex-col gap-2.5">
            <input
              type="password" bind:value={curPw} placeholder="Current password" autocomplete="current-password"
              class="px-3 py-2 text-body rounded-md border border-[var(--border)] bg-[var(--bg)] text-[var(--text)]
                     outline-none focus-visible:ring-2 focus-visible:ring-[var(--accent)]"
            />
            <input
              type="password" bind:value={newPw} placeholder="New password (min 8 chars)" autocomplete="new-password"
              class="px-3 py-2 text-body rounded-md border border-[var(--border)] bg-[var(--bg)] text-[var(--text)]
                     outline-none focus-visible:ring-2 focus-visible:ring-[var(--accent)]"
              onkeydown={(e) => { if (e.key === 'Enter') submitPassword(); }}
            />
          </div>
          {#if pwError}
            <p class="text-caption text-[var(--error)] mt-2 flex items-center gap-1"><AlertTriangle size={12} /> {pwError}</p>
          {/if}
          <div class="flex items-center gap-3 mt-3">
            <button
              class="text-body-sm font-medium text-[var(--btn-success-text)] bg-[var(--btn-success)] px-3 py-1.5 rounded-md
                     hover:bg-[var(--btn-success-hover)] transition-colors disabled:opacity-40 disabled:cursor-not-allowed"
              disabled={pwSaving || !curPw || !newPw}
              onclick={submitPassword}
            >
              {pwSaving ? "Updating…" : "Change password"}
            </button>
            {#if pwSuccess}
              <span class="inline-flex items-center gap-1 text-body-sm text-[var(--success)]"><Check size={13} /> Changed</span>
            {/if}
          </div>
        </div>

        <!-- Sessions (sign out) — the very bottom of the page -->
        <div class="mt-8 pt-6 border-t border-[var(--border)]">
          <h3 class="text-[0.9375rem] font-semibold text-[var(--text)] mb-1">Sessions</h3>
          <p class="text-body-sm text-[var(--text-muted)] mb-3.5 leading-relaxed">
            Sign out of this device, or revoke every active session everywhere.
          </p>
          <div class="flex flex-wrap items-center gap-2">
            <button
              class="inline-flex items-center gap-1.5 text-body-sm font-medium text-[var(--text)]
                     border border-[var(--border)] px-3 py-1.5 rounded-md
                     hover:bg-[var(--bg-subtle)] transition-colors"
              onclick={logoutNow}
            >
              <LogOut size={14} />
              Sign out
            </button>
            <button
              class="inline-flex items-center gap-1.5 text-body-sm text-[var(--error)] border border-[var(--error)]
                     px-3 py-1.5 rounded-md hover:bg-[var(--error-bg)] transition-colors disabled:opacity-50"
              disabled={signingOut}
              onclick={signOutAll}
            >
              {signingOut ? "Signing out…" : "Sign out of all sessions"}
            </button>
          </div>
        </div>
      </section>
    {/if}
  </div>
</div>

<!-- ── CONNECT MODAL ──────────────────────────────────── -->
{#if connectTool}
  <!-- svelte-ignore a11y_click_events_have_key_events a11y_no_static_element_interactions -->
  <div class="fixed inset-0 z-50 bg-black/50 grid place-items-center p-4" onclick={closeConnect}>
    <!-- svelte-ignore a11y_click_events_have_key_events a11y_no_static_element_interactions -->
    <div
      class="w-full max-w-[620px] bg-[var(--surface)] border border-[var(--border)] rounded-xl shadow-2xl
             max-h-[85vh] overflow-y-auto"
      onclick={(e) => e.stopPropagation()}
    >
      <!-- Header -->
      <div class="flex items-center gap-3 px-5 py-4 border-b border-[var(--border)]">
        <div class="size-9 shrink-0 rounded-lg bg-[var(--bg-subtle)] grid place-items-center text-[var(--text)]">
          <ToolIcon tool={connectTool.id} size={18} />
        </div>
        <div class="flex-1 min-w-0">
          <h3 class="text-[0.9375rem] font-semibold text-[var(--text)] leading-tight">Connect {connectTool.name}</h3>
          <p class="text-caption text-[var(--text-muted)] truncate">{connectTool.description}</p>
        </div>
        <button class="size-7 grid place-items-center rounded-md text-[var(--text-muted)] hover:text-[var(--text)] hover:bg-[var(--bg-subtle)] transition-colors" onclick={closeConnect} aria-label="Close">
          <X size={16} />
        </button>
      </div>

      <div class="p-5">
        {#if connecting}
          <div class="flex items-center gap-3 py-8 justify-center text-[var(--text-muted)]">
            <div class="size-5 rounded-full border-2 border-[var(--border)] border-t-[var(--accent)] animate-spin"></div>
            <span class="text-body">Minting credentials…</span>
          </div>
        {:else if connectError}
          <div class="flex items-start gap-2.5 text-body text-[var(--error)] bg-[var(--error-bg)] px-3.5 py-3 rounded-md" role="alert">
            <AlertTriangle size={16} class="shrink-0 mt-0.5" />
            <span>{connectError}</span>
          </div>
        {:else if connectKey}
          {@const stepCount = connectTool.usesEnvKey ? 3 : 2}

          <!-- ── STEP 1 · API KEY ───────────────────────────────
               Shown once; hidden behind a mask by default. Surfaced as its
               own first-class step so env-var tools (Pi, Codex) never bury
               the secret inside a config the user might not read. -->
          <section class="mb-5">
            <div class="flex items-center gap-2.5 mb-2">
              <span class="size-5 shrink-0 grid place-items-center rounded-full bg-[var(--btn-success)] text-[var(--btn-success-text)] text-micro font-bold tabular-nums">1</span>
              <h4 class="flex items-center gap-1.5 text-body font-semibold text-[var(--text)]">
                <KeyRound size={14} class="text-[var(--text-muted)]" /> Copy your API key
              </h4>
            </div>

            <div class="rounded-lg border border-[var(--btn-success)] bg-[color-mix(in_oklab,var(--btn-success)_8%,var(--bg))] overflow-hidden">
              <div class="flex items-center gap-2 px-3 py-2.5">
                <code
                  class="flex-1 min-w-0 text-body-sm font-mono text-[var(--text)] overflow-x-auto whitespace-nowrap leading-none py-0.5
                         {keyRevealed ? '' : 'select-none cursor-default'}"
                >{displayKey}</code>
                <button
                  class="shrink-0 size-7 grid place-items-center rounded-md text-[var(--text-muted)] hover:text-[var(--text)] hover:bg-[var(--bg-subtle)] transition-colors"
                  onclick={() => (keyRevealed = !keyRevealed)}
                  title={keyRevealed ? "Hide key" : "Reveal key"}
                  aria-label={keyRevealed ? "Hide key" : "Reveal key"}
                >
                  {#if keyRevealed}<EyeOff size={15} />{:else}<Eye size={15} />{/if}
                </button>
              </div>
              <button
                class="w-full flex items-center justify-center gap-1.5 py-2 text-body-sm font-semibold border-t border-[color-mix(in_oklab,var(--btn-success)_35%,transparent)] transition-colors
                       {keyCopied
                  ? 'bg-[var(--success-bg)] text-[var(--success)]'
                  : 'bg-[var(--btn-success)] text-[var(--btn-success-text)] hover:bg-[var(--btn-success-hover)]'}"
                onclick={copyKey}
              >
                {#if keyCopied}<Check size={14} /> Copied to clipboard{:else}<Copy size={14} /> Copy key{/if}
              </button>
            </div>
          </section>

          <!-- ── STEP 2 · CONFIG ────────────────────────────────── -->
          <section class="mb-5">
            <div class="flex items-center gap-2.5 mb-2">
              <span class="size-5 shrink-0 grid place-items-center rounded-full bg-[var(--btn-success)] text-[var(--btn-success-text)] text-micro font-bold tabular-nums">2</span>
              <h4 class="flex items-center gap-1.5 text-body font-semibold text-[var(--text)]">
                <FileCode2 size={14} class="text-[var(--text-muted)]" /> Add to your config
              </h4>
            </div>

            <!-- OS selector: its own full-width segmented row. OSes that share
                 an identical path are merged into one button (e.g. Linux / macOS).
                 A single group means the path is the same everywhere. -->
            <div class="flex items-center gap-2 mb-2.5">
              <div class="inline-flex flex-1 p-0.5 rounded-lg bg-[var(--bg)] shadow-[inset_0_1px_2px_rgba(0,0,0,0.08)]">
                {#each osGroups as group (group.label)}
                  {@const active = group.oses.includes(selectedOs)}
                  <button
                    class="flex-1 px-2 py-1.5 rounded-md text-caption font-medium transition-all
                           {active
                      ? 'bg-[var(--btn-success)] text-[var(--btn-success-text)] shadow-[0_1px_2px_rgba(0,0,0,0.12)]'
                      : 'text-[var(--text-muted)] hover:text-[var(--text)]'}"
                    onclick={() => (selectedOs = group.oses[0])}
                  >
                    {group.label}
                  </button>
                {/each}
              </div>
              {#if !pathsDiffer}
                <span class="text-micro text-[var(--text-muted)] shrink-0">same everywhere</span>
              {/if}
            </div>

            <div class="flex items-center gap-2 mb-2.5">
              <span class="text-micro font-semibold uppercase tracking-wide text-[var(--text-muted)] shrink-0">File</span>
              <code class="flex-1 min-w-0 font-mono text-caption bg-[var(--bg-subtle)] px-2 py-1 rounded text-[var(--text)] overflow-x-auto whitespace-nowrap">{activePath}</code>
            </div>

            {#if connectTool.configNote}
              <div class="flex flex-col gap-1.5 mb-2.5">
                {#each connectTool.configNote as step, i (i)}
                  {#if step.text}
                    <p class="text-body-sm text-[var(--text)] leading-snug">{step.text}</p>
                  {/if}
                  {#if step.command}
                    <!-- Same shape as the API-key card: content row on top,
                         full-width copy bar underneath. No floating button
                         overlapping a single line of scrolling text. -->
                    <div class="rounded-lg bg-[var(--bg)] overflow-hidden ring-1 ring-[var(--border)]">
                      <pre class="px-3 py-2.5 text-caption font-mono text-[var(--text)] overflow-x-auto whitespace-pre">{step.command}</pre>
                      <!-- Separator matches the bar's own bg so no seam fights
                           the rounded container. Green-tinted bar + green text:
                           lively and on-brand without a loud solid button. -->
                      <button
                        class="w-full flex items-center justify-center gap-1.5 py-2 text-caption font-semibold transition-colors
                               {noteCopiedIdx === i
                          ? 'bg-[var(--success-bg)] text-[var(--success)]'
                          : 'bg-[color-mix(in_oklab,var(--btn-success)_14%,var(--bg))] text-[var(--success)] hover:bg-[color-mix(in_oklab,var(--btn-success)_22%,var(--bg))]'}"
                        onclick={() => copyNote(i, step.command!)}
                      >
                        {#if noteCopiedIdx === i}<Check size={13} /> Copied{:else}<Copy size={13} /> Copy command{/if}
                      </button>
                    </div>
                  {/if}
                {/each}
              </div>
            {/if}

            <div class="relative">
              <pre
                class="bg-[var(--bg)] border border-[var(--border)] rounded-lg p-3.5 pr-12 text-caption font-mono text-[var(--text)] overflow-x-auto leading-relaxed
                       {keyRevealed ? '' : 'select-none'}"
              >{displayConfig}</pre>
              <button
                class="absolute top-2 right-2 inline-flex items-center gap-1 text-micro font-semibold
                       px-2 py-1 rounded-md bg-[var(--surface)] border border-[var(--border)]
                       {copied ? 'text-[var(--success)]' : 'text-[var(--text-muted)] hover:text-[var(--text)]'} transition-colors"
                onclick={copyConfig}
              >
                {#if copied}<Check size={12} /> Copied{:else}<Copy size={12} /> Copy{/if}
              </button>
            </div>
          </section>

          <!-- ── STEP 3 · ENV VAR (env-key tools only) ──────────── -->
          {#if connectTool.usesEnvKey}
            <section class="mb-5">
              <div class="flex items-center gap-2.5 mb-2">
                <span class="size-5 shrink-0 grid place-items-center rounded-full bg-[var(--btn-success)] text-[var(--btn-success-text)] text-micro font-bold tabular-nums">3</span>
                <h4 class="flex items-center gap-1.5 text-body font-semibold text-[var(--text)]">
                  <Terminal size={14} class="text-[var(--text-muted)]" /> Set the env var
                  <span class="font-normal text-[var(--text-muted)]">· {OS_LABELS[selectedOs]}</span>
                </h4>
              </div>
              <div class="relative">
                <pre
                  class="bg-[var(--bg)] border border-[var(--border)] rounded-lg p-3 pr-12 text-caption font-mono text-[var(--text)] overflow-x-auto
                         {keyRevealed ? '' : 'select-none'}"
                >{keyRevealed ? exportLine : exportLine.split(connectKey).join(maskedKey)}</pre>
                <button
                  class="absolute top-2 right-2 inline-flex items-center gap-1 text-micro font-semibold
                         px-2 py-1 rounded-md bg-[var(--surface)] border border-[var(--border)]
                         {exportCopied ? 'text-[var(--success)]' : 'text-[var(--text-muted)] hover:text-[var(--text)]'} transition-colors"
                  onclick={copyExport}
                >
                  {#if exportCopied}<Check size={12} /> Copied{:else}<Copy size={12} /> Copy{/if}
                </button>
              </div>
              <p class="text-micro text-[var(--text-muted)] mt-1.5 leading-relaxed">{persistHint}</p>
            </section>
          {/if}

          <p class="text-micro text-[var(--text-muted)] text-center leading-relaxed">
            The key is shown only this once, so copy it now. You can reconnect any time to mint a fresh one.
          </p>
        {/if}
      </div>

      <div class="flex justify-end px-5 py-3.5 border-t border-[var(--border)]">
        <button
          class="text-body-sm font-medium text-[var(--btn-success-text)] bg-[var(--btn-success)] px-4 py-1.5 rounded-md hover:bg-[var(--btn-success-hover)] transition-colors"
          onclick={closeConnect}
        >
          Done
        </button>
      </div>
    </div>
  </div>
{/if}

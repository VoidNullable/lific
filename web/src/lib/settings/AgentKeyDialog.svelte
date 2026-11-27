<script lang="ts" module>
  import type { Bot } from "../api";

  /** What the dialog was opened for: a new agent account, or a fresh key
   *  for one that has none. */
  export type AgentDialogStart = { kind: "new" } | { kind: "rekey"; bot: Bot };
</script>

<script lang="ts">
  import { createBot, TOOL_TEMPLATES, type ToolTemplate } from "../api";
  import {
    RECENT_AUTH_ERROR,
    needsReauth,
    reauthenticateWithoutPassword,
    reauthenticateWithPassword,
    retryOnceAfterReauth,
  } from "../reauth";
  import { onMount } from "svelte";
  import ToolIcon from "../ToolIcon.svelte";
  import { copyToClipboard } from "../clipboard";
  import {
    accountId,
    accountIdProblem,
    cleanAccountId,
    genericTemplate,
    suggestAccountId,
    templateFor,
  } from "./agents";
  import { Check, Copy, X, AlertTriangle, Lock, Eye, EyeOff, KeyRound, FileCode2, Terminal, Bot as BotIcon } from "lucide-svelte";

  let {
    start,
    bots,
    userId,
    username,
    webAutoLogin,
    onclose,
    onchanged,
  }: {
    start: AgentDialogStart;
    bots: Bot[];
    userId: number;
    username: string;
    webAutoLogin: boolean;
    onclose: () => void;
    /** The account list changed (a bot or key was created). */
    onchanged: () => Promise<void> | void;
  } = $props();

  const OTHER = "other";
  const TOOLS = [...TOOL_TEMPLATES.map((t) => ({ id: t.id, name: t.name })), { id: OTHER, name: "Other MCP client" }];

  let taken = $derived(new Set(bots.map(accountId)));

  // ── Setup (new accounts only) ─────────────────────────
  let tool = $state(TOOL_TEMPLATES[0].id);
  let idTouched = $state(false);
  let nameTouched = $state(false);
  let draftId = $state("");
  let draftName = $state("");
  let setupTried = $state(false);

  let toolName = $derived(TOOLS.find((t) => t.id === tool)?.name ?? tool);
  let suggestedId = $derived(suggestAccountId(tool === OTHER ? "agent" : tool, taken));
  $effect(() => {
    if (!idTouched) draftId = suggestedId;
  });
  $effect(() => {
    if (!nameTouched) draftName = tool === OTHER ? "" : toolName;
  });
  let idProblem = $derived(accountIdProblem(draftId, taken));

  function onIdInput(e: Event) {
    const el = e.currentTarget as HTMLInputElement;
    const cleaned = cleanAccountId(el.value);
    if (el.value !== cleaned) el.value = cleaned;
    draftId = cleaned;
    idTouched = true;
  }

  // ── Minting ───────────────────────────────────────────
  type Phase = "setup" | "minting" | "reauth" | "key" | "error";
  let phase = $state<Phase>(start.kind === "new" ? "setup" : "minting");
  let template = $state<ToolTemplate | null>(null);
  let key = $state<string | null>(null);
  let botUsername = $state("");
  let error = $state("");
  let reauthPassword = $state("");
  let reauthBusy = $state(false);
  let autoReauthNote = $state("");

  onMount(() => {
    if (start.kind === "rekey") {
      mint(templateFor(accountId(start.bot), start.bot.display_name), start.bot.display_name);
    }
  });

  function createFromSetup() {
    setupTried = true;
    if (idProblem) return;
    const name = draftName.trim() || draftId;
    const base = tool === OTHER ? genericTemplate(draftId, name) : templateFor(tool);
    mint({ ...base, id: draftId, name }, name);
  }

  /** Mint the key; on a stale session, try the passwordless route once and
   *  otherwise ask for the password. */
  async function mint(t: ToolTemplate, name: string, recover = true) {
    template = t;
    phase = "minting";
    error = "";
    const res = await retryOnceAfterReauth(
      () => createBot(t.id, name),
      async () => {
        if (!recover || !webAutoLogin) {
          return { ok: false, error: RECENT_AUTH_ERROR, recoverable: recover };
        }
        const auto = await reauthenticateWithoutPassword(userId);
        if (!auto.ok) autoReauthNote = auto.error;
        return auto;
      },
    );
    if (res.ok) {
      key = res.data.key;
      botUsername = res.data.bot.username;
      phase = "key";
      await onchanged();
      return;
    }
    if (needsReauth(res)) {
      if (!recover) {
        error = "That still was not accepted. Sign out and sign back in, then try again.";
        phase = "error";
        return;
      }
      phase = "reauth";
      return;
    }
    error = res.error;
    phase = "error";
  }

  async function submitReauth() {
    if (reauthBusy || !reauthPassword || !template) return;
    reauthBusy = true;
    error = "";
    const refreshed = await reauthenticateWithPassword(reauthPassword, userId);
    reauthPassword = "";
    if (!refreshed.ok) {
      error = refreshed.error;
      reauthBusy = false;
      return;
    }
    autoReauthNote = "";
    reauthBusy = false;
    await mint(template, template.name, false);
  }

  function close() {
    if (phase === "minting" || reauthBusy) return;
    onclose();
  }

  // ── Key and config display ────────────────────────────
  let keyRevealed = $state(false);
  let copied = $state(false);
  let keyCopied = $state(false);
  let exportCopied = $state(false);
  let noteCopiedIdx = $state<number | null>(null);

  let maskedKey = $derived.by(() => {
    if (!key) return "";
    const m = key.match(/^(lific_sk-live-|lific_sk-test-|lific_sk-|sk-live-|sk-)/);
    const prefix = m ? m[0] : key.slice(0, 6);
    return prefix + "•".repeat(Math.min(Math.max(key.length - prefix.length, 0), 40));
  });
  let displayKey = $derived(keyRevealed ? (key ?? "") : maskedKey);

  type Os = "linux" | "mac" | "windows";
  const OS_LABELS: Record<Os, string> = { linux: "Linux", mac: "macOS", windows: "Windows" };
  function detectOs(): Os {
    const p = `${navigator.platform} ${navigator.userAgent}`.toLowerCase();
    if (p.includes("win")) return "windows";
    if (p.includes("mac")) return "mac";
    return "linux";
  }
  let selectedOs = $state<Os>(detectOs());
  $effect(() => {
    // Open on an OS the tool supports (Claude Desktop has no Linux build).
    const t = template;
    if (!t) return;
    const detected = detectOs();
    selectedOs = t.configPath[detected] ? detected : ((["linux", "mac", "windows"] as Os[]).find((o) => t.configPath[o]) ?? "mac");
  });

  type OsGroup = { oses: Os[]; label: string };
  let osGroups = $derived.by<OsGroup[]>(() => {
    if (!template) return [];
    const groups: OsGroup[] = [];
    for (const os of ["linux", "mac", "windows"] as Os[]) {
      const path = template.configPath[os];
      if (!path) continue;
      const existing = groups.find((g) => template!.configPath[g.oses[0]] === path);
      if (existing) existing.oses.push(os);
      else groups.push({ oses: [os], label: "" });
    }
    return groups.map((g) => ({ oses: g.oses, label: g.oses.map((o) => OS_LABELS[o]).join(" / ") }));
  });
  let pathsDiffer = $derived(osGroups.length > 1);
  let activePath = $derived(template ? template.configPath[selectedOs] : "");

  let exportLine = $derived.by(() => {
    if (!template?.usesEnvKey || !key) return "";
    const v = template.envVar ?? "LIFIC_API_KEY";
    return selectedOs === "windows" ? `setx ${v} "${key}"` : `export ${v}="${key}"`;
  });
  let persistHint = $derived.by(() => {
    switch (selectedOs) {
      case "mac":
        return "Runs in the current shell. To keep it, add the line to ~/.zshrc (the default macOS shell).";
      case "linux":
        return "Runs in the current shell. To keep it, add the line to ~/.bashrc or ~/.profile.";
      case "windows":
        return "setx applies to new terminals, not the current one. Reopen your terminal after running it.";
    }
  });

  function configText(): string {
    if (!template || !key) return "";
    return template.generateConfig(window.location.origin + "/mcp", key);
  }
  let displayConfig = $derived.by(() => {
    const text = configText();
    if (keyRevealed || !key) return text;
    return text.split(key).join(maskedKey);
  });

  async function copyWithFlag(text: string, flag: (v: boolean) => void) {
    const ok = await copyToClipboard(text, { silentSuccess: true });
    if (ok) {
      flag(true);
      window.setTimeout(() => flag(false), 1500);
    }
  }
  const copyConfig = () => copyWithFlag(configText(), (v) => (copied = v));
  const copyKey = () => key && copyWithFlag(key, (v) => (keyCopied = v));
  const copyExport = () => copyWithFlag(exportLine, (v) => (exportCopied = v));
  const copyNote = (idx: number, command: string) => copyWithFlag(command, (v) => (noteCopiedIdx = v ? idx : null));

  let title = $derived(
    phase === "setup"
      ? "New agent account"
      : start.kind === "rekey"
        ? `New key for ${start.bot.display_name}`
        : `Set up ${template?.name ?? ""}`,
  );

  const fieldClass =
    "w-full text-body-sm rounded-md border border-[var(--border)] bg-[var(--bg)] text-[var(--text)] " +
    "px-2.5 py-1.5 outline-none transition-colors placeholder:text-[var(--text-faint)] " +
    "focus:border-[var(--accent)] focus-visible:ring-2 focus-visible:ring-[var(--accent-subtle)]";
</script>

<svelte:window onkeydown={(e) => { if (e.key === "Escape") close(); }} />

<!-- svelte-ignore a11y_click_events_have_key_events a11y_no_static_element_interactions -->
<div class="fixed inset-0 z-50 bg-black/50 grid place-items-center p-4" role="presentation" onclick={close}>
  <!-- svelte-ignore a11y_click_events_have_key_events a11y_no_static_element_interactions -->
  <div
    class="w-full max-w-[620px] bg-[var(--surface)] border border-[var(--border)] rounded-xl shadow-2xl max-h-[88dvh] overflow-y-auto"
    role="dialog"
    aria-modal="true"
    aria-label={title}
    tabindex="-1"
    onclick={(e) => e.stopPropagation()}
  >
    <div class="flex items-center gap-3 px-5 py-4 border-b border-[var(--border)]">
      <div class="size-9 shrink-0 rounded-lg bg-[var(--bg-subtle)] grid place-items-center text-[var(--text)]">
        {#if phase === "setup"}
          <ToolIcon tool={tool === OTHER ? null : tool} size={18} fallback={BotIcon} />
        {:else}
          <ToolIcon tool={template?.id ?? null} size={18} fallback={BotIcon} />
        {/if}
      </div>
      <div class="flex-1 min-w-0">
        <h3 class="text-body-lg font-semibold text-[var(--text)] leading-tight truncate">{title}</h3>
        {#if phase === "key" && botUsername}
          <p class="text-caption font-mono text-[var(--text-muted)] truncate">@{botUsername}</p>
        {:else if phase === "setup"}
          <p class="text-caption text-[var(--text-muted)]">A Lific account for one agent, with its own API key.</p>
        {/if}
      </div>
      <button
        class="size-7 grid place-items-center rounded-md text-[var(--text-muted)] hover:text-[var(--text)] hover:bg-[var(--bg-subtle)] transition-colors disabled:opacity-50"
        onclick={close}
        disabled={phase === "minting" || reauthBusy}
        aria-label="Close"
      >
        <X size={16} />
      </button>
    </div>

    <div class="p-5">
      {#if phase === "setup"}
        <form class="flex flex-col gap-5" onsubmit={(e) => { e.preventDefault(); createFromSetup(); }}>
          <fieldset>
            <legend class="text-body-sm font-medium text-[var(--text)] mb-1">Which tool will use it?</legend>
            <p class="text-caption text-[var(--text-muted)] mb-2.5">This only picks the setup steps shown next.</p>
            <div class="grid grid-cols-2 sm:grid-cols-3 gap-1.5" role="radiogroup" aria-label="Tool">
              {#each TOOLS as t (t.id)}
                {@const selected = tool === t.id}
                <button
                  type="button"
                  role="radio"
                  aria-checked={selected}
                  class="flex items-center gap-2.5 rounded-lg px-2.5 py-2 text-left text-body-sm border transition-colors
                         focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-[var(--accent)]
                         {selected
                    ? 'border-[var(--accent)] bg-[var(--accent-subtle)] text-[var(--text)]'
                    : 'border-[var(--border)] text-[var(--text-muted)] hover:text-[var(--text)] hover:bg-[var(--bg-subtle)]'}"
                  onclick={() => (tool = t.id)}
                >
                  <span class="size-5 shrink-0 grid place-items-center {selected ? 'text-[var(--accent)]' : ''}">
                    <ToolIcon tool={t.id === OTHER ? null : t.id} size={16} fallback={BotIcon} />
                  </span>
                  <span class="truncate">{t.name}</span>
                </button>
              {/each}
            </div>
          </fieldset>

          <div class="grid sm:grid-cols-2 gap-4">
            <label class="flex flex-col gap-1.5">
              <span class="text-body-sm font-medium text-[var(--text)]">Name</span>
              <input
                class={fieldClass}
                bind:value={draftName}
                oninput={() => (nameTouched = true)}
                maxlength="80"
                placeholder={tool === OTHER ? "My agent" : toolName}
              />
            </label>
            <label class="flex flex-col gap-1.5">
              <span class="text-body-sm font-medium text-[var(--text)]">Account ID</span>
              <input
                class="{fieldClass} font-mono"
                value={draftId}
                oninput={onIdInput}
                maxlength="48"
                spellcheck="false"
                autocomplete="off"
                aria-invalid={setupTried && idProblem ? "true" : undefined}
                aria-describedby="agent-id-note"
              />
            </label>
          </div>
          <p id="agent-id-note" class="-mt-2 text-caption leading-relaxed" aria-live="polite">
            {#if setupTried && idProblem}
              <span class="text-[var(--error)]">{idProblem}</span>
            {:else}
              <span class="text-[var(--text-muted)]">
                Its username will be <span class="font-mono text-[var(--text)]">@{draftId || "…"}-{username}</span>.
                Give each machine or agent its own account so their work stays apart.
              </span>
            {/if}
          </p>

          <div class="flex items-center justify-end gap-2 pt-1">
            <button type="button" class="text-body-sm text-[var(--text-muted)] px-3 py-1.5 rounded-md hover:bg-[var(--bg-subtle)] transition-colors" onclick={close}>
              Cancel
            </button>
            <button
              type="submit"
              class="text-body-sm font-medium text-[var(--accent-text)] bg-[var(--accent)] px-3.5 py-1.5 rounded-md hover:bg-[var(--accent-hover)] transition-colors"
            >
              Create account and key
            </button>
          </div>
        </form>
      {:else if phase === "minting"}
        <div class="flex items-center gap-3 py-8 justify-center text-[var(--text-muted)]">
          <div class="size-5 rounded-full border-2 border-[var(--border)] border-t-[var(--accent)] animate-spin"></div>
          <span class="text-body">Creating the key…</span>
        </div>
      {:else if phase === "reauth"}
        <div class="flex flex-col gap-3">
          <div class="flex items-start gap-2.5 text-body text-[var(--text)] bg-[var(--bg-subtle)] px-3.5 py-3 rounded-md">
            <Lock size={16} class="shrink-0 mt-0.5 text-[var(--text-muted)]" />
            <p class="leading-relaxed">
              A key works until you revoke it, so Lific asks for your password when you signed in
              more than 15 minutes ago.
            </p>
          </div>
          {#if autoReauthNote}
            <p class="text-caption text-[var(--text-muted)]" role="status">
              Signing you in again automatically did not work ({autoReauthNote}).
            </p>
          {/if}
          {#if error}
            <p class="text-caption text-[var(--error)] flex items-center gap-1" role="alert"><AlertTriangle size={12} /> {error}</p>
          {/if}
          <label class="block">
            <span class="block text-body-sm font-medium text-[var(--text)] mb-1.5">Current password</span>
            <input
              type="password"
              bind:value={reauthPassword}
              autocomplete="current-password"
              disabled={reauthBusy}
              class="{fieldClass} disabled:opacity-50"
              onkeydown={(e) => { if (e.key === "Enter") submitReauth(); }}
            />
          </label>
          <div class="flex items-center gap-2">
            <button
              class="text-body-sm font-medium text-[var(--accent-text)] bg-[var(--accent)] px-3 py-1.5 rounded-md hover:bg-[var(--accent-hover)] transition-colors disabled:opacity-40 disabled:cursor-not-allowed"
              disabled={reauthBusy || !reauthPassword}
              onclick={submitReauth}
            >
              {reauthBusy ? "Checking…" : "Confirm and create the key"}
            </button>
            <button class="text-body-sm text-[var(--text-muted)] px-3 py-1.5 rounded-md hover:bg-[var(--bg-subtle)] transition-colors disabled:opacity-50" disabled={reauthBusy} onclick={close}>
              Cancel
            </button>
          </div>
        </div>
      {:else if phase === "error"}
        <div class="flex items-start gap-2.5 text-body text-[var(--error)] bg-[var(--error-bg)] px-3.5 py-3 rounded-md" role="alert">
          <AlertTriangle size={16} class="shrink-0 mt-0.5" />
          <span>{error}</span>
        </div>
      {:else if phase === "key" && key && template}
        <section class="mb-5">
          <div class="flex items-center gap-2.5 mb-2">
            <span class="size-5 shrink-0 grid place-items-center rounded-full bg-[var(--accent)] text-[var(--accent-text)] text-micro font-bold tabular-nums">1</span>
            <h4 class="flex items-center gap-1.5 text-body font-semibold text-[var(--text)]">
              <KeyRound size={14} class="text-[var(--text-muted)]" /> Copy the API key
            </h4>
          </div>
          <div class="rounded-lg border border-[var(--border)] bg-[var(--bg)] overflow-hidden">
            <div class="flex items-center gap-2 px-3 py-2.5">
              <code class="flex-1 min-w-0 text-body-sm font-mono text-[var(--text)] overflow-x-auto whitespace-nowrap leading-none py-0.5 {keyRevealed ? '' : 'select-none cursor-default'}">{displayKey}</code>
              <button
                class="shrink-0 size-7 grid place-items-center rounded-md text-[var(--text-muted)] hover:text-[var(--text)] hover:bg-[var(--bg-subtle)] transition-colors"
                onclick={() => (keyRevealed = !keyRevealed)}
                aria-label={keyRevealed ? "Hide key" : "Reveal key"}
                title={keyRevealed ? "Hide key" : "Reveal key"}
              >
                {#if keyRevealed}<EyeOff size={15} />{:else}<Eye size={15} />{/if}
              </button>
            </div>
            <button
              class="w-full flex items-center justify-center gap-1.5 py-2 text-body-sm font-semibold border-t border-[var(--border)] transition-colors
                     {keyCopied ? 'bg-[var(--success-bg)] text-[var(--success)]' : 'bg-[var(--accent)] text-[var(--accent-text)] hover:bg-[var(--accent-hover)]'}"
              onclick={copyKey}
            >
              {#if keyCopied}<Check size={14} /> Copied to clipboard{:else}<Copy size={14} /> Copy key{/if}
            </button>
          </div>
          <p class="text-caption text-[var(--text-muted)] mt-2 leading-relaxed">
            Lific keeps only a hash of the key and can't show it again. If it gets lost, revoke it
            and issue a new one.
          </p>
        </section>

        <section class="mb-5">
          <div class="flex items-center gap-2.5 mb-2">
            <span class="size-5 shrink-0 grid place-items-center rounded-full bg-[var(--accent)] text-[var(--accent-text)] text-micro font-bold tabular-nums">2</span>
            <h4 class="flex items-center gap-1.5 text-body font-semibold text-[var(--text)]">
              <FileCode2 size={14} class="text-[var(--text-muted)]" /> Add it to {template.name}'s config
            </h4>
          </div>

          {#if osGroups.length > 0}
            <div class="flex items-center gap-2 mb-2.5">
              <div class="inline-flex flex-1 p-0.5 rounded-lg bg-[var(--bg)] shadow-[inset_0_1px_2px_rgba(0,0,0,0.08)]">
                {#each osGroups as group (group.label)}
                  {@const active = group.oses.includes(selectedOs)}
                  <button
                    class="flex-1 px-2 py-1.5 rounded-md text-caption font-medium transition-all
                           {active ? 'bg-[var(--surface)] text-[var(--text)] shadow-[0_1px_2px_rgba(0,0,0,0.12)]' : 'text-[var(--text-muted)] hover:text-[var(--text)]'}"
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
          {/if}

          {#if template.configNote}
            <div class="flex flex-col gap-1.5 mb-2.5">
              {#each template.configNote as step, i (i)}
                {#if step.text}<p class="text-body-sm text-[var(--text)] leading-snug">{step.text}</p>{/if}
                {#if step.command}
                  <div class="rounded-lg bg-[var(--bg)] overflow-hidden ring-1 ring-[var(--border)]">
                    <pre class="px-3 py-2.5 text-caption font-mono text-[var(--text)] overflow-x-auto whitespace-pre">{step.command}</pre>
                    <button
                      class="w-full flex items-center justify-center gap-1.5 py-2 text-caption font-semibold border-t border-[var(--border)] transition-colors
                             {noteCopiedIdx === i ? 'bg-[var(--success-bg)] text-[var(--success)]' : 'text-[var(--text-muted)] hover:text-[var(--text)] hover:bg-[var(--bg-subtle)]'}"
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
            <pre class="bg-[var(--bg)] border border-[var(--border)] rounded-lg p-3.5 pr-12 text-caption font-mono text-[var(--text)] overflow-x-auto leading-relaxed {keyRevealed ? '' : 'select-none'}">{displayConfig}</pre>
            <button
              class="absolute top-2 right-2 inline-flex items-center gap-1 text-micro font-semibold px-2 py-1 rounded-md bg-[var(--surface)] border border-[var(--border)]
                     {copied ? 'text-[var(--success)]' : 'text-[var(--text-muted)] hover:text-[var(--text)]'} transition-colors"
              onclick={copyConfig}
            >
              {#if copied}<Check size={12} /> Copied{:else}<Copy size={12} /> Copy{/if}
            </button>
          </div>
        </section>

        {#if template.usesEnvKey}
          <section class="mb-1">
            <div class="flex items-center gap-2.5 mb-2">
              <span class="size-5 shrink-0 grid place-items-center rounded-full bg-[var(--accent)] text-[var(--accent-text)] text-micro font-bold tabular-nums">3</span>
              <h4 class="flex items-center gap-1.5 text-body font-semibold text-[var(--text)]">
                <Terminal size={14} class="text-[var(--text-muted)]" /> Set the environment variable
                <span class="font-normal text-[var(--text-muted)]">· {OS_LABELS[selectedOs]}</span>
              </h4>
            </div>
            <div class="relative">
              <pre class="bg-[var(--bg)] border border-[var(--border)] rounded-lg p-3 pr-12 text-caption font-mono text-[var(--text)] overflow-x-auto {keyRevealed ? '' : 'select-none'}">{keyRevealed ? exportLine : exportLine.split(key).join(maskedKey)}</pre>
              <button
                class="absolute top-2 right-2 inline-flex items-center gap-1 text-micro font-semibold px-2 py-1 rounded-md bg-[var(--surface)] border border-[var(--border)]
                       {exportCopied ? 'text-[var(--success)]' : 'text-[var(--text-muted)] hover:text-[var(--text)]'} transition-colors"
                onclick={copyExport}
              >
                {#if exportCopied}<Check size={12} /> Copied{:else}<Copy size={12} /> Copy{/if}
              </button>
            </div>
            <p class="text-micro text-[var(--text-muted)] mt-1.5 leading-relaxed">{persistHint}</p>
          </section>
        {/if}
      {/if}
    </div>

    {#if phase === "key" || phase === "error"}
      <div class="flex justify-end px-5 py-3.5 border-t border-[var(--border)]">
        <button
          class="text-body-sm font-medium text-[var(--accent-text)] bg-[var(--accent)] px-4 py-1.5 rounded-md hover:bg-[var(--accent-hover)] transition-colors"
          onclick={close}
        >
          Done
        </button>
      </div>
    {/if}
  </div>
</div>

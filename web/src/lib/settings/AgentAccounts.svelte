<script lang="ts">
  import { disconnectBot, deleteBot, type Bot } from "../api";
  import ToolIcon from "../ToolIcon.svelte";
  import { formatRelative, formatDate } from "../format";
  import AgentKeyDialog, { type AgentDialogStart } from "./AgentKeyDialog.svelte";
  import { accountId, agentStatus } from "./agents";
  import { Plus, ArrowRight, AlertTriangle, Bot as BotIcon, KeyRound } from "lucide-svelte";

  let {
    bots,
    userId,
    username,
    webAutoLogin,
    reload,
    navigate,
  }: {
    bots: Bot[];
    userId: number;
    username: string;
    webAutoLogin: boolean;
    reload: () => Promise<void>;
    navigate: (path: string) => void;
  } = $props();

  let dialog = $state<AgentDialogStart | null>(null);
  let confirming = $state<{ id: number; action: "revoke" | "delete" } | null>(null);
  let busyId = $state<number | null>(null);
  let error = $state("");

  async function revoke(bot: Bot) {
    busyId = bot.id;
    error = "";
    const res = await disconnectBot(bot.id);
    if (!res.ok) error = res.error;
    confirming = null;
    await reload();
    busyId = null;
  }

  async function remove(bot: Bot) {
    busyId = bot.id;
    error = "";
    const res = await deleteBot(bot.id);
    if (!res.ok) error = res.error;
    confirming = null;
    await reload();
    busyId = null;
  }

  function plural(n: number, word: string) {
    return `${n.toLocaleString()} ${word}${n === 1 ? "" : "s"}`;
  }

  const quiet =
    "text-body-sm text-[var(--text-muted)] px-2.5 py-1.5 rounded-md hover:text-[var(--text)] hover:bg-[var(--bg-subtle)] transition-colors disabled:opacity-50";
</script>

<div class="flex flex-col sm:flex-row sm:items-end justify-between gap-x-6 gap-y-3 mb-3">
  <div class="flex-1 min-w-0 max-w-[62ch]">
    <h2 id="settings-agents-heading" class="text-[1.15rem] font-semibold tracking-tight text-[var(--text)]">Agents</h2>
    <p class="text-body-sm text-[var(--text-muted)] mt-0.5 leading-relaxed">
      Each agent works in Lific through its own account and API key. What it does is recorded
      under that account, so you can see which tool did what, and cut one off without touching
      the others.
    </p>
  </div>
  <button
    class="self-start sm:self-auto shrink-0 inline-flex items-center gap-1.5 text-body-sm font-medium text-[var(--accent-text)] bg-[var(--accent)] px-3 py-1.5 rounded-md hover:bg-[var(--accent-hover)] transition-colors"
    onclick={() => (dialog = { kind: "new" })}
  >
    <Plus size={14} /> New agent account
  </button>
</div>

<div class="rounded-xl bg-[var(--surface)] shadow-[0_1px_2px_rgba(0,0,0,0.06)]">
  {#if bots.length === 0}
    <div class="flex items-start gap-4 px-5 py-6">
      <span class="size-10 shrink-0 grid place-items-center rounded-lg bg-[var(--bg-subtle)] text-[var(--text-muted)]">
        <BotIcon size={18} />
      </span>
      <div class="min-w-0 max-w-[58ch]">
        <p class="text-body font-medium text-[var(--text)]">No agent accounts yet</p>
        <p class="text-body-sm text-[var(--text-muted)] mt-1 leading-relaxed">
          Create one for each tool that should work in Lific and paste its key into the tool's MCP
          settings. Running <code class="font-mono text-caption px-1 py-0.5 rounded bg-[var(--bg-subtle)] text-[var(--text)]">lific connect</code>
          on the agent's machine does the same from a terminal, and a client that signs in through
          OAuth gets an account when you approve it.
        </p>
      </div>
    </div>
  {:else}
    {#each bots as bot (bot.id)}
      {@const id = accountId(bot)}
      {@const status = agentStatus(bot)}
      {@const isConfirming = confirming?.id === bot.id ? confirming.action : null}
      <div data-connection-id={id} class="px-4 py-3.5 border-t border-[var(--border)] first:border-t-0">
        <div class="flex flex-wrap items-center gap-x-4 gap-y-2.5">
          <div class="size-10 shrink-0 rounded-lg bg-[var(--bg-subtle)] grid place-items-center {status === 'none' ? 'text-[var(--text-faint)]' : 'text-[var(--text)]'}">
            <ToolIcon tool={id} size={20} fallback={BotIcon} />
          </div>
          <div class="flex-1 min-w-[12rem]">
            <div class="flex flex-wrap items-center gap-x-2 gap-y-1">
              <span class="text-body font-medium text-[var(--text)] break-all">{bot.display_name}</span>
              {#if status === "key"}
                <span class="inline-flex items-center gap-1 text-caption text-[var(--success)] bg-[var(--success-bg)] px-1.5 py-px rounded-full">
                  <span class="size-1.5 rounded-full bg-[var(--success)]"></span> Key active
                </span>
              {:else if status === "oauth"}
                <span class="inline-flex items-center gap-1 text-caption text-[var(--success)] bg-[var(--success-bg)] px-1.5 py-px rounded-full">
                  <span class="size-1.5 rounded-full bg-[var(--success)]"></span> Signed in with OAuth
                </span>
              {:else}
                <span class="text-caption text-[var(--text-muted)] bg-[var(--bg-subtle)] px-1.5 py-px rounded-full">No key</span>
              {/if}
            </div>
            <p class="text-caption text-[var(--text-muted)] mt-0.5 flex flex-wrap gap-x-1.5">
              <span class="font-mono">@{bot.username}</span>
              {#if bot.key_created_at && status === "key"}
                <span aria-hidden="true">·</span>
                <span title={formatDate(bot.key_created_at)}>key issued {formatRelative(bot.key_created_at)}</span>
              {/if}
              <span aria-hidden="true">·</span>
              {#if bot.last_active}
                <span title={formatDate(bot.last_active)}>last active {formatRelative(bot.last_active)}</span>
              {:else}
                <span>not used yet</span>
              {/if}
              {#if bot.changes}
                <span aria-hidden="true">·</span>
                <span>{plural(bot.changes, "change")}</span>
              {/if}
            </p>
          </div>
          <div class="shrink-0 flex items-center gap-1 ml-auto">
            {#if status !== "none"}
              <button
                class="{quiet} hover:!text-[var(--error)]"
                disabled={busyId === bot.id}
                aria-expanded={isConfirming === "revoke"}
                onclick={() => (confirming = isConfirming === "revoke" ? null : { id: bot.id, action: "revoke" })}
              >
                {status === "oauth" ? "Revoke access" : "Revoke key"}
              </button>
            {:else}
              <button
                class="{quiet} hover:!text-[var(--error)]"
                disabled={busyId === bot.id}
                aria-expanded={isConfirming === "delete"}
                onclick={() => (confirming = isConfirming === "delete" ? null : { id: bot.id, action: "delete" })}
              >
                Delete
              </button>
              <button
                class="inline-flex items-center gap-1.5 text-body-sm font-medium text-[var(--text)] border border-[var(--border)] px-2.5 py-1.5 rounded-md hover:bg-[var(--bg-subtle)] transition-colors"
                onclick={() => (dialog = { kind: "rekey", bot })}
              >
                <KeyRound size={13} /> New key
              </button>
            {/if}
          </div>
        </div>

        {#if isConfirming}
          <div class="mt-3 sm:ml-14 flex flex-col gap-2.5 rounded-lg bg-[var(--error-bg)] px-3.5 py-3" role="alert">
            <p class="text-body-sm text-[var(--text)] leading-relaxed">
              {#if isConfirming === "revoke"}
                {bot.display_name} stops working in Lific until you give it a new key. The account and
                everything it did stay.
              {:else}
                This deletes <span class="font-mono">@{bot.username}</span>{#if bot.comments}
                  and the {plural(bot.comments, "comment")} it wrote{/if}. It can't be undone.
              {/if}
            </p>
            <div class="flex items-center gap-2">
              <button
                class="text-body-sm font-medium text-[var(--error-text)] bg-[var(--error)] px-3 py-1.5 rounded-md hover:opacity-90 transition-opacity disabled:opacity-50"
                disabled={busyId === bot.id}
                onclick={() => (isConfirming === "revoke" ? revoke(bot) : remove(bot))}
              >
                {#if busyId === bot.id}Working…{:else if isConfirming === "revoke"}Yes, revoke{:else}Delete account{/if}
              </button>
              <button class={quiet} disabled={busyId === bot.id} onclick={() => (confirming = null)}>Cancel</button>
            </div>
          </div>
        {/if}
      </div>
    {/each}
  {/if}
</div>

{#if error}
  <p role="alert" class="mt-3 text-body-sm text-[var(--error)] flex items-center gap-1.5"><AlertTriangle size={13} /> {error}</p>
{/if}

{#if bots.length > 0}
  <button
    class="mt-3 inline-flex items-center gap-1 text-body-sm text-[var(--text-muted)] hover:text-[var(--accent)] rounded transition-colors"
    onclick={() => navigate("/agents")}
  >
    What they've been doing, in Agent activity <ArrowRight size={13} aria-hidden="true" />
  </button>
{/if}

{#if dialog}
  <AgentKeyDialog
    start={dialog}
    {bots}
    {userId}
    {username}
    {webAutoLogin}
    onclose={() => (dialog = null)}
    onchanged={reload}
  />
{/if}

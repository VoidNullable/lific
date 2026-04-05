<script lang="ts">
  import {
    me,
    logout,
    clearSession,
    listBots,
    createBot,
    disconnectBot,
    deleteBot,
    TOOL_TEMPLATES,
    type AuthUser,
    type Bot,
    type ToolTemplate,
  } from "../lib/api";
  import ThemeToggle from "../lib/ThemeToggle.svelte";

  let { navigate }: { navigate: (path: string) => void } = $props();

  let user = $state<AuthUser | null>(null);
  let bots = $state<Bot[]>([]);
  let loading = $state(true);

  // Connection flow
  let connecting = $state(false);
  let selectedTool = $state<ToolTemplate | null>(null);
  let createdKey = $state<string | null>(null);
  let connectError = $state("");

  // Disconnect / Delete
  let disconnectingId = $state<number | null>(null);
  let deletingId = $state<number | null>(null);

  $effect(() => {
    loadUser();
  });

  async function loadUser() {
    const result = await me();
    if (result.ok) {
      user = result.data;
      await loadBots();
    } else {
      clearSession();
      navigate("/login");
    }
    loading = false;
  }

  async function loadBots() {
    const result = await listBots();
    if (result.ok) bots = result.data;
  }

  async function handleLogout() {
    await logout();
    clearSession();
    navigate("/login");
  }

  function startConnect(template: ToolTemplate) {
    selectedTool = template;
    createdKey = null;
    connectError = "";
  }

  function cancelConnect() {
    selectedTool = null;
    createdKey = null;
    connectError = "";
  }

  async function confirmConnect() {
    if (!selectedTool) return;
    connecting = true;
    connectError = "";

    const result = await createBot(selectedTool.id);
    if (result.ok) {
      createdKey = result.data.key;
      await loadBots();
    } else {
      connectError = result.error;
    }
    connecting = false;
  }

  async function handleDisconnect(id: number) {
    disconnectingId = id;
    await disconnectBot(id);
    await loadBots();
    disconnectingId = null;
  }

  async function handleDelete(id: number) {
    deletingId = id;
    await deleteBot(id);
    await loadBots();
    deletingId = null;
  }

  function initials(name: string): string {
    return name
      .split(/[\s_-]+/)
      .slice(0, 2)
      .map((w) => w[0]?.toUpperCase() ?? "")
      .join("");
  }

  function formatDate(iso: string): string {
    const d = new Date(iso + "Z");
    return d.toLocaleDateString("en-US", {
      month: "short",
      day: "numeric",
      year: "numeric",
    });
  }

  function getToolBot(toolId: string): Bot | undefined {
    return bots.find((b) => b.username.startsWith(toolId + "-"));
  }

  function isToolConnected(toolId: string): boolean {
    const bot = getToolBot(toolId);
    return !!bot && bot.has_active_key;
  }

  function isToolDisconnected(toolId: string): boolean {
    const bot = getToolBot(toolId);
    return !!bot && !bot.has_active_key;
  }
</script>

<div class="min-h-dvh flex flex-col">
  {#if loading}
    <div class="flex-1 flex items-center justify-center">
      <div
        class="size-6 rounded-full border-2 border-[var(--border)]
               border-t-[var(--accent)] animate-spin"
      ></div>
    </div>
  {:else if user}
    <header
      class="flex items-center justify-between px-6 py-3
             border-b border-[var(--border)]"
    >
      <span class="font-display text-lg tracking-tight">Lific</span>
      <div class="flex items-center gap-3">
        <ThemeToggle />
        <div
          class="size-8 rounded-full bg-[var(--accent)] text-[var(--accent-text)]
                 flex items-center justify-center text-xs font-semibold
                 tracking-wide select-none"
          title={user.username}
        >
          {initials(user.display_name || user.username)}
        </div>
        <button
          class="text-[0.8125rem] text-[var(--text-muted)] px-2 py-1
                 rounded transition-colors
                 hover:text-[var(--text)] hover:bg-[var(--bg-subtle)]"
          onclick={handleLogout}
        >
          Sign out
        </button>
      </div>
    </header>

    <main class="flex-1 flex justify-center px-6 py-10 md:py-16">
      <div class="w-full max-w-[600px]">

        <!-- User info -->
        <section class="mb-10 animate-reveal delay-100">
          <div class="mb-6">
            <span
              class="block text-[0.8125rem] font-medium uppercase
                     tracking-widest text-[var(--text-muted)] mb-1"
            >
              Signed in as
            </span>
            <h1
              class="font-display text-[clamp(1.75rem,4vw,2.25rem)]
                     tracking-tight text-[var(--text)]"
            >
              {user.display_name || user.username}
            </h1>
          </div>

          <div
            class="grid grid-cols-3 max-sm:grid-cols-1
                   gap-px bg-[var(--border)] border border-[var(--border)]
                   rounded-md overflow-hidden"
          >
            {#each [
              { label: "Username", value: user.username },
              { label: "Email", value: user.email },
              { label: "Role", value: user.is_admin ? "Admin" : "Member" },
            ] as item}
              <div class="bg-[var(--surface)] p-4">
                <span
                  class="block text-[0.6875rem] font-medium uppercase
                         tracking-widest text-[var(--text-faint)] mb-1"
                >
                  {item.label}
                </span>
                <span class="text-[0.9375rem] font-medium text-[var(--text)]">
                  {item.value}
                </span>
              </div>
            {/each}
          </div>
        </section>

        <!-- Connected Tools -->
        <section class="animate-reveal delay-250">
          <div class="mb-6">
            <h2 class="font-display text-[1.375rem] text-[var(--text)] mb-1">
              Connected Tools
            </h2>
            <p class="text-[0.875rem] text-[var(--text-muted)]">
              Link your AI coding tools to Lific. Each connection creates a bot identity that acts on your behalf.
            </p>
          </div>

          <!-- Connection flow overlay -->
          {#if selectedTool}
            <div
              class="border border-[var(--border)] rounded-md p-5 mb-6
                     bg-[var(--surface)]"
            >
              {#if createdKey}
                <!-- Success: show config snippet -->
                <div class="mb-4">
                  <h3 class="text-[0.9375rem] font-semibold text-[var(--text)] mb-1">
                    {selectedTool.name} connected
                  </h3>
                  <p class="text-[0.8125rem] text-[var(--text-muted)]">
                    Add this to <code class="font-mono text-[0.8125rem] bg-[var(--bg-subtle)] px-1.5 py-0.5 rounded">{selectedTool.configPath}</code>
                  </p>
                  {#if selectedTool.configNote}
                    <p class="text-[0.75rem] text-[var(--text-faint)] mt-1">
                      {selectedTool.configNote}
                    </p>
                  {/if}
                </div>

                <div class="relative">
                  <pre
                    class="bg-[var(--bg)] border border-[var(--border)] rounded-md
                           p-4 text-[0.8125rem] font-mono text-[var(--text)]
                           overflow-x-auto leading-relaxed"
                  >{selectedTool.generateConfig(window.location.origin + "/mcp", createdKey)}</pre>
                  <button
                    class="absolute top-2 right-2 text-[0.6875rem] font-semibold
                           uppercase tracking-wider text-[var(--accent)] px-2 py-1
                           rounded bg-[var(--surface)] border border-[var(--border)]
                           hover:bg-[var(--accent-subtle)] transition-colors"
                    onclick={() =>
                      navigator.clipboard.writeText(
                        selectedTool!.generateConfig(
                          window.location.origin + "/mcp",
                          createdKey!
                        )
                      )
                    }
                  >
                    Copy
                  </button>
                </div>

                <div
                  class="mt-4 text-[0.8125rem] text-[var(--success)] bg-[var(--success-bg)]
                         px-3 py-2 rounded-md border-l-[3px] border-[var(--success)]"
                >
                  Save this configuration now. The API key won't be shown again.
                </div>

                <button
                  class="mt-4 text-[0.8125rem] text-[var(--text-muted)]
                         hover:text-[var(--text)] transition-colors"
                  onclick={cancelConnect}
                >
                  Done
                </button>
              {:else}
                <!-- Confirm connection -->
                <h3 class="text-[0.9375rem] font-semibold text-[var(--text)] mb-1">
                  Connect {selectedTool.name}
                </h3>
                <p class="text-[0.8125rem] text-[var(--text-muted)] mb-4">
                  This will create a bot account for {selectedTool.name} that can manage issues, comments, and pages on your behalf.
                </p>

                {#if connectError}
                  <div
                    class="text-sm text-[var(--error)] bg-[var(--error-bg)]
                           px-4 py-2 rounded-md border-l-[3px] border-[var(--error)] mb-4"
                    role="alert"
                  >
                    {connectError}
                  </div>
                {/if}

                <div class="flex gap-3">
                  <button
                    class="rounded-md bg-[var(--accent)] text-[var(--accent-text)]
                           text-[0.875rem] font-medium px-4 py-2
                           transition-all duration-200
                           hover:bg-[var(--accent-hover)] active:scale-[0.98]
                           disabled:opacity-60 disabled:cursor-not-allowed"
                    disabled={connecting}
                    onclick={confirmConnect}
                  >
                    {connecting ? "Connecting..." : "Connect"}
                  </button>
                  <button
                    class="rounded-md text-[0.875rem] text-[var(--text-muted)]
                           px-4 py-2 hover:bg-[var(--bg-subtle)] transition-colors"
                    onclick={cancelConnect}
                  >
                    Cancel
                  </button>
                </div>
              {/if}
            </div>
          {/if}

          <!-- Tool grid -->
          <div class="grid grid-cols-2 max-sm:grid-cols-1 gap-3 mb-6">
            {#each TOOL_TEMPLATES as template (template.id)}
              {@const connected = isToolConnected(template.id)}
              {@const disconnected = isToolDisconnected(template.id)}
              <button
                class="text-left p-4 rounded-md border transition-all duration-200
                       {connected
                  ? 'border-[var(--success)] bg-[var(--success-bg)]'
                  : disconnected
                    ? 'border-[var(--border)] bg-[var(--surface)] hover:border-[var(--accent)] hover:shadow-sm border-dashed'
                    : 'border-[var(--border)] bg-[var(--surface)] hover:border-[var(--accent)] hover:shadow-sm'}
                       disabled:opacity-50"
                disabled={connected || !!selectedTool}
                onclick={() => startConnect(template)}
              >
                <div class="flex items-center justify-between mb-1">
                  <span class="text-[0.9375rem] font-medium text-[var(--text)]">
                    {template.name}
                  </span>
                  {#if connected}
                    <span
                      class="text-[0.6875rem] font-semibold uppercase tracking-wide
                             text-[var(--success)]"
                    >
                      Connected
                    </span>
                  {:else if disconnected}
                    <span
                      class="text-[0.6875rem] font-semibold uppercase tracking-wide
                             text-[var(--text-faint)]"
                    >
                      Reconnect
                    </span>
                  {/if}
                </div>
                <span class="text-[0.8125rem] text-[var(--text-muted)]">
                  {template.description}
                </span>
              </button>
            {/each}
          </div>

          <!-- Active connections list -->
          {#if bots.length > 0}
            <div class="mt-8">
              <h3
                class="text-[0.8125rem] font-medium uppercase tracking-widest
                       text-[var(--text-faint)] mb-3"
              >
                Active connections
              </h3>
              <div class="border border-[var(--border)] rounded-md overflow-hidden">
                {#each bots as bot, i (bot.id)}
                  <div
                    class="flex items-center justify-between px-4 py-3
                           bg-[var(--surface)] gap-4
                           {i > 0 ? 'border-t border-[var(--border)]' : ''}"
                    class:opacity-50={!bot.has_active_key}
                  >
                    <div class="flex flex-col gap-0.5 min-w-0">
                      <div class="flex items-center gap-2">
                        <span class="text-[0.9375rem] font-medium text-[var(--text)] truncate">
                          {bot.display_name}
                        </span>
                        <span class="text-[0.75rem] text-[var(--text-faint)]">
                          {bot.username}
                        </span>
                      </div>
                      <span class="text-[0.75rem] text-[var(--text-faint)] flex items-center gap-2">
                        Connected {formatDate(bot.created_at)}
                        {#if bot.has_active_key}
                          <span
                            class="text-[0.6875rem] font-semibold uppercase tracking-wide
                                   text-[var(--success)] bg-[var(--success-bg)]
                                   px-1.5 py-0.5 rounded"
                          >
                            Active
                          </span>
                        {:else}
                          <span
                            class="text-[0.6875rem] font-semibold uppercase tracking-wide
                                   text-[var(--error)] bg-[var(--error-bg)]
                                   px-1.5 py-0.5 rounded"
                          >
                            Disconnected
                          </span>
                        {/if}
                      </span>
                    </div>
                    <div class="flex items-center gap-2 shrink-0">
                      {#if bot.has_active_key}
                        <button
                          class="text-[0.8125rem] text-[var(--error)]
                                 px-2 py-1 rounded transition-colors
                                 hover:bg-[var(--error-bg)]
                                 disabled:opacity-50 disabled:cursor-not-allowed"
                          disabled={disconnectingId === bot.id}
                          onclick={() => handleDisconnect(bot.id)}
                        >
                          {disconnectingId === bot.id ? "..." : "Disconnect"}
                        </button>
                      {:else}
                        <button
                          class="text-[0.8125rem] text-[var(--error)]
                                 px-2 py-1 rounded transition-colors
                                 hover:bg-[var(--error-bg)]
                                 disabled:opacity-50 disabled:cursor-not-allowed"
                          disabled={deletingId === bot.id}
                          onclick={() => handleDelete(bot.id)}
                        >
                          {deletingId === bot.id ? "..." : "Remove"}
                        </button>
                      {/if}
                    </div>
                  </div>
                {/each}
              </div>
            </div>
          {/if}
        </section>
      </div>
    </main>
  {/if}
</div>

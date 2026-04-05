<script lang="ts">
  import {
    me,
    logout,
    clearSession,
    listKeys,
    createKey,
    revokeKey,
    type AuthUser,
    type ApiKey,
  } from "../lib/api";
  import ThemeToggle from "../lib/ThemeToggle.svelte";

  let { navigate }: { navigate: (path: string) => void } = $props();

  let user = $state<AuthUser | null>(null);
  let keys = $state<ApiKey[]>([]);
  let loading = $state(true);

  let newKeyName = $state("");
  let creatingKey = $state(false);
  let createdKey = $state<string | null>(null);
  let keyError = $state("");
  let revokingId = $state<number | null>(null);

  $effect(() => {
    loadUser();
  });

  async function loadUser() {
    const result = await me();
    if (result.ok) {
      user = result.data;
      await loadKeys();
    } else {
      clearSession();
      navigate("/login");
    }
    loading = false;
  }

  async function loadKeys() {
    const result = await listKeys();
    if (result.ok) keys = result.data;
  }

  async function handleLogout() {
    await logout();
    clearSession();
    navigate("/login");
  }

  async function handleCreateKey(e: Event) {
    e.preventDefault();
    keyError = "";
    createdKey = null;

    const name = newKeyName.trim();
    if (!name) {
      keyError = "Give your key a name.";
      return;
    }

    creatingKey = true;
    const result = await createKey(name);
    if (result.ok) {
      createdKey = result.data.key;
      newKeyName = "";
      await loadKeys();
    } else {
      keyError = result.error;
    }
    creatingKey = false;
  }

  async function handleRevoke(id: number) {
    revokingId = id;
    await revokeKey(id);
    await loadKeys();
    revokingId = null;
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
    <!-- Top bar -->
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

    <!-- Main content -->
    <main class="flex-1 flex justify-center px-6 py-10 md:py-16">
      <div class="w-full max-w-[560px]">

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

          <!-- Info grid -->
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

        <!-- API Keys -->
        <section class="animate-reveal delay-250">
          <div class="mb-6">
            <h2 class="font-display text-[1.375rem] text-[var(--text)] mb-1">
              API Keys
            </h2>
            <p class="text-[0.875rem] text-[var(--text-muted)]">
              Keys authenticate CLI tools, MCP clients, and scripts acting on your behalf.
            </p>
          </div>

          <!-- Create form -->
          <form onsubmit={handleCreateKey} class="flex gap-2 mb-4">
            <input
              type="text"
              bind:value={newKeyName}
              placeholder="Key name (e.g. opencode, laptop, ci)"
              disabled={creatingKey}
              class="flex-1 rounded-md px-3 py-2 text-[0.9375rem]"
            />
            <button
              type="submit"
              disabled={creatingKey}
              class="shrink-0 rounded-md bg-[var(--accent)] text-[var(--accent-text)]
                     text-[0.875rem] font-medium px-4 py-2
                     whitespace-nowrap transition-all duration-200
                     hover:bg-[var(--accent-hover)] active:scale-[0.98]
                     disabled:opacity-60 disabled:cursor-not-allowed"
            >
              {creatingKey ? "Creating..." : "Create key"}
            </button>
          </form>

          {#if keyError}
            <div
              class="text-sm text-[var(--error)] bg-[var(--error-bg)]
                     px-4 py-2 rounded-md border-l-[3px] border-[var(--error)] mb-4"
              role="alert"
            >
              {keyError}
            </div>
          {/if}

          {#if createdKey}
            <div
              class="bg-[var(--success-bg)] border-l-[3px] border-[var(--success)]
                     rounded-md p-4 mb-4"
            >
              <div class="flex items-baseline gap-2 mb-2">
                <strong class="text-[var(--success)] text-sm">Key created</strong>
                <span class="text-[0.8125rem] text-[var(--text-muted)]">
                  Copy it now — it won't be shown again.
                </span>
              </div>
              <div
                class="flex items-center gap-2 bg-[var(--surface)]
                       border border-[var(--border)] rounded px-3 py-2"
              >
                <code
                  class="flex-1 font-mono text-[0.8125rem] text-[var(--text)]
                         break-all"
                >
                  {createdKey}
                </code>
                <button
                  class="shrink-0 text-[0.75rem] font-semibold uppercase
                         tracking-wide text-[var(--accent)] px-2 py-1
                         rounded transition-colors
                         hover:bg-[var(--accent-subtle)]"
                  onclick={() => navigator.clipboard.writeText(createdKey!)}
                >
                  Copy
                </button>
              </div>
            </div>
          {/if}

          <!-- Key list -->
          {#if keys.length === 0}
            <p class="text-[0.875rem] text-[var(--text-faint)] py-6">
              No keys yet. Create one to get started.
            </p>
          {:else}
            <div class="border border-[var(--border)] rounded-md overflow-hidden">
              {#each keys as key, i (key.id)}
                <div
                  class="flex items-center justify-between px-4 py-3
                         bg-[var(--surface)] gap-4
                         {i > 0 ? 'border-t border-[var(--border)]' : ''}"
                  class:opacity-50={key.revoked}
                >
                  <div class="flex flex-col gap-0.5 min-w-0">
                    <span class="text-[0.9375rem] font-medium text-[var(--text)] truncate">
                      {key.name}
                    </span>
                    <span class="text-[0.75rem] text-[var(--text-faint)] flex items-center gap-2">
                      Created {formatDate(key.created_at)}
                      {#if key.revoked}
                        <span
                          class="text-[0.6875rem] font-semibold uppercase tracking-wide
                                 text-[var(--error)] bg-[var(--error-bg)]
                                 px-1.5 py-0.5 rounded"
                        >
                          Revoked
                        </span>
                      {:else}
                        <span
                          class="text-[0.6875rem] font-semibold uppercase tracking-wide
                                 text-[var(--success)] bg-[var(--success-bg)]
                                 px-1.5 py-0.5 rounded"
                        >
                          Active
                        </span>
                      {/if}
                    </span>
                  </div>
                  {#if !key.revoked}
                    <button
                      class="shrink-0 text-[0.8125rem] text-[var(--error)]
                             px-2 py-1 rounded transition-colors
                             hover:bg-[var(--error-bg)]
                             disabled:opacity-50 disabled:cursor-not-allowed"
                      disabled={revokingId === key.id}
                      onclick={() => handleRevoke(key.id)}
                    >
                      {revokingId === key.id ? "Revoking..." : "Revoke"}
                    </button>
                  {/if}
                </div>
              {/each}
            </div>
          {/if}
        </section>
      </div>
    </main>
  {/if}
</div>

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

  let { navigate }: { navigate: (path: string) => void } = $props();

  let user = $state<AuthUser | null>(null);
  let keys = $state<ApiKey[]>([]);
  let loading = $state(true);

  // Key creation
  let newKeyName = $state("");
  let creatingKey = $state(false);
  let createdKey = $state<string | null>(null);
  let keyError = $state("");

  // Key revocation
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
    if (result.ok) {
      keys = result.data;
    }
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

<div class="home-layout">
  {#if loading}
    <div class="loading-state">
      <div class="spinner"></div>
    </div>
  {:else if user}
    <header class="topbar">
      <span class="topbar-brand">Lific</span>
      <div class="topbar-right">
        <div class="avatar" title={user.username}>
          {initials(user.display_name || user.username)}
        </div>
        <button class="btn-ghost" onclick={handleLogout}>Sign out</button>
      </div>
    </header>

    <main class="home-main">
      <div class="content-area">
        <!-- User info -->
        <section class="section section-enter-1">
          <div class="welcome-hello">
            <span class="greeting">Signed in as</span>
            <h1 class="display-name">{user.display_name || user.username}</h1>
          </div>

          <div class="info-grid">
            <div class="info-item">
              <span class="info-label">Username</span>
              <span class="info-value">{user.username}</span>
            </div>
            <div class="info-item">
              <span class="info-label">Email</span>
              <span class="info-value">{user.email}</span>
            </div>
            <div class="info-item">
              <span class="info-label">Role</span>
              <span class="info-value">{user.is_admin ? "Admin" : "Member"}</span>
            </div>
          </div>
        </section>

        <!-- API Keys -->
        <section class="section section-enter-2">
          <div class="section-header">
            <h2>API Keys</h2>
            <p class="section-desc">
              Keys authenticate CLI tools, MCP clients, and scripts acting on your behalf.
            </p>
          </div>

          <!-- Create key form -->
          <form class="create-key-form" onsubmit={handleCreateKey}>
            <input
              type="text"
              bind:value={newKeyName}
              placeholder="Key name (e.g. opencode, laptop, ci)"
              disabled={creatingKey}
            />
            <button type="submit" class="btn-primary btn-sm" disabled={creatingKey}>
              {creatingKey ? "Creating..." : "Create key"}
            </button>
          </form>

          {#if keyError}
            <div class="msg msg-error" role="alert">{keyError}</div>
          {/if}

          {#if createdKey}
            <div class="msg msg-key">
              <div class="msg-key-header">
                <strong>Key created</strong>
                <span class="msg-key-warn">Copy it now — it won't be shown again.</span>
              </div>
              <div class="key-display">
                <code>{createdKey}</code>
                <button
                  class="btn-ghost btn-copy"
                  onclick={() => { navigator.clipboard.writeText(createdKey!); }}
                >
                  Copy
                </button>
              </div>
            </div>
          {/if}

          <!-- Key list -->
          {#if keys.length === 0}
            <p class="empty-state">No keys yet. Create one to get started.</p>
          {:else}
            <div class="key-list">
              {#each keys as key (key.id)}
                <div class="key-row" class:revoked={key.revoked}>
                  <div class="key-info">
                    <span class="key-name">{key.name}</span>
                    <span class="key-meta">
                      Created {formatDate(key.created_at)}
                      {#if key.revoked}
                        <span class="key-badge badge-revoked">Revoked</span>
                      {:else}
                        <span class="key-badge badge-active">Active</span>
                      {/if}
                    </span>
                  </div>
                  {#if !key.revoked}
                    <button
                      class="btn-ghost btn-danger"
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

<style>
  .home-layout {
    min-height: 100dvh;
    display: flex;
    flex-direction: column;
  }

  .loading-state {
    flex: 1;
    display: flex;
    align-items: center;
    justify-content: center;
  }

  .spinner {
    width: 24px;
    height: 24px;
    border: 2px solid var(--border);
    border-top-color: var(--accent);
    border-radius: 50%;
    animation: spin 0.6s linear infinite;
  }

  @keyframes spin {
    to { transform: rotate(360deg); }
  }

  /* ── Top bar ───────────────────────────── */

  .topbar {
    display: flex;
    align-items: center;
    justify-content: space-between;
    padding: var(--space-md) var(--space-xl);
    border-bottom: 1px solid var(--border);
  }

  .topbar-brand {
    font-family: var(--font-display);
    font-size: 1.125rem;
    letter-spacing: -0.01em;
  }

  .topbar-right {
    display: flex;
    align-items: center;
    gap: var(--space-md);
  }

  .avatar {
    width: 32px;
    height: 32px;
    border-radius: 50%;
    background: var(--accent);
    color: #fff;
    display: flex;
    align-items: center;
    justify-content: center;
    font-size: 0.75rem;
    font-weight: 600;
    letter-spacing: 0.02em;
    user-select: none;
  }

  .btn-ghost {
    background: none;
    color: var(--text-muted);
    font-size: 0.8125rem;
    padding: var(--space-xs) var(--space-sm);
    border-radius: var(--radius-sm);
    transition: color 0.15s var(--ease-out), background 0.15s var(--ease-out);
  }

  .btn-ghost:hover {
    color: var(--text);
    background: var(--bg-subtle);
  }

  /* ── Main ──────────────────────────────── */

  .home-main {
    flex: 1;
    padding: var(--space-2xl) var(--space-xl);
    display: flex;
    justify-content: center;
  }

  .content-area {
    width: 100%;
    max-width: 560px;
  }

  .section {
    margin-bottom: var(--space-2xl);
  }

  .section-enter-1 {
    opacity: 0;
    animation: reveal 0.5s var(--ease-out) 0.1s forwards;
  }
  .section-enter-2 {
    opacity: 0;
    animation: reveal 0.5s var(--ease-out) 0.25s forwards;
  }

  /* ── User info ─────────────────────────── */

  .welcome-hello {
    margin-bottom: var(--space-lg);
  }

  .greeting {
    font-size: 0.8125rem;
    font-weight: 500;
    text-transform: uppercase;
    letter-spacing: 0.05em;
    color: var(--text-muted);
    display: block;
    margin-bottom: var(--space-xs);
  }

  .display-name {
    font-size: clamp(1.75rem, 4vw, 2.25rem);
    letter-spacing: -0.02em;
  }

  .info-grid {
    display: grid;
    grid-template-columns: repeat(3, 1fr);
    gap: 1px;
    background: var(--border);
    border: 1px solid var(--border);
    border-radius: var(--radius-md);
    overflow: hidden;
  }

  @media (max-width: 480px) {
    .info-grid {
      grid-template-columns: 1fr;
    }
  }

  .info-item {
    background: var(--surface);
    padding: var(--space-md);
  }

  .info-label {
    display: block;
    font-size: 0.6875rem;
    font-weight: 500;
    text-transform: uppercase;
    letter-spacing: 0.06em;
    color: var(--text-faint);
    margin-bottom: var(--space-xs);
  }

  .info-value {
    font-size: 0.9375rem;
    font-weight: 500;
  }

  /* ── API Keys section ──────────────────── */

  .section-header {
    margin-bottom: var(--space-lg);
  }

  .section-header h2 {
    font-size: 1.375rem;
    margin-bottom: var(--space-xs);
  }

  .section-desc {
    font-size: 0.875rem;
    color: var(--text-muted);
  }

  .create-key-form {
    display: flex;
    gap: var(--space-sm);
    margin-bottom: var(--space-md);
  }

  .create-key-form input {
    flex: 1;
  }

  .btn-primary {
    background: var(--accent);
    color: #fff;
    font-size: 0.875rem;
    font-weight: 500;
    padding: 0.5rem 1rem;
    border-radius: var(--radius-md);
    white-space: nowrap;
    transition: background 0.2s var(--ease-out), transform 0.15s var(--ease-out);
  }

  .btn-primary:hover:not(:disabled) {
    background: var(--accent-hover);
  }

  .btn-primary:active:not(:disabled) {
    transform: scale(0.98);
  }

  .btn-primary:disabled {
    opacity: 0.6;
    cursor: not-allowed;
  }

  /* ── Messages ──────────────────────────── */

  .msg {
    padding: var(--space-sm) var(--space-md);
    border-radius: var(--radius-md);
    margin-bottom: var(--space-md);
    font-size: 0.875rem;
  }

  .msg-error {
    color: var(--error);
    background: var(--error-bg);
    border-left: 3px solid var(--error);
  }

  .msg-key {
    background: var(--success-bg);
    border-left: 3px solid var(--success);
    padding: var(--space-md);
  }

  .msg-key-header {
    display: flex;
    align-items: baseline;
    gap: var(--space-sm);
    margin-bottom: var(--space-sm);
  }

  .msg-key-header strong {
    color: var(--success);
  }

  .msg-key-warn {
    font-size: 0.8125rem;
    color: var(--text-muted);
  }

  .key-display {
    display: flex;
    align-items: center;
    gap: var(--space-sm);
    background: var(--surface);
    border: 1px solid var(--border);
    border-radius: var(--radius-sm);
    padding: var(--space-sm) var(--space-md);
  }

  .key-display code {
    flex: 1;
    font-family: ui-monospace, "Cascadia Code", monospace;
    font-size: 0.8125rem;
    color: var(--text);
    word-break: break-all;
  }

  .btn-copy {
    font-size: 0.75rem;
    font-weight: 600;
    text-transform: uppercase;
    letter-spacing: 0.04em;
    color: var(--accent);
    flex-shrink: 0;
  }

  .btn-copy:hover {
    background: var(--accent-subtle);
  }

  /* ── Key list ──────────────────────────── */

  .empty-state {
    font-size: 0.875rem;
    color: var(--text-faint);
    padding: var(--space-lg) 0;
  }

  .key-list {
    border: 1px solid var(--border);
    border-radius: var(--radius-md);
    overflow: hidden;
  }

  .key-row {
    display: flex;
    align-items: center;
    justify-content: space-between;
    padding: var(--space-md);
    background: var(--surface);
    gap: var(--space-md);
  }

  .key-row + .key-row {
    border-top: 1px solid var(--border);
  }

  .key-row.revoked {
    opacity: 0.5;
  }

  .key-info {
    display: flex;
    flex-direction: column;
    gap: 2px;
    min-width: 0;
  }

  .key-name {
    font-size: 0.9375rem;
    font-weight: 500;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }

  .key-meta {
    font-size: 0.75rem;
    color: var(--text-faint);
    display: flex;
    align-items: center;
    gap: var(--space-sm);
  }

  .key-badge {
    font-size: 0.6875rem;
    font-weight: 600;
    text-transform: uppercase;
    letter-spacing: 0.04em;
    padding: 0.1em 0.4em;
    border-radius: 3px;
  }

  .badge-active {
    color: var(--success);
    background: var(--success-bg);
  }

  .badge-revoked {
    color: var(--error);
    background: var(--error-bg);
  }

  .btn-danger {
    color: var(--error);
    font-size: 0.8125rem;
    flex-shrink: 0;
  }

  .btn-danger:hover {
    background: var(--error-bg);
    color: var(--error);
  }

  .btn-danger:disabled {
    opacity: 0.5;
    cursor: not-allowed;
  }

  /* ── Animation ─────────────────────────── */

  @keyframes reveal {
    from {
      opacity: 0;
      transform: translateY(12px);
    }
    to {
      opacity: 1;
      transform: translateY(0);
    }
  }
</style>

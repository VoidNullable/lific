<script lang="ts">
  import { me, logout, clearSession, type AuthUser } from "../lib/api";

  let { navigate }: { navigate: (path: string) => void } = $props();

  let user = $state<AuthUser | null>(null);
  let loading = $state(true);
  let error = $state("");

  $effect(() => {
    loadUser();
  });

  async function loadUser() {
    const result = await me();
    if (result.ok) {
      user = result.data;
    } else {
      // Session expired or invalid
      clearSession();
      navigate("/login");
    }
    loading = false;
  }

  async function handleLogout() {
    await logout();
    clearSession();
    navigate("/login");
  }

  function initials(name: string): string {
    return name
      .split(/[\s_-]+/)
      .slice(0, 2)
      .map((w) => w[0]?.toUpperCase() ?? "")
      .join("");
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
      <div class="welcome-block">
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

        <div class="session-hint">
          <p>
            Your session token is stored locally. Use the API at
            <code>/api</code> or connect via MCP to manage issues.
          </p>
        </div>
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

  /* ── Loading ───────────────────────────── */

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
    transition:
      color 0.15s var(--ease-out),
      background 0.15s var(--ease-out);
  }

  .btn-ghost:hover {
    color: var(--text);
    background: var(--bg-subtle);
  }

  /* ── Main content ──────────────────────── */

  .home-main {
    flex: 1;
    display: flex;
    align-items: center;
    justify-content: center;
    padding: var(--space-2xl) var(--space-xl);
  }

  .welcome-block {
    max-width: 480px;
    width: 100%;
    opacity: 0;
    animation: reveal 0.5s var(--ease-out) 0.1s forwards;
  }

  .welcome-hello {
    margin-bottom: var(--space-xl);
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
    font-size: clamp(1.75rem, 4vw, 2.5rem);
    letter-spacing: -0.02em;
  }

  /* ── Info grid ─────────────────────────── */

  .info-grid {
    display: grid;
    grid-template-columns: repeat(3, 1fr);
    gap: 1px;
    background: var(--border);
    border: 1px solid var(--border);
    border-radius: var(--radius-md);
    overflow: hidden;
    margin-bottom: var(--space-xl);
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
    color: var(--text);
  }

  /* ── Session hint ──────────────────────── */

  .session-hint {
    padding: var(--space-md);
    background: var(--bg-subtle);
    border-radius: var(--radius-md);
  }

  .session-hint p {
    font-size: 0.8125rem;
    color: var(--text-muted);
    line-height: 1.5;
  }

  .session-hint code {
    font-family: ui-monospace, "Cascadia Code", monospace;
    font-size: 0.8125rem;
    background: var(--border);
    padding: 0.1em 0.35em;
    border-radius: 3px;
    color: var(--text);
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

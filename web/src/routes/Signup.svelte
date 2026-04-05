<script lang="ts">
  import { signup, saveSession } from "../lib/api";

  let { navigate }: { navigate: (path: string) => void } = $props();

  let username = $state("");
  let email = $state("");
  let password = $state("");
  let error = $state("");
  let loading = $state(false);

  async function handleSubmit(e: Event) {
    e.preventDefault();
    error = "";

    if (password.length < 8) {
      error = "Password must be at least 8 characters.";
      return;
    }

    loading = true;
    const result = await signup(username, email, password);

    if (result.ok) {
      saveSession(result.data.token);
      navigate("/home");
    } else {
      error = result.error;
      loading = false;
    }
  }
</script>

<div class="auth-layout">
  <aside class="auth-aside">
    <div class="aside-content">
      <h1 class="brand">Lific</h1>
      <p class="tagline">Local-first issue tracking.<br />Single binary. No dependencies.</p>
      <div class="aside-detail">
        <span class="detail-label">v0.1</span>
        <span class="detail-sep"></span>
        <span class="detail-value">SQLite + Rust + MCP</span>
      </div>
    </div>
  </aside>

  <main class="auth-main">
    <div class="auth-form-container">
      <div class="form-header">
        <h2>Create account</h2>
        <p class="form-subtitle">Set up your identity. No email verification needed.</p>
      </div>

      <form onsubmit={handleSubmit}>
        {#if error}
          <div class="error-message" role="alert">
            {error}
          </div>
        {/if}

        <div class="field">
          <label for="username">Username</label>
          <input
            id="username"
            type="text"
            bind:value={username}
            placeholder="blake"
            required
            autocomplete="username"
            aria-invalid={error ? "true" : undefined}
          />
        </div>

        <div class="field">
          <label for="email">Email</label>
          <input
            id="email"
            type="email"
            bind:value={email}
            placeholder="you@example.com"
            required
            autocomplete="email"
          />
        </div>

        <div class="field">
          <label for="password">Password</label>
          <input
            id="password"
            type="password"
            bind:value={password}
            placeholder="8 characters minimum"
            required
            minlength={8}
            autocomplete="new-password"
          />
        </div>

        <button type="submit" class="btn-primary" disabled={loading}>
          {#if loading}
            Creating account...
          {:else}
            Create account
          {/if}
        </button>
      </form>

      <p class="auth-switch">
        Already have an account? <button class="link-btn" onclick={() => navigate("/login")}>Sign in</button>
      </p>
    </div>
  </main>
</div>

<style>
  .auth-layout {
    display: grid;
    grid-template-columns: 1fr 1fr;
    min-height: 100dvh;
  }

  @media (max-width: 768px) {
    .auth-layout {
      grid-template-columns: 1fr;
    }
    .auth-aside {
      display: none;
    }
  }

  .auth-aside {
    background: var(--text);
    color: var(--bg);
    display: flex;
    align-items: flex-end;
    padding: var(--space-2xl);
  }

  .aside-content {
    opacity: 0;
    animation: reveal 0.6s var(--ease-out) 0.15s forwards;
  }

  .brand {
    font-size: clamp(2.5rem, 5vw, 3.5rem);
    color: var(--bg);
    margin-bottom: var(--space-md);
    letter-spacing: -0.02em;
  }

  .tagline {
    font-size: 1.0625rem;
    color: var(--text-faint);
    line-height: 1.5;
    max-width: 20ch;
  }

  .aside-detail {
    display: flex;
    align-items: center;
    gap: var(--space-sm);
    margin-top: var(--space-xl);
    font-size: 0.8125rem;
    color: var(--text-faint);
  }

  .detail-sep {
    width: 16px;
    height: 1px;
    background: var(--text-muted);
  }

  .auth-main {
    display: flex;
    align-items: center;
    justify-content: center;
    padding: var(--space-2xl) var(--space-xl);
  }

  .auth-form-container {
    width: 100%;
    max-width: 360px;
    opacity: 0;
    animation: reveal 0.6s var(--ease-out) 0.3s forwards;
  }

  .form-header {
    margin-bottom: var(--space-xl);
  }

  .form-header h2 {
    font-size: clamp(1.5rem, 3vw, 2rem);
    margin-bottom: var(--space-xs);
  }

  .form-subtitle {
    color: var(--text-muted);
    font-size: 0.9375rem;
  }

  form {
    display: flex;
    flex-direction: column;
    gap: var(--space-lg);
  }

  .field {
    display: flex;
    flex-direction: column;
  }

  .error-message {
    font-size: 0.875rem;
    color: var(--error);
    background: var(--error-bg);
    padding: var(--space-sm) var(--space-md);
    border-radius: var(--radius-md);
    border-left: 3px solid var(--error);
  }

  .btn-primary {
    background: var(--accent);
    color: #fff;
    font-size: 0.9375rem;
    font-weight: 500;
    padding: 0.6875rem 1.25rem;
    border-radius: var(--radius-md);
    transition:
      background 0.2s var(--ease-out),
      transform 0.15s var(--ease-out);
    margin-top: var(--space-sm);
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

  .auth-switch {
    text-align: center;
    margin-top: var(--space-xl);
    color: var(--text-muted);
    font-size: 0.875rem;
  }

  .link-btn {
    background: none;
    color: var(--accent);
    font-size: inherit;
    padding: 0;
    font-weight: 500;
  }

  .link-btn:hover {
    text-decoration: underline;
  }

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

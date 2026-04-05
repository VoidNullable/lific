<script lang="ts">
  import { login, saveSession } from "../lib/api";
  import ThemeToggle from "../lib/ThemeToggle.svelte";

  let { navigate }: { navigate: (path: string) => void } = $props();

  let identity = $state("");
  let password = $state("");
  let error = $state("");
  let loading = $state(false);

  async function handleSubmit(e: Event) {
    e.preventDefault();
    error = "";
    loading = true;

    const result = await login(identity, password);

    if (result.ok) {
      saveSession(result.data.token);
      navigate("/home");
    } else {
      error = result.error;
      loading = false;
    }
  }
</script>

<div class="grid min-h-dvh md:grid-cols-2">
  <!-- Left panel -->
  <aside
    class="hidden md:flex flex-col justify-between p-10
           bg-[var(--panel-bg)] text-[var(--panel-text)]"
  >
    <div></div>
    <div class="animate-reveal delay-150">
      <h1 class="font-display text-[clamp(2.5rem,5vw,3.5rem)] leading-[1.1] tracking-tight text-[var(--panel-text)] mb-4">
        Lific
      </h1>
      <p class="text-[1.0625rem] leading-relaxed text-[var(--panel-muted)] max-w-[20ch]">
        Local-first issue tracking.<br />Single binary. No dependencies.
      </p>
      <div class="flex items-center gap-2 mt-10 text-[0.8125rem] text-[var(--panel-muted)]">
        <span>v0.1</span>
        <span class="w-4 h-px bg-[var(--panel-muted)]"></span>
        <span>SQLite + Rust + MCP</span>
      </div>
    </div>
  </aside>

  <!-- Right panel -->
  <main class="flex flex-col items-center justify-center p-10">
    <div class="w-full max-w-[360px] animate-reveal delay-300">

      <!-- Mobile brand -->
      <h1 class="md:hidden font-display text-2xl text-[var(--text)] mb-6">Lific</h1>

      <div class="mb-10">
        <h2 class="font-display text-[clamp(1.5rem,3vw,2rem)] text-[var(--text)] mb-1">Sign in</h2>
        <p class="text-[0.9375rem] text-[var(--text-muted)]">
          Enter your username or email to continue.
        </p>
      </div>

      <form onsubmit={handleSubmit} class="flex flex-col gap-6">
        {#if error}
          <div
            class="text-sm text-[var(--error)] bg-[var(--error-bg)]
                   px-4 py-2 rounded-md border-l-[3px] border-[var(--error)]"
            role="alert"
          >
            {error}
          </div>
        {/if}

        <div class="flex flex-col">
          <label
            for="identity"
            class="text-[0.8125rem] font-medium text-[var(--text-muted)]
                   uppercase tracking-wider mb-1"
          >
            Username or email
          </label>
          <input
            id="identity"
            type="text"
            bind:value={identity}
            placeholder="blake"
            required
            autocomplete="username"
            aria-invalid={error ? "true" : undefined}
            class="rounded-md px-3 py-2.5 text-[0.9375rem]"
          />
        </div>

        <div class="flex flex-col">
          <label
            for="password"
            class="text-[0.8125rem] font-medium text-[var(--text-muted)]
                   uppercase tracking-wider mb-1"
          >
            Password
          </label>
          <input
            id="password"
            type="password"
            bind:value={password}
            required
            autocomplete="current-password"
            class="rounded-md px-3 py-2.5 text-[0.9375rem]"
          />
        </div>

        <button
          type="submit"
          disabled={loading}
          class="mt-2 rounded-md bg-[var(--accent)] text-[var(--accent-text)]
                 text-[0.9375rem] font-medium py-2.5 px-5
                 transition-all duration-200
                 hover:bg-[var(--accent-hover)] active:scale-[0.98]
                 disabled:opacity-60 disabled:cursor-not-allowed"
        >
          {loading ? "Signing in..." : "Sign in"}
        </button>
      </form>

      <p class="text-center mt-10 text-[0.875rem] text-[var(--text-muted)]">
        No account?
        <button
          class="text-[var(--accent)] font-medium bg-transparent border-none cursor-pointer hover:underline"
          onclick={() => navigate("/signup")}
        >
          Create one
        </button>
      </p>

      <div class="flex justify-center mt-6">
        <ThemeToggle />
      </div>
    </div>
  </main>
</div>

<script lang="ts">
  import { signup, saveSession } from "../lib/api";
  import AuthShell from "../lib/AuthShell.svelte";
  import { AlertTriangle } from "lucide-svelte";

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
      navigate("/settings");
    } else {
      error = result.error;
      loading = false;
    }
  }
</script>

<AuthShell
  {navigate}
  title="New around here."
  tagline="Lizzy's got a fresh notebook ready. Let's get you set up."
  altText="Already have an account?"
  altLabel="Sign in"
  altHref="/login"
  mascotSrc="/LizzyWriting.png"
  mascotW={567}
  mascotH={562}
  mascotScale={0.46}
  mascotCaption="First issue's on her."
>
  <form onsubmit={handleSubmit} class="flex flex-col gap-4">
    {#if error}
      <div
        class="flex items-start gap-2 text-[0.8125rem] text-[var(--error)]
               bg-[var(--error-bg)] px-3 py-2.5 rounded-lg"
        role="alert"
      >
        <AlertTriangle size={15} class="shrink-0 mt-0.5" />
        <span>{error}</span>
      </div>
    {/if}

    <label class="flex flex-col gap-1.5">
      <span class="text-[0.6875rem] font-semibold uppercase tracking-widest text-[var(--text-faint)]">
        Username
      </span>
      <input
        type="text"
        bind:value={username}
        placeholder="jane"
        required
        autocomplete="username"
        aria-invalid={error ? "true" : undefined}
        class="rounded-lg px-3 py-2.5 text-[0.9375rem]"
      />
    </label>

    <label class="flex flex-col gap-1.5">
      <span class="text-[0.6875rem] font-semibold uppercase tracking-widest text-[var(--text-faint)]">
        Email
      </span>
      <input
        type="email"
        bind:value={email}
        placeholder="you@example.com"
        required
        autocomplete="email"
        class="rounded-lg px-3 py-2.5 text-[0.9375rem]"
      />
    </label>

    <label class="flex flex-col gap-1.5">
      <span class="text-[0.6875rem] font-semibold uppercase tracking-widest text-[var(--text-faint)]">
        Password
      </span>
      <input
        type="password"
        bind:value={password}
        placeholder="8 characters minimum"
        required
        minlength={8}
        autocomplete="new-password"
        class="rounded-lg px-3 py-2.5 text-[0.9375rem]"
      />
    </label>

    <button
      type="submit"
      disabled={loading}
      class="mt-2 rounded-lg bg-[var(--btn-success)] text-[var(--btn-success-text)]
             text-[0.9375rem] font-medium py-2.5 px-5
             transition-all duration-200
             hover:bg-[var(--btn-success-hover)] motion-safe:active:scale-[0.98]
             focus-visible:ring-2 focus-visible:ring-[var(--btn-success)] focus-visible:ring-offset-2 focus-visible:ring-offset-[var(--bg)]
             disabled:opacity-60 disabled:cursor-not-allowed"
    >
      {loading ? "Creating account…" : "Create account"}
    </button>
  </form>
</AuthShell>

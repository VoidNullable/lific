<script lang="ts">
  import { login, saveSession, getInstance } from "../lib/api";
  import AuthShell from "../lib/AuthShell.svelte";
  import { AlertTriangle, Eye, EyeOff } from "lucide-svelte";
  import { onMount } from "svelte";

  let { navigate }: { navigate: (path: string) => void } = $props();

  let identity = $state("");
  let password = $state("");
  let error = $state("");
  let loading = $state(false);

  // Shown as a plain fact in the breadcrumb (which server you are on).
  const host = window.location.host;
  let instanceName = $state<string | null>(null);
  let loginMessage = $state<string | null>(null);

  onMount(async () => {
    const res = await getInstance();
    if (res.ok) {
      instanceName = res.data.instance_name;
      loginMessage = res.data.login_message;
    }
  });

  // Per-field touch tracking so syntax errors only surface on blur, never
  // mid-keystroke (research: "avoid screaming errors on every keystroke").
  let identityTouched = $state(false);

  // Password visibility toggle. Swaps the input `type` in place — the DOM
  // node is never unmounted, so cursor/selection and autofill survive.
  let showPassword = $state(false);

  let identityError = $derived(
    identityTouched && identity.trim() === ""
      ? "Enter your username or email."
      : "",
  );

  // Submit is enabled once both fields have content. Final validity is the
  // server's call; this just blocks obviously-empty submits.
  let canSubmit = $derived(identity.trim() !== "" && password !== "");

  let identityEl: HTMLInputElement | undefined = $state();
  let passwordEl: HTMLInputElement | undefined = $state();

  async function handleSubmit(e: Event) {
    e.preventDefault();
    error = "";
    identityTouched = true;

    // Focus the first empty field instead of submitting a doomed request.
    if (identity.trim() === "") {
      identityEl?.focus();
      return;
    }
    if (password === "") {
      passwordEl?.focus();
      return;
    }

    loading = true;
    const result = await login(identity, password);

    if (result.ok) {
      saveSession(result.data.token);
      navigate("/settings");
    } else {
      error = result.error;
      loading = false;
      // Server rejected the credentials — return focus to the password so a
      // retry is one keystroke away.
      passwordEl?.focus();
    }
  }
</script>

<AuthShell
  mode="login"
  {navigate}
  {host}
  {instanceName}
  {loginMessage}
  title="Welcome back."
  subtitle="Sign in to continue on this instance."
  mascotSrc="/LizzyReading.png"
  mascotW={487}
  mascotH={714}
>
  <form onsubmit={handleSubmit} class="flex flex-col gap-5" novalidate>
    <!-- Form-level error, announced politely to screen readers. -->
    <div aria-live="polite">
      {#if error}
        <div
          class="flex items-start gap-2.5 text-[0.8125rem] text-[var(--error)]
                 bg-[var(--error-bg)] px-3.5 py-3 rounded-lg"
          role="alert"
        >
          <AlertTriangle size={15} class="shrink-0 mt-0.5" />
          <span>{error}</span>
        </div>
      {/if}
    </div>

    <!-- Identity -->
    <div class="flex flex-col gap-1.5">
      <label
        for="login-identity"
        class="text-[0.6875rem] font-semibold uppercase tracking-widest text-[var(--text-faint)]"
      >
        Username or email
      </label>
      <input
        id="login-identity"
        bind:this={identityEl}
        type="text"
        bind:value={identity}
        onblur={() => (identityTouched = true)}
        placeholder="jane"
        autocomplete="username"
        autocapitalize="none"
        spellcheck="false"
        aria-invalid={identityError ? "true" : undefined}
        aria-describedby={identityError ? "login-identity-err" : undefined}
        class="input-field rounded-lg px-3.5 py-2.5 text-[0.9375rem]"
      />
      {#if identityError}
        <p id="login-identity-err" class="text-[0.75rem] text-[var(--error)]">
          {identityError}
        </p>
      {/if}
    </div>

    <!-- Password with in-place visibility toggle. -->
    <div class="flex flex-col gap-1.5">
      <label
        for="login-password"
        class="text-[0.6875rem] font-semibold uppercase tracking-widest text-[var(--text-faint)]"
      >
        Password
      </label>
      <div class="relative">
        <input
          id="login-password"
          bind:this={passwordEl}
          type={showPassword ? "text" : "password"}
          bind:value={password}
          autocomplete="current-password"
          class="input-field w-full rounded-lg pl-3.5 pr-11 py-2.5 text-[0.9375rem]"
        />
        <button
          type="button"
          onclick={() => (showPassword = !showPassword)}
          aria-pressed={showPassword}
          aria-label={showPassword ? "Hide password" : "Show password"}
          title={showPassword ? "Hide password" : "Show password"}
          class="absolute inset-y-0 right-0 flex items-center px-3 text-[var(--text-faint)]
                 hover:text-[var(--text-muted)] transition-colors
                 focus-visible:outline-none focus-visible:text-[var(--accent)]"
          tabindex="-1"
        >
          {#if showPassword}
            <EyeOff size={17} />
          {:else}
            <Eye size={17} />
          {/if}
        </button>
      </div>
    </div>

    <button
      type="submit"
      disabled={loading || !canSubmit}
      class="mt-1 rounded-lg bg-[var(--btn-success)] text-[var(--btn-success-text)]
             text-[0.9375rem] font-medium py-2.5 px-5
             transition-all duration-200
             hover:bg-[var(--btn-success-hover)] motion-safe:active:scale-[0.98]
             focus-visible:ring-2 focus-visible:ring-[var(--btn-success)] focus-visible:ring-offset-2 focus-visible:ring-offset-[var(--bg)]
             disabled:opacity-55 disabled:cursor-not-allowed disabled:hover:bg-[var(--btn-success)]"
    >
      {loading ? "Signing in…" : "Sign in"}
    </button>
  </form>
</AuthShell>

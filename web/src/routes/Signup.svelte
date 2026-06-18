<script lang="ts">
  import { signup, saveSession, getInstance } from "../lib/api";
  import AuthShell from "../lib/AuthShell.svelte";
  import StatusIcon, { statusCssColor } from "../lib/StatusIcon.svelte";
  import { AlertTriangle, Eye, EyeOff, Check, Lock } from "lucide-svelte";
  import { onMount } from "svelte";

  let { navigate }: { navigate: (path: string) => void } = $props();

  let username = $state("");
  let email = $state("");
  let password = $state("");
  let error = $state("");
  let loading = $state(false);

  let showPassword = $state(false);

  // Touch tracking: syntax errors surface on blur, never per-keystroke.
  let usernameTouched = $state(false);
  let emailTouched = $state(false);
  // The password checklist is the exception: it is progress feedback, so it
  // appears as soon as the user starts typing (and turns green live).
  let passwordTouched = $state(false);

  let usernameEl: HTMLInputElement | undefined = $state();
  let emailEl: HTMLInputElement | undefined = $state();
  let passwordEl: HTMLInputElement | undefined = $state();

  // ── Instance state (drives framing, never claims ownership) ──
  // Admin is granted out of band via the CLI, never by web signup, so this
  // page never tells anyone they own the instance. It only distinguishes a
  // brand-new instance (be the first account) from one you are joining, and
  // surfaces a real closed-signup state instead of submitting then erroring.
  const host = window.location.host;
  let infoLoaded = $state(false);
  let allowSignup = $state(true); // optimistic: open is the common case
  let hasUsers = $state(true); //    optimistic: avoids a "be the first" flash
  let instanceName = $state<string | null>(null);
  let loginMessage = $state<string | null>(null);

  onMount(async () => {
    const result = await getInstance();
    if (result.ok) {
      allowSignup = result.data.allow_signup;
      hasUsers = result.data.has_users;
      instanceName = result.data.instance_name;
      loginMessage = result.data.login_message;
    }
    infoLoaded = true;
  });

  const closed = $derived(infoLoaded && !allowSignup);
  const fresh = $derived(allowSignup && !hasUsers);

  const title = $derived(
    closed ? "Signups are closed." : fresh ? "Be the first." : "Create your account.",
  );
  const subtitle = $derived(
    closed
      ? "This instance is not accepting new accounts right now."
      : fresh
        ? "No one has an account on this instance yet. Create the first one to get started."
        : "Join this instance and start tracking work.",
  );

  // ── Getting started, as real Lific issue rows ──
  // The flow is rendered in the app's own issue + status vocabulary. On a
  // successful signup the first step ticks from active to done, exactly the
  // way the app changes an issue's status, then we navigate in.
  let accountDone = $state(false);
  const steps = $derived([
    { title: "Create your account", status: accountDone ? "done" : "active" },
    { title: "Start your first project", status: "todo" },
    { title: "Connect your AI tools", status: "backlog" },
  ]);

  // ── Validation ──
  // Permissive email check: reject the obviously broken (no @, no domain dot,
  // spaces) without playing RFC hero. Deliverability is the server's job.
  const emailOk = $derived(/^[^\s@]+@[^\s@]+\.[^\s@]+$/.test(email));
  const usernameOk = $derived(/^[a-zA-Z0-9_-]{2,}$/.test(username.trim()));

  // The 8-char minimum mirrors the server exactly; the others nudge toward a
  // stronger secret without blocking submit.
  const reqs = $derived([
    { label: "At least 8 characters", met: password.length >= 8 },
    { label: "A lowercase and uppercase letter", met: /[a-z]/.test(password) && /[A-Z]/.test(password) },
    { label: "A number or symbol", met: /[\d\W]/.test(password) },
  ]);
  const metCount = $derived(reqs.filter((r) => r.met).length);
  const passwordOk = $derived(password.length >= 8);

  const strength = $derived(
    metCount <= 1
      ? { label: "Weak", color: "var(--strength-weak)", segments: 1 }
      : metCount === 2
        ? { label: "Fair", color: "var(--strength-fair)", segments: 2 }
        : { label: "Strong", color: "var(--strength-strong)", segments: 3 },
  );

  const usernameError = $derived(
    usernameTouched && username.trim() !== "" && !usernameOk
      ? "Use at least 2 letters, numbers, dashes or underscores."
      : usernameTouched && username.trim() === ""
        ? "Pick a username."
        : "",
  );
  const emailError = $derived(
    emailTouched && email !== "" && !emailOk
      ? "That does not look like an email address."
      : emailTouched && email === ""
        ? "Enter your email."
        : "",
  );

  const canSubmit = $derived(usernameOk && emailOk && passwordOk);

  async function handleSubmit(e: Event) {
    e.preventDefault();
    error = "";
    usernameTouched = true;
    emailTouched = true;
    passwordTouched = true;

    if (!usernameOk) return usernameEl?.focus();
    if (!emailOk) return emailEl?.focus();
    if (!passwordOk) return passwordEl?.focus();

    loading = true;
    const result = await signup(username, email, password);

    if (result.ok) {
      saveSession(result.data.token);
      // Tick the first step done (the app's status-change), then enter.
      accountDone = true;
      setTimeout(() => navigate("/settings"), 750);
    } else {
      error = result.error;
      loading = false;
    }
  }
</script>

<AuthShell
  mode="signup"
  {navigate}
  {host}
  {instanceName}
  {loginMessage}
  {title}
  {subtitle}
  mascotSrc="/LizzyWriting.png"
  mascotW={567}
  mascotH={562}
>
  {#if closed}
    <!-- Closed-signup state: no form. New accounts come from whoever runs the
         instance (admin is granted via the CLI, not web signup). -->
    <div class="flex flex-col gap-5">
      <div class="flex items-start gap-2.5 text-[0.8125rem] text-[var(--text-muted)] bg-[var(--bg-subtle)] px-3.5 py-3 rounded-lg">
        <Lock size={15} class="shrink-0 mt-0.5" />
        <span>
          New accounts on this instance are created by whoever runs it. Ask them to
          add you, then come back and sign in.
        </span>
      </div>
      <button
        type="button"
        onclick={() => navigate("/login")}
        class="rounded-lg bg-[var(--btn-success)] text-[var(--btn-success-text)]
               text-[0.9375rem] font-medium py-2.5 px-5 transition-all duration-200
               hover:bg-[var(--btn-success-hover)] motion-safe:active:scale-[0.98]
               focus-visible:ring-2 focus-visible:ring-[var(--btn-success)] focus-visible:ring-offset-2 focus-visible:ring-offset-[var(--bg)]"
      >
        Go to sign in
      </button>
    </div>
  {:else}
    <form onsubmit={handleSubmit} class="flex flex-col gap-5" novalidate>
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

      <!-- Username -->
      <div class="flex flex-col gap-1.5">
        <label
          for="signup-username"
          class="text-[0.6875rem] font-semibold uppercase tracking-widest text-[var(--text-faint)]"
        >
          Username
        </label>
        <input
          id="signup-username"
          bind:this={usernameEl}
          type="text"
          bind:value={username}
          onblur={() => (usernameTouched = true)}
          placeholder="jane"
          autocomplete="username"
          autocapitalize="none"
          spellcheck="false"
          aria-invalid={usernameError ? "true" : undefined}
          aria-describedby={usernameError ? "signup-username-err" : undefined}
          class="input-field rounded-lg px-3.5 py-2.5 text-[0.9375rem]"
        />
        {#if usernameError}
          <p id="signup-username-err" class="text-[0.75rem] text-[var(--error)]">
            {usernameError}
          </p>
        {/if}
      </div>

      <!-- Email -->
      <div class="flex flex-col gap-1.5">
        <label
          for="signup-email"
          class="text-[0.6875rem] font-semibold uppercase tracking-widest text-[var(--text-faint)]"
        >
          Email
        </label>
        <input
          id="signup-email"
          bind:this={emailEl}
          type="email"
          bind:value={email}
          onblur={() => (emailTouched = true)}
          placeholder="you@example.com"
          autocomplete="email"
          autocapitalize="none"
          spellcheck="false"
          aria-invalid={emailError ? "true" : undefined}
          aria-describedby={emailError ? "signup-email-err" : undefined}
          class="input-field rounded-lg px-3.5 py-2.5 text-[0.9375rem]"
        />
        {#if emailError}
          <p id="signup-email-err" class="text-[0.75rem] text-[var(--error)]">
            {emailError}
          </p>
        {/if}
      </div>

      <!-- Password + live requirement checklist -->
      <div class="flex flex-col gap-1.5">
        <label
          for="signup-password"
          class="text-[0.6875rem] font-semibold uppercase tracking-widest text-[var(--text-faint)]"
        >
          Password
        </label>
        <div class="relative">
          <input
            id="signup-password"
            bind:this={passwordEl}
            type={showPassword ? "text" : "password"}
            bind:value={password}
            oninput={() => (passwordTouched = true)}
            autocomplete="new-password"
            aria-describedby="signup-password-reqs"
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

        <!-- Strength meter + checklist. Appears once the user starts typing;
             collapses to nothing on an empty field so the form stays calm. -->
        {#if passwordTouched && password !== ""}
          <div id="signup-password-reqs" class="mt-2 flex flex-col gap-2.5">
            <div class="flex items-center gap-2">
              <div class="flex gap-1 grow">
                {#each [0, 1, 2] as i (i)}
                  <span
                    class="h-1 grow rounded-full transition-colors duration-300"
                    style="background-color: {i < strength.segments
                      ? strength.color
                      : 'var(--border)'}"
                  ></span>
                {/each}
              </div>
              <span
                class="text-[0.6875rem] font-medium tabular-nums w-[3.25rem] text-right"
                style="color: {strength.color}"
              >
                {strength.label}
              </span>
            </div>

            <ul class="flex flex-col gap-1">
              {#each reqs as req (req.label)}
                <li
                  class="flex items-center gap-2 text-[0.75rem] transition-colors duration-200"
                  style="color: {req.met ? 'var(--success)' : 'var(--text-faint)'}"
                >
                  <span
                    class="flex items-center justify-center size-4 rounded-full shrink-0 transition-all duration-200"
                    style="background-color: {req.met
                      ? 'color-mix(in srgb, var(--success) 18%, transparent)'
                      : 'var(--bg-subtle)'}"
                  >
                    {#if req.met}
                      <Check size={11} strokeWidth={3} />
                    {:else}
                      <span class="size-1 rounded-full bg-[var(--text-faint)]"></span>
                    {/if}
                  </span>
                  {req.label}
                </li>
              {/each}
            </ul>
          </div>
        {/if}
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
        {accountDone ? "Welcome aboard…" : loading ? "Creating account…" : "Create account"}
      </button>
    </form>

    <!-- Getting started, in Lific's own issue + status vocabulary. These are
         your real next steps; the first ticks to done when signup succeeds. -->
    <div class="mt-8 pt-6 border-t border-[var(--border)]">
      <p class="text-[0.625rem] font-semibold uppercase tracking-widest text-[var(--text-faint)] mb-2.5">
        Getting started
      </p>
      <ul class="rounded-lg border border-[var(--border)] bg-[var(--surface)] overflow-hidden">
        {#each steps as step, i (step.title)}
          <li
            class="flex items-center gap-2.5 px-3 py-2.5 {i > 0
              ? 'border-t border-[var(--border)]'
              : ''}"
          >
            <StatusIcon status={step.status} size={15} />
            <span
              class="flex-1 text-[0.8125rem] {step.status === 'done'
                ? 'line-through text-[var(--text-muted)]'
                : 'text-[var(--text)]'}"
            >
              {step.title}
            </span>
            <span
              class="text-[0.6875rem] font-medium capitalize tabular-nums"
              style="color: {statusCssColor(step.status)}"
            >
              {step.status}
            </span>
          </li>
        {/each}
      </ul>
    </div>
  {/if}
</AuthShell>

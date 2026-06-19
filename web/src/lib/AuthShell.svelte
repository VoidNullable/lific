<script lang="ts">
  // LIF-203 v6: the auth screen IS Lific, signed out. It wears the real
  // application chrome so you feel like you are already inside the product:
  //
  //   - the 230px --chrome sidebar (real brand header + version pill, a
  //     signed-out footer identity slot, the real theme cycle)
  //   - the --chrome topbar with a breadcrumb (the host you are on, a plain
  //     fact for self-hosters, no decoration) and the exact segmented switcher
  //     the issue list uses for List/Board, here flipping Sign in / Create
  //     account
  //   - the recessed, rounded-tl content panel with the chrome's cast-shadow
  //     overlays, holding the page's headline + form
  //
  // No live-status dot, no glow, no "INSTANCE" eyebrow chrome: those were
  // decoration faking telemetry the page does not have. Below lg the sidebar
  // drops away and the panel goes full bleed so the form owns the phone. The
  // mascot rests silently in the corner at the app's empty-state scale.
  import Mascot from "./Mascot.svelte";
  import {
    getPreference,
    setPreference,
    resolveTheme,
    type ThemePreference,
  } from "./theme";
  import { LogIn, UserPlus, Sun, Moon, Monitor, ChevronRight } from "lucide-svelte";

  let {
    mode,
    navigate,
    host,
    instanceName = null,
    loginMessage = null,
    title,
    subtitle,
    mascotSrc,
    mascotW,
    mascotH,
    children,
  }: {
    /** Which auth surface is active; drives the switcher + breadcrumb. */
    mode: "login" | "signup";
    navigate: (path: string) => void;
    /** Host of the instance, shown plainly in the breadcrumb. */
    host: string;
    /** Admin-set instance name; replaces the host in the breadcrumb when set. */
    instanceName?: string | null;
    /** Admin-set message shown on the auth screen. */
    loginMessage?: string | null;
    title: string;
    subtitle: string;
    mascotSrc: string;
    mascotW: number;
    mascotH: number;
    children: import("svelte").Snippet;
  } = $props();

  const crumb = $derived(mode === "signup" ? "Create account" : "Sign in");

  // Theme cycle, identical behavior to the real sidebar footer control.
  let themePref = $state<ThemePreference>(getPreference());
  let themeResolved = $derived(resolveTheme(themePref));
  function cycleTheme() {
    const order: ThemePreference[] = ["light", "dark", "system"];
    themePref = order[(order.indexOf(themePref) + 1) % order.length];
    setPreference(themePref);
  }
</script>

{#snippet themeButton(extra: string)}
  <button
    class="size-8 shrink-0 grid place-items-center rounded-md
           text-[var(--text-muted)] hover:text-[var(--text)] hover:bg-[var(--bg-subtle)] transition-colors {extra}"
    onclick={cycleTheme}
    title="Theme: {themePref}"
    aria-label="Cycle theme, current: {themePref}"
  >
    {#if themePref === "system"}
      <Monitor size={15} />
    {:else if themeResolved === "dark"}
      <Moon size={15} />
    {:else}
      <Sun size={15} />
    {/if}
  </button>
{/snippet}

{#snippet switcher()}
  <!-- Same segmented control the issue list uses for List/Board. -->
  <div
    class="flex items-center gap-0.5 p-0.5 rounded-md bg-[var(--bg)]
           shadow-[inset_0_1px_2px_rgba(0,0,0,0.10)]"
  >
    <button
      class="flex items-center gap-1 px-2.5 py-1 rounded text-caption font-medium transition-all
             {mode === 'login'
        ? 'bg-[var(--surface)] text-[var(--text)] shadow-[0_1px_2px_rgba(0,0,0,0.16),0_1px_1px_rgba(0,0,0,0.10)]'
        : 'text-[var(--text-muted)] hover:text-[var(--text)]'}"
      aria-pressed={mode === "login"}
      onclick={() => navigate("/login")}
    >
      <LogIn size={11} class="shrink-0" />
      Sign in
    </button>
    <button
      class="flex items-center gap-1 px-2.5 py-1 rounded text-caption font-medium transition-all
             {mode === 'signup'
        ? 'bg-[var(--surface)] text-[var(--text)] shadow-[0_1px_2px_rgba(0,0,0,0.16),0_1px_1px_rgba(0,0,0,0.10)]'
        : 'text-[var(--text-muted)] hover:text-[var(--text)]'}"
      aria-pressed={mode === "signup"}
      onclick={() => navigate("/signup")}
    >
      <UserPlus size={11} class="shrink-0" />
      Create account
    </button>
  </div>
{/snippet}

<div class="h-dvh flex overflow-hidden bg-[var(--chrome)]">
  <!-- ── SIDEBAR (signed out) ──────────────────────────────── -->
  <aside class="hidden lg:flex w-[230px] shrink-0 flex-col bg-[var(--chrome)] select-none">
    <!-- Brand header, identical to the app. -->
    <div class="px-3 pt-3 pb-2">
      <div class="flex items-center gap-2.5 px-1 py-1">
        <img src="/logo.webp" alt="" width="26" height="26" class="rounded-md shrink-0" />
        <span class="font-display text-heading tracking-tight text-[var(--text)] leading-none flex-1">
          Lific
        </span>
        <span class="font-mono text-micro tracking-tight text-[var(--text-faint)] px-1.5 py-0.5 rounded-md bg-[var(--bg-subtle)]">
          v{__APP_VERSION__}
        </span>
      </div>
    </div>

    <div class="flex-1"></div>

    <!-- Footer: signed-out identity placeholder + theme cycle. -->
    <div class="p-2 flex items-center gap-1">
      <div class="flex-1 min-w-0 flex items-center gap-2.5 px-2 py-1.5 rounded-md">
        <div class="size-7 rounded-full border border-[var(--border)] bg-[var(--bg-subtle)] grid place-items-center shrink-0">
          <LogIn size={13} class="text-[var(--text-faint)]" />
        </div>
        <div class="flex-1 min-w-0">
          <div class="text-body-sm text-[var(--text-muted)] truncate leading-tight">
            Not signed in
          </div>
          <div class="text-micro text-[var(--text-faint)] leading-tight mt-0.5">
            {crumb} to continue
          </div>
        </div>
      </div>
      {@render themeButton("")}
    </div>
  </aside>

  <!-- ── Right column: topbar + recessed content panel ─────── -->
  <div class="flex-1 min-w-0 flex flex-col">
    <div class="shrink-0 flex items-center gap-3 px-4 sm:px-6 py-2 bg-[var(--chrome)]">
      <!-- Breadcrumb: host (a plain fact) › current surface. -->
      <div class="flex items-center gap-1.5 min-w-0">
        <img src="/logo.webp" alt="" width="20" height="20" class="rounded shrink-0 lg:hidden" />
        {#if instanceName}
          <span class="hidden sm:inline text-body-sm font-medium text-[var(--text-muted)] truncate max-w-[16rem]" title={host}>
            {instanceName}
          </span>
        {:else}
          <span class="hidden sm:inline font-mono text-body-sm font-medium text-[var(--text-muted)] truncate max-w-[16rem]" title={host}>
            {host}
          </span>
        {/if}
        <ChevronRight size={12} class="hidden sm:inline text-[var(--text-faint)] shrink-0" />
        <span class="text-body-sm font-medium text-[var(--text)]">{crumb}</span>
      </div>

      <div class="ml-auto flex items-center gap-2">
        {@render switcher()}
        {@render themeButton("lg:hidden")}
      </div>
    </div>

    <!-- Recessed content panel with the chrome's cast-shadow overlays. -->
    <div class="relative flex-1 min-w-0 lg:rounded-tl-xl overflow-hidden">
      <main class="absolute inset-0 bg-[var(--bg)] overflow-y-auto">
        <div class="min-h-full flex flex-col">
          <div class="flex-1 px-6 sm:px-10 lg:px-14 py-10 lg:py-14">
            <div class="w-full max-w-[28rem]">
              <h1 class="font-display text-display font-semibold tracking-[-0.02em] text-[var(--text)] leading-[1.08] animate-reveal">
                {title}
              </h1>
              <p class="text-body-lg text-[var(--text-muted)] leading-relaxed mt-2.5 max-w-[40ch] animate-reveal delay-100">
                {subtitle}
              </p>

              {#if loginMessage}
                <div class="mt-5 text-body-sm text-[var(--text-muted)] leading-relaxed bg-[var(--bg-subtle)] border border-[var(--border)] rounded-lg px-3.5 py-2.5 max-w-[40ch] animate-reveal delay-100">
                  {loginMessage}
                </div>
              {/if}

              <div class="mt-8 animate-reveal delay-150">
                {@render children()}
              </div>
            </div>
          </div>

          <!-- Silent mascot, resting in the corner at app empty-state scale. -->
          <div class="pointer-events-none flex justify-end px-6 lg:px-10 pb-6 opacity-90">
            <Mascot src={mascotSrc} nativeW={mascotW} nativeH={mascotH} scale={0.16} />
          </div>
        </div>
      </main>
      <!-- Top edge cast shadow. -->
      <div class="pointer-events-none absolute top-0 left-0 right-0 h-6 z-10 bg-gradient-to-b from-[var(--shadow-recess)] to-transparent"></div>
      <!-- Left edge cast shadow (desktop, where the sidebar sits). -->
      <div class="pointer-events-none absolute top-0 left-0 bottom-0 w-6 z-10 bg-gradient-to-r from-[var(--shadow-recess)] to-transparent hidden lg:block"></div>
    </div>
  </div>
</div>

<script lang="ts">
  // LIF-203 v2: auth chrome with actual personality. Lizzy is the co-host,
  // not decoration. The layout is asymmetric and editorial — form anchored
  // left, Lizzy occupying the right at floor level like she's sitting next
  // to you. No centered card, no glow, no circles. The warm stone surface
  // and Space Grotesk do the work.
  //
  // Each auth surface passes the relevant Lizzy (Reading for login, Writing
  // for signup) so the mascot's activity matches the page's intent.
  import Mascot from "./Mascot.svelte";
  import ThemeToggle from "./ThemeToggle.svelte";

  let {
    tagline,
    title,
    altText,
    altLabel,
    altHref,
    navigate,
    mascotSrc,
    mascotW,
    mascotH,
    mascotScale = 0.4,
    mascotCaption = "",
    children,
  }: {
    tagline: string;
    title: string;
    altText: string;
    altLabel: string;
    altHref: string;
    navigate: (path: string) => void;
    mascotSrc: string;
    mascotW: number;
    mascotH: number;
    mascotScale?: number;
    mascotCaption?: string;
    children: import("svelte").Snippet;
  } = $props();
</script>

<div class="min-h-dvh relative bg-[var(--bg)] overflow-hidden">
  <!-- Theme toggle, top-right corner. Small, unobtrusive. -->
  <div class="absolute top-5 right-5 z-20">
    <ThemeToggle />
  </div>

  <!-- Lizzy, right side, floor level. Hidden on mobile — she's brand, not
       functionality, and at phone widths she'd crush the form. On desktop
       she's a real presence: large, warm, sitting next to you. -->
  <div
    aria-hidden="true"
    class="hidden lg:flex pointer-events-none absolute right-0 bottom-0 z-0
           items-end pr-8 xl:pr-16 pb-0"
  >
    <div class="flex flex-col items-center">
      <Mascot src={mascotSrc} nativeW={mascotW} nativeH={mascotH} scale={mascotScale} />
      {#if mascotCaption}
        <p class="font-display text-[0.75rem] text-[var(--text-faint)] italic mt-3 max-w-[18ch] text-center leading-snug">
          {mascotCaption}
        </p>
      {/if}
    </div>
  </div>

  <!-- Form column, anchored left. -->
  <div class="relative z-10 min-h-dvh flex items-center px-6 sm:px-12 lg:px-20 xl:px-28 py-16">
    <div class="w-full max-w-[400px] animate-reveal">

      <!-- Masthead: logo + wordmark, inline. Not centered, not in a card. -->
      <div class="flex items-center gap-3 mb-10">
        <img
          src="/logo.webp"
          alt=""
          width="36"
          height="36"
          class="rounded-lg shadow-[0_2px_8px_rgba(0,0,0,0.1)]"
        />
        <span class="font-display text-[1.5rem] font-semibold tracking-tight text-[var(--text)] leading-none">
          Lific
        </span>
      </div>

      <!-- Editorial headline + voice tagline. Big, left-aligned, confident. -->
      <h1 class="font-display text-[2rem] sm:text-[2.25rem] font-semibold tracking-tight text-[var(--text)] leading-[1.1]">
        {title}
      </h1>
      <p class="text-[0.9375rem] text-[var(--text-muted)] leading-relaxed mt-2.5 max-w-[34ch]">
        {tagline}
      </p>

      <!-- Form, directly on the surface. No card wrapper — the layout
           provides structure, a container would just be noise. -->
      <div class="mt-8">
        {@render children()}
      </div>

      <!-- Switch link. Quiet, left-aligned. -->
      <p class="mt-6 text-[0.875rem] text-[var(--text-muted)]">
        {altText}
        <button
          class="text-[var(--accent)] font-medium bg-transparent border-none cursor-pointer hover:underline ml-1"
          onclick={() => navigate(altHref)}
        >
          {altLabel}
        </button>
      </p>
    </div>
  </div>

  <!-- Version, bottom-left corner. Mono, faint. -->
  <p class="absolute bottom-5 left-6 sm:left-12 lg:left-20 xl:left-28 z-10
            font-mono text-[0.6875rem] text-[var(--text-faint)] tracking-wide">
    v{__APP_VERSION__}
  </p>
</div>

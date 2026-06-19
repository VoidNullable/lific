<script lang="ts">
  // LIF-193: shared error surface. Deliberately NOT the centered vertical
  // stack used by empty states — here the copy sits left and the "Oops"
  // mascot bolts off toward the right edge (tilted + bleeding past the
  // frame), so an error reads as "Lizzy ran off with it" rather than a calm
  // empty room. No animation: the artwork already implies the motion, so a
  // static tilt + exit does the work.
  //
  // IMPORTANT (no server-state leak): callers choose `message`. Use the
  // backend's deliberate API error strings for expected failures; for
  // UNEXPECTED exceptions (error boundary) pass generic copy, never raw
  // Error/stack text.
  import Mascot from "./Mascot.svelte";

  let {
    title,
    message = "",
    scale = 0.42,
    children,
  }: {
    title: string;
    message?: string;
    scale?: number;
    /** Action buttons (e.g. Home / Reload). */
    children?: import("svelte").Snippet;
  } = $props();
</script>

<!-- w-full + flex-1 so the surface fills its parent even inside a flex *row*
     (e.g. the board's column track), where a default flex child would shrink
     to content width and pin left, leaving dead space on the right. -->
<div class="w-full flex-1 h-full min-h-[55vh] relative overflow-hidden flex items-center">
  <!-- Copy, anchored left -->
  <div class="relative z-10 max-w-[440px] pl-8 sm:pl-14 pr-6 py-12">
    <p class="font-display text-[1.375rem] tracking-tight text-[var(--text)] leading-tight">
      {title}
    </p>
    {#if message}
      <p class="text-body text-[var(--text-muted)] leading-relaxed mt-2 max-w-[42ch]">
        {message}
      </p>
    {/if}
    {#if children}
      <div class="flex items-center gap-2 mt-5">{@render children()}</div>
    {/if}
  </div>

  <!-- Lizzy bolting for the exit: low-right, tilted into the run, and
       translated so she's already half off the frame (clipped by the
       container's overflow-hidden). -->
  <div
    aria-hidden="true"
    class="pointer-events-none absolute right-0 bottom-[14%] z-0
           rotate-[7deg] translate-x-[18%]"
  >
    <Mascot src="/LizzyOops.png" nativeW={742} nativeH={488} {scale} />
  </div>
</div>

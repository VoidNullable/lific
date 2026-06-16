<script lang="ts">
  // LIF-193: shared error surface — the "Oops" mascot + a title, an optional
  // message, and caller-supplied action buttons. Rendered at the same mascot
  // scale (0.25) as every other empty/quiet state so errors feel like part of
  // the product, not a raw stack trace.
  //
  // IMPORTANT (no server-state leak): callers decide what `message` to show.
  // Use deliberate, server-authored API error strings (res.error) for expected
  // failures; for UNEXPECTED exceptions (error boundary) pass a generic line
  // and NEVER the raw Error/stack.
  import Mascot from "./Mascot.svelte";

  let {
    title,
    message = "",
    scale = 0.25,
    children,
  }: {
    title: string;
    message?: string;
    scale?: number;
    /** Action buttons (e.g. Home / Reload). */
    children?: import("svelte").Snippet;
  } = $props();
</script>

<div class="h-full min-h-[55vh] flex flex-col items-center justify-center gap-4 px-6 text-center">
  <Mascot src="/LizzyOops.png" nativeW={742} nativeH={488} {scale} />
  <div class="flex flex-col items-center gap-1.5 max-w-[440px]">
    <p class="text-[1.0625rem] font-medium text-[var(--text)]">{title}</p>
    {#if message}
      <p class="text-[0.875rem] text-[var(--text-muted)] leading-relaxed">{message}</p>
    {/if}
  </div>
  {#if children}
    <div class="flex items-center gap-2 mt-1">{@render children()}</div>
  {/if}
</div>

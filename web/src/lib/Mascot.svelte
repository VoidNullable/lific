<script lang="ts">
  // Shared mascot renderer. The white-silhouette PNGs are drawn via CSS
  // mask so they pick up a theme-aware muted fill (a raw <img> vanishes on
  // light). Crucially, size is driven by `scale` = screen pixels per source
  // pixel, NOT a fixed width: every mascot rendered at the same `scale`
  // therefore shares an identical pixel-to-screen ratio, so the artwork
  // appears at a consistent scale across surfaces regardless of each PNG's
  // canvas dimensions or padding.

  let {
    src,
    nativeW,
    nativeH,
    scale = 0.25,
    class: className = "",
    fill = "var(--text-faint)",
    opacity = 0.5,
  }: {
    src: string;
    /** Intrinsic pixel dimensions of the source PNG. */
    nativeW: number;
    nativeH: number;
    /** Screen pixels per source pixel. Same value ⇒ same rendered scale. */
    scale?: number;
    class?: string;
    /** Mask fill color. Override on dark brand surfaces where the default
     *  muted text color would vanish. */
    fill?: string;
    /** Mask opacity. */
    opacity?: number;
  } = $props();

  const w = $derived(Math.round(nativeW * scale));
  const h = $derived(Math.round(nativeH * scale));
</script>

<div
  aria-hidden="true"
  class="shrink-0 {className}"
  style="width: {w}px; height: {h}px; opacity: {opacity};
         background-color: {fill};
         -webkit-mask: url({src}) center / contain no-repeat;
         mask: url({src}) center / contain no-repeat;"
></div>

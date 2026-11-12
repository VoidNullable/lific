<script lang="ts" module>
  export interface LightboxImage {
    /** The original, never the thumbnail. */
    src: string;
    alt: string;
    /** Suggested filename for Download. */
    name?: string;
  }
</script>

<script lang="ts">
  // One lightbox for every image surface: Markdown bodies, the Attachments
  // panel and the inline attachment viewer. Opens fitted to the screen; a
  // click toggles 100% (about the click), the wheel and pinch zoom about the
  // pointer, a drag pans, arrow keys step through the other images of the
  // same surface, and Download / Open original reach the full file.
  //
  // The geometry lives in lightboxZoom.ts; this component only turns pointer
  // events into calls on it.

  import { onMount, untrack } from "svelte";
  import { ChevronLeft, ChevronRight, Download, ExternalLink, X } from "lucide-svelte";
  import {
    fitScale,
    fitView,
    panBy,
    stepIndex,
    toggleZoom,
    zoomAt,
    type Size,
    type View,
  } from "./lightboxZoom";

  let {
    images,
    start = 0,
    onClose,
  }: {
    images: LightboxImage[];
    start?: number;
    onClose: () => void;
  } = $props();

  /** Space kept clear around a fitted image, per side. */
  const PAD = 32;
  /** Movement past this many pixels makes a press a drag, not a click. */
  const DRAG_SLOP = 4;

  let index = $state(untrack(() => start));
  let current = $derived(images[index]);
  let stage = $state<HTMLDivElement | null>(null);
  let natural = $state<Size | null>(null);
  let viewport = $state<Size>({ width: 0, height: 0 });
  let view = $state<View>({ scale: 1, x: 0, y: 0 });

  let zoomed = $derived(natural !== null && view.scale > fitScale(natural, viewport) + 1e-6);

  function measure() {
    if (!stage) return;
    const next = {
      width: Math.max(1, stage.clientWidth - 2 * PAD),
      height: Math.max(1, stage.clientHeight - 2 * PAD),
    };
    const wasZoomed = zoomed;
    viewport = next;
    if (natural && !wasZoomed) view = fitView(natural, viewport);
  }

  onMount(() => {
    measure();
    stage?.focus();
    const observer = new ResizeObserver(measure);
    if (stage) observer.observe(stage);
    return () => observer.disconnect();
  });

  function onLoad(e: Event) {
    const img = e.currentTarget as HTMLImageElement;
    natural = { width: img.naturalWidth, height: img.naturalHeight };
    view = fitView(natural, viewport);
  }

  function go(delta: number) {
    if (images.length < 2) return;
    index = stepIndex(index, delta, images.length);
    natural = null;
  }

  /** A client point relative to the stage centre, which is the viewport's. */
  function pointFrom(clientX: number, clientY: number) {
    const rect = stage!.getBoundingClientRect();
    return {
      x: clientX - rect.left - rect.width / 2,
      y: clientY - rect.top - rect.height / 2,
    };
  }

  function zoomBy(factor: number, point = { x: 0, y: 0 }) {
    if (natural) view = zoomAt(view, factor, point, natural, viewport);
  }

  // Pointer state: every active pointer's last position, for pans and pinches.
  const pointers = new Map<number, { x: number; y: number }>();
  let pressStart: { x: number; y: number } | null = null;
  let dragged = false;
  let dragging = $state(false);

  function pinchSpan() {
    const [a, b] = [...pointers.values()];
    return {
      distance: Math.hypot(a.x - b.x, a.y - b.y),
      mid: { x: (a.x + b.x) / 2, y: (a.y + b.y) / 2 },
    };
  }

  function onPointerDown(e: PointerEvent) {
    if (e.button !== 0) return;
    if ((e.target as HTMLElement).closest("button, a")) return;
    stage?.setPointerCapture(e.pointerId);
    pointers.set(e.pointerId, { x: e.clientX, y: e.clientY });
    if (pointers.size === 1) {
      pressStart = { x: e.clientX, y: e.clientY };
      dragged = false;
    }
  }

  function onPointerMove(e: PointerEvent) {
    const last = pointers.get(e.pointerId);
    if (!last || !natural) return;
    if (pointers.size === 2) {
      const before = pinchSpan();
      pointers.set(e.pointerId, { x: e.clientX, y: e.clientY });
      const after = pinchSpan();
      if (before.distance > 0) {
        zoomBy(after.distance / before.distance, pointFrom(after.mid.x, after.mid.y));
      }
      dragged = true;
      return;
    }
    pointers.set(e.pointerId, { x: e.clientX, y: e.clientY });
    if (pressStart && Math.hypot(e.clientX - pressStart.x, e.clientY - pressStart.y) > DRAG_SLOP) {
      dragged = true;
    }
    if (dragged && zoomed) {
      dragging = true;
      view = panBy(view, e.clientX - last.x, e.clientY - last.y, natural, viewport);
    }
  }

  function onPointerUp(e: PointerEvent) {
    pointers.delete(e.pointerId);
    if (pointers.size === 0) {
      pressStart = null;
      dragging = false;
    }
  }

  // Pointer capture retargets the click to the stage, so whether it landed
  // on the image is decided by position: on the image it zooms, on the
  // backdrop it closes.
  let imageEl = $state<HTMLImageElement | null>(null);

  function onStageClick(e: MouseEvent) {
    if ((e.target as HTMLElement).closest("button, a")) return;
    if (dragged) {
      dragged = false;
      return;
    }
    const r = imageEl?.getBoundingClientRect();
    const onImage =
      !!r && e.clientX >= r.left && e.clientX <= r.right && e.clientY >= r.top && e.clientY <= r.bottom;
    if (!onImage) onClose();
    else if (natural) view = toggleZoom(view, pointFrom(e.clientX, e.clientY), natural, viewport);
  }

  function onWheel(e: WheelEvent) {
    e.preventDefault();
    zoomBy(Math.exp(-e.deltaY * 0.0015), pointFrom(e.clientX, e.clientY));
  }

  function onKeydown(e: KeyboardEvent) {
    let handled = true;
    if (e.key === "Escape") onClose();
    else if (e.key === "ArrowLeft") go(-1);
    else if (e.key === "ArrowRight") go(1);
    else if (e.key === "+" || e.key === "=") zoomBy(1.25);
    else if (e.key === "-") zoomBy(0.8);
    else if (e.key === "0" && natural) view = fitView(natural, viewport);
    else handled = false;
    if (handled) {
      e.preventDefault();
      e.stopPropagation();
    }
  }
</script>

<svelte:window onkeydown={onKeydown} />

<!-- svelte-ignore a11y_no_noninteractive_element_interactions -->
<!-- svelte-ignore a11y_click_events_have_key_events -->
<div
  bind:this={stage}
  class="lightbox"
  class:lightbox--zoomed={zoomed}
  class:lightbox--dragging={dragging}
  role="dialog"
  aria-modal="true"
  aria-label="Image preview"
  tabindex="-1"
  onclick={onStageClick}
  onpointerdown={onPointerDown}
  onpointermove={onPointerMove}
  onpointerup={onPointerUp}
  onpointercancel={onPointerUp}
  onwheel={onWheel}
>
  {#key current.src}
    <img
      bind:this={imageEl}
      class="lightbox__img"
      class:lightbox__img--ready={natural !== null}
      src={current.src}
      alt={current.alt}
      draggable="false"
      onload={onLoad}
      style:width={natural ? `${natural.width * view.scale}px` : null}
      style:height={natural ? `${natural.height * view.scale}px` : null}
      style:transform={`translate(calc(-50% + ${view.x}px), calc(-50% + ${view.y}px))`}
    />
  {/key}

  <div class="lightbox__bar">
    {#if natural}
      <span class="lightbox__zoom" aria-live="polite">{Math.round(view.scale * 100)}%</span>
    {/if}
    <a
      class="lightbox__btn"
      href={current.src}
      download={current.name ?? ""}
      title="Download"
      aria-label="Download"
    >
      <Download size={16} />
    </a>
    <a
      class="lightbox__btn"
      href={current.src}
      target="_blank"
      rel="noopener noreferrer"
      title="Open original"
      aria-label="Open original"
    >
      <ExternalLink size={16} />
    </a>
    <button type="button" class="lightbox__btn" title="Close" aria-label="Close" onclick={onClose}>
      <X size={16} />
    </button>
  </div>

  {#if images.length > 1}
    <button
      type="button"
      class="lightbox__btn lightbox__nav lightbox__nav--prev"
      title="Previous image"
      aria-label="Previous image"
      onclick={() => go(-1)}
    >
      <ChevronLeft size={20} />
    </button>
    <button
      type="button"
      class="lightbox__btn lightbox__nav lightbox__nav--next"
      title="Next image"
      aria-label="Next image"
      onclick={() => go(1)}
    >
      <ChevronRight size={20} />
    </button>
  {/if}

  {#if images.length > 1 || current.alt}
    <div class="lightbox__caption">
      {#if images.length > 1}<span class="lightbox__count">{index + 1} / {images.length}</span>{/if}
      {#if current.alt}<span class="lightbox__alt">{current.alt}</span>{/if}
    </div>
  {/if}
</div>

<style>
  .lightbox {
    position: fixed;
    inset: 0;
    z-index: 1200;
    overflow: hidden;
    background: rgba(0, 0, 0, 0.82);
    cursor: zoom-out;
    touch-action: none;
    user-select: none;
    outline: none;
    animation: lightbox-fade 0.12s var(--ease-out-expo);
  }
  .lightbox__img {
    position: absolute;
    top: 50%;
    left: 50%;
    max-width: none;
    opacity: 0;
    border-radius: 0.375rem;
    box-shadow: 0 24px 64px rgba(0, 0, 0, 0.5);
    cursor: zoom-in;
  }
  .lightbox__img--ready {
    opacity: 1;
  }
  .lightbox--zoomed .lightbox__img {
    cursor: grab;
  }
  .lightbox--dragging .lightbox__img {
    cursor: grabbing;
  }
  .lightbox__bar {
    position: absolute;
    top: max(0.75rem, env(safe-area-inset-top));
    right: 0.75rem;
    display: flex;
    align-items: center;
    gap: 0.25rem;
  }
  .lightbox__zoom {
    min-width: 3rem;
    padding: 0 0.375rem;
    color: rgba(255, 255, 255, 0.85);
    font-size: var(--text-caption);
    font-variant-numeric: tabular-nums;
    text-align: right;
  }
  .lightbox__btn {
    display: inline-flex;
    align-items: center;
    justify-content: center;
    width: 2.25rem;
    height: 2.25rem;
    border-radius: 0.5rem;
    background: rgba(0, 0, 0, 0.45);
    color: #fff;
    cursor: pointer;
    transition: background 0.15s var(--ease-out-expo);
  }
  .lightbox__btn:hover {
    background: rgba(0, 0, 0, 0.7);
  }
  .lightbox__btn:focus-visible {
    outline: 2px solid #fff;
    outline-offset: 1px;
  }
  .lightbox__nav {
    position: absolute;
    top: 50%;
    transform: translateY(-50%);
  }
  .lightbox__nav--prev {
    left: 0.75rem;
  }
  .lightbox__nav--next {
    right: 0.75rem;
  }
  .lightbox__caption {
    position: absolute;
    left: 50%;
    bottom: max(0.75rem, env(safe-area-inset-bottom));
    transform: translateX(-50%);
    display: flex;
    gap: 0.75rem;
    max-width: calc(100% - 1.5rem);
    padding: 0.25rem 0.625rem;
    border-radius: 0.375rem;
    background: rgba(0, 0, 0, 0.55);
    color: rgba(255, 255, 255, 0.9);
    font-size: var(--text-caption);
    pointer-events: none;
  }
  .lightbox__count {
    flex-shrink: 0;
    font-variant-numeric: tabular-nums;
  }
  .lightbox__alt {
    min-width: 0;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }
  @keyframes lightbox-fade {
    from {
      opacity: 0;
    }
    to {
      opacity: 1;
    }
  }
  @media (prefers-reduced-motion: reduce) {
    .lightbox {
      animation: none;
    }
    .lightbox__btn {
      transition: none;
    }
  }
</style>

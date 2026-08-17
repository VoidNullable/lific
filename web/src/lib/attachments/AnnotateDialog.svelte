<script lang="ts">
  // LIF-418: annotate a screenshot on its way into an upload.
  //
  // Two phases behind one component so the caller only has to await one
  // promise:
  //
  //   1. prompt  — a small chip offering "Annotate before upload?". It does not
  //                block anything, it does not steal focus, and it gives up
  //                after four seconds and lets the plain upload proceed. The
  //                overwhelmingly common case is "I pasted a screenshot and I
  //                just want it attached", so the annotator must never be
  //                something you have to dismiss.
  //   2. editing — a full-viewport canvas editor.
  //
  // Raw canvas, no drawing library. Shapes are kept in image space (see
  // annotateMath.ts) and re-rendered from scratch on every change, which is
  // cheap at screenshot sizes and means undo is just "restore the previous
  // snapshot" rather than a pile of inverse operations.
  //
  // Redaction is real. `pixelateSteps` downsamples the region to one pixel per
  // block and paints it back with smoothing off, so the flattened PNG never
  // contained the original pixels. An overlay rectangle would be a lie: anyone
  // can open the file and peel it off.

  import {
    createUndoStack,
    fitScale,
    normalizeRect,
    outputFilename,
    outputMime,
    pixelateSteps,
    resizeCrop,
    strokeWidthFor,
    type CropHandle,
    type Point,
    type Rect,
  } from "./annotateMath";
  import { Crop, ArrowUpRight, Square, Pen, EyeOff, Undo2, X } from "lucide-svelte";

  let {
    file,
    onDone,
  }: {
    file: File;
    /** Called exactly once with the file that should actually be uploaded:
     *  the flattened annotation, or the untouched original on skip/cancel. */
    onDone: (result: File) => void;
  } = $props();

  type Tool = "crop" | "arrow" | "rect" | "pen" | "redact";

  type Shape =
    | { kind: "arrow"; from: Point; to: Point; color: string }
    | { kind: "rect"; from: Point; to: Point; color: string }
    | { kind: "pen"; points: Point[]; color: string }
    | { kind: "redact"; rect: Rect };

  interface Snapshot {
    shapes: Shape[];
    crop: Rect | null;
  }

  // Four colours, no picker. Any more and the toolbar becomes a paint program;
  // these read against light chrome, dark chrome and photographic content.
  const COLORS = ["#ff3b30", "#ffb020", "#22c55e", "#3b82f6"];
  const PROMPT_MS = 4000;

  const TOOLS: { id: Tool; label: string; icon: typeof Crop }[] = [
    { id: "arrow", label: "Arrow", icon: ArrowUpRight },
    { id: "rect", label: "Rectangle", icon: Square },
    { id: "pen", label: "Pen", icon: Pen },
    { id: "redact", label: "Redact", icon: EyeOff },
    { id: "crop", label: "Crop", icon: Crop },
  ];

  let phase = $state<"prompt" | "editing">("prompt");
  let promptProgress = $state(0);
  let tool = $state<Tool>("arrow");
  let color = $state(COLORS[0]);
  let shapes = $state<Shape[]>([]);
  let crop = $state<Rect | null>(null);
  let undoDepth = $state(0);
  let loadFailed = $state(false);
  let flattening = $state(false);
  let viewport = $state({ w: 1024, h: 640 });

  let canvasEl = $state<HTMLCanvasElement | null>(null);
  let rootEl = $state<HTMLDivElement | null>(null);

  const history = createUndoStack<Snapshot>();
  // `file` is fixed for the lifetime of the dialog: annotateFlow mounts one
  // instance per file and unmounts it as soon as the promise settles. Reading
  // it once at setup is the intent, not an oversight.
  // svelte-ignore state_referenced_locally
  const objectUrl = URL.createObjectURL(file);
  const scratch = document.createElement("canvas");

  // Decoded eagerly, during the prompt phase: by the time someone presses
  // Enter the bitmap is already there and the editor opens with no flash.
  let image = $state<HTMLImageElement | null>(null);
  const loader = new Image();
  loader.onload = () => {
    image = loader;
  };
  loader.onerror = () => {
    loadFailed = true;
    if (phase === "editing") finish(file);
  };
  loader.src = objectUrl;

  let imageSize = $derived({ w: image?.naturalWidth ?? 0, h: image?.naturalHeight ?? 0 });
  let scale = $derived(image ? fitScale(imageSize, viewport) : 1);
  let stroke = $derived(image ? strokeWidthFor(imageSize) : 4);
  let displayW = $derived(Math.max(1, Math.round(imageSize.w * scale)));
  let displayH = $derived(Math.max(1, Math.round(imageSize.h * scale)));

  // ── lifecycle ────────────────────────────────────────────

  let settled = false;

  function finish(result: File) {
    if (settled) return;
    settled = true;
    URL.revokeObjectURL(objectUrl);
    onDone(result);
  }

  function openEditor() {
    if (phase === "editing") return;
    if (loadFailed) {
      finish(file);
      return;
    }
    phase = "editing";
    measure();
    queueMicrotask(() => rootEl?.focus());
  }

  function measure() {
    if (typeof window === "undefined") return;
    // Leave room for the toolbar strip and the footer actions.
    viewport = {
      w: Math.max(240, window.innerWidth - 48),
      h: Math.max(200, window.innerHeight - 172),
    };
  }

  $effect(() => {
    measure();
    const onResize = () => measure();
    window.addEventListener("resize", onResize);
    return () => window.removeEventListener("resize", onResize);
  });

  // Prompt countdown. Rendered as a hairline progress bar so the four seconds
  // are visible rather than a surprise.
  $effect(() => {
    if (phase !== "prompt") return;
    const started = performance.now();
    let raf = 0;
    const tick = () => {
      const elapsed = performance.now() - started;
      promptProgress = Math.min(1, elapsed / PROMPT_MS);
      if (elapsed >= PROMPT_MS) {
        finish(file);
        return;
      }
      raf = requestAnimationFrame(tick);
    };
    raf = requestAnimationFrame(tick);
    return () => cancelAnimationFrame(raf);
  });

  function onKey(e: KeyboardEvent) {
    if (e.key === "Escape") {
      e.preventDefault();
      finish(file);
      return;
    }
    if (phase === "prompt") {
      if (e.key === "Enter") {
        e.preventDefault();
        openEditor();
      }
      return;
    }
    if ((e.ctrlKey || e.metaKey) && e.key.toLowerCase() === "z") {
      e.preventDefault();
      undo();
      return;
    }
    // Enter accepts, unless a toolbar button has focus, where Enter means
    // "press this button" to every keyboard user alive.
    if (e.key === "Enter" && !(e.target instanceof HTMLButtonElement)) {
      e.preventDefault();
      void accept();
    }
  }

  // ── history ──────────────────────────────────────────────

  function cloneShape(shape: Shape): Shape {
    return shape.kind === "pen" ? { ...shape, points: shape.points.slice() } : { ...shape };
  }

  function snapshot() {
    history.push({ shapes: shapes.map(cloneShape), crop: crop ? { ...crop } : null });
    undoDepth = history.size();
  }

  function undo() {
    const prev = history.undo();
    undoDepth = history.size();
    if (!prev) return;
    shapes = prev.shapes;
    crop = prev.crop;
    render();
  }

  // ── pointer gestures ─────────────────────────────────────

  // In-flight gesture state is deliberately NOT reactive: a pen stroke fires
  // dozens of pointermove events per second and each one would otherwise
  // invalidate the render effect. We mutate and call render() directly.
  let live: Shape | null = null;
  let dragStart: Point | null = null;
  let cropHandle: CropHandle | null = null;

  function toImage(e: PointerEvent): Point {
    const el = canvasEl;
    if (!el) return { x: 0, y: 0 };
    const rect = el.getBoundingClientRect();
    return {
      x: ((e.clientX - rect.left) / rect.width) * imageSize.w,
      y: ((e.clientY - rect.top) / rect.height) * imageSize.h,
    };
  }

  const HANDLES: CropHandle[] = ["nw", "n", "ne", "e", "se", "s", "sw", "w"];

  function handlePoint(rect: Rect, handle: CropHandle): Point {
    const midX = rect.x + rect.w / 2;
    const midY = rect.y + rect.h / 2;
    const x = handle.includes("w") ? rect.x : handle.includes("e") ? rect.x + rect.w : midX;
    const y = handle.includes("n") ? rect.y : handle.includes("s") ? rect.y + rect.h : midY;
    return { x, y };
  }

  /** Handle under the pointer, using a constant *screen* radius so grabbing an
   *  edge is equally easy on a 400px thumbnail and a 4K screenshot. */
  function hitHandle(p: Point): CropHandle | null {
    if (!crop) return null;
    const radius = 14 / scale;
    for (const handle of HANDLES) {
      const h = handlePoint(crop, handle);
      if (Math.abs(h.x - p.x) <= radius && Math.abs(h.y - p.y) <= radius) return handle;
    }
    return null;
  }

  function onPointerDown(e: PointerEvent) {
    if (!image || e.button !== 0) return;
    canvasEl?.setPointerCapture(e.pointerId);
    const p = toImage(e);
    dragStart = p;

    if (tool === "crop") {
      cropHandle = hitHandle(p);
      snapshot();
      if (!cropHandle) crop = { x: p.x, y: p.y, w: 0, h: 0 };
      render();
      return;
    }

    snapshot();
    live =
      tool === "pen"
        ? { kind: "pen", points: [p], color }
        : tool === "redact"
          ? { kind: "redact", rect: { x: p.x, y: p.y, w: 0, h: 0 } }
          : { kind: tool, from: p, to: p, color };
    render();
  }

  function onPointerMove(e: PointerEvent) {
    if (!dragStart || !image) return;
    const p = toImage(e);

    if (tool === "crop") {
      crop = cropHandle
        ? resizeCrop(crop ?? { x: 0, y: 0, w: imageSize.w, h: imageSize.h }, cropHandle, p, imageSize)
        : normalizeRect(dragStart, p);
      render();
      return;
    }

    if (!live) return;
    if (live.kind === "pen") live.points.push(p);
    else if (live.kind === "redact") live.rect = normalizeRect(dragStart, p);
    else live.to = p;
    render();
  }

  function onPointerUp(e: PointerEvent) {
    if (!dragStart) return;
    canvasEl?.releasePointerCapture(e.pointerId);
    const started = dragStart;
    dragStart = null;
    cropHandle = null;

    if (tool === "crop") {
      // A stray click (rather than a drag) should not leave a 0x0 crop that
      // would flatten to an empty image.
      if (crop && (crop.w < 8 || crop.h < 8)) crop = null;
      render();
      return;
    }

    const committed = live;
    live = null;
    if (!committed) return;

    const moved =
      committed.kind === "pen"
        ? committed.points.length > 1
        : committed.kind === "redact"
          ? committed.rect.w > 4 && committed.rect.h > 4
          : Math.hypot(committed.to.x - started.x, committed.to.y - started.y) > 4;

    if (!moved) {
      // Nothing was drawn, so the snapshot taken on pointerdown would make
      // undo a no-op step. Roll it back.
      history.undo();
      undoDepth = history.size();
      render();
      return;
    }
    shapes.push(committed);
    render();
  }

  // ── rendering ────────────────────────────────────────────

  function drawRedaction(ctx: CanvasRenderingContext2D, rect: Rect) {
    if (!image) return;
    const steps = pixelateSteps(rect, imageSize);
    if (!steps) return;
    scratch.width = steps.small.w;
    scratch.height = steps.small.h;
    const sctx = scratch.getContext("2d");
    if (!sctx) return;
    sctx.clearRect(0, 0, scratch.width, scratch.height);
    sctx.drawImage(
      image,
      steps.source.x,
      steps.source.y,
      steps.source.w,
      steps.source.h,
      0,
      0,
      steps.small.w,
      steps.small.h,
    );
    ctx.save();
    ctx.imageSmoothingEnabled = false;
    ctx.drawImage(
      scratch,
      0,
      0,
      steps.small.w,
      steps.small.h,
      steps.dest.x,
      steps.dest.y,
      steps.dest.w,
      steps.dest.h,
    );
    ctx.restore();
  }

  function drawArrow(ctx: CanvasRenderingContext2D, from: Point, to: Point, width: number) {
    const angle = Math.atan2(to.y - from.y, to.x - from.x);
    const head = Math.max(width * 3.4, 10);
    // Stop the shaft short of the tip so the head reads as solid rather than
    // as a line poking through a triangle.
    const shaftX = to.x - Math.cos(angle) * head * 0.72;
    const shaftY = to.y - Math.sin(angle) * head * 0.72;
    ctx.beginPath();
    ctx.moveTo(from.x, from.y);
    ctx.lineTo(shaftX, shaftY);
    ctx.stroke();
    ctx.beginPath();
    ctx.moveTo(to.x, to.y);
    ctx.lineTo(to.x - Math.cos(angle - 0.42) * head, to.y - Math.sin(angle - 0.42) * head);
    ctx.lineTo(to.x - Math.cos(angle + 0.42) * head, to.y - Math.sin(angle + 0.42) * head);
    ctx.closePath();
    ctx.fill();
  }

  function drawShape(ctx: CanvasRenderingContext2D, shape: Shape, width: number) {
    if (shape.kind === "redact") {
      drawRedaction(ctx, shape.rect);
      return;
    }
    ctx.save();
    ctx.strokeStyle = shape.color;
    ctx.fillStyle = shape.color;
    ctx.lineWidth = width;
    ctx.lineCap = "round";
    ctx.lineJoin = "round";
    if (shape.kind === "arrow") {
      drawArrow(ctx, shape.from, shape.to, width);
    } else if (shape.kind === "rect") {
      const r = normalizeRect(shape.from, shape.to);
      ctx.strokeRect(r.x, r.y, r.w, r.h);
    } else {
      ctx.beginPath();
      shape.points.forEach((p, i) => (i === 0 ? ctx.moveTo(p.x, p.y) : ctx.lineTo(p.x, p.y)));
      ctx.stroke();
    }
    ctx.restore();
  }

  /** Base image plus every committed shape, in image coordinates. Shared by the
   *  on-screen canvas and the export canvas so what you see is what ships. */
  function drawScene(ctx: CanvasRenderingContext2D, width: number) {
    if (!image) return;
    ctx.drawImage(image, 0, 0);
    // Redactions first: they sample the untouched bitmap, so they must not be
    // able to eat an arrow the user drew to point at something.
    for (const shape of shapes) if (shape.kind === "redact") drawShape(ctx, shape, width);
    for (const shape of shapes) if (shape.kind !== "redact") drawShape(ctx, shape, width);
  }

  function drawCropOverlay(ctx: CanvasRenderingContext2D) {
    if (!crop || crop.w <= 0 || crop.h <= 0) return;
    ctx.save();
    ctx.fillStyle = "rgba(10, 12, 11, 0.52)";
    ctx.beginPath();
    ctx.rect(0, 0, imageSize.w, imageSize.h);
    ctx.rect(crop.x, crop.y, crop.w, crop.h);
    ctx.fill("evenodd");
    ctx.strokeStyle = "#ffffff";
    ctx.lineWidth = Math.max(1, 1.5 / scale);
    ctx.strokeRect(crop.x, crop.y, crop.w, crop.h);
    const size = 9 / scale;
    for (const handle of HANDLES) {
      const p = handlePoint(crop, handle);
      ctx.fillStyle = "#ffffff";
      ctx.fillRect(p.x - size / 2, p.y - size / 2, size, size);
    }
    ctx.restore();
  }

  function render() {
    const el = canvasEl;
    if (!el || !image) return;
    if (el.width !== imageSize.w || el.height !== imageSize.h) {
      el.width = imageSize.w;
      el.height = imageSize.h;
    }
    const ctx = el.getContext("2d");
    if (!ctx) return;
    ctx.clearRect(0, 0, el.width, el.height);
    drawScene(ctx, stroke);
    if (live) drawShape(ctx, live, stroke);
    if (tool === "crop") drawCropOverlay(ctx);
  }

  // Committed state changes repaint through the effect; in-flight gestures call
  // render() directly (see `live`).
  $effect(() => {
    void [image, shapes.length, crop, tool, canvasEl, scale];
    render();
  });

  // ── output ───────────────────────────────────────────────

  async function flatten(): Promise<File> {
    if (!image) return file;
    const area = crop && crop.w >= 8 && crop.h >= 8
      ? crop
      : { x: 0, y: 0, w: imageSize.w, h: imageSize.h };
    const out = document.createElement("canvas");
    out.width = Math.max(1, Math.round(area.w));
    out.height = Math.max(1, Math.round(area.h));
    const ctx = out.getContext("2d");
    if (!ctx) return file;
    ctx.translate(-Math.round(area.x), -Math.round(area.y));
    drawScene(ctx, stroke);
    ctx.setTransform(1, 0, 0, 1, 0, 0);

    const mime = outputMime(file.type);
    const blob = await new Promise<Blob | null>((resolve) =>
      out.toBlob(resolve, mime, mime === "image/jpeg" ? 0.92 : undefined),
    );
    if (!blob) return file;
    return new File([blob], outputFilename(file.name, mime), {
      type: mime,
      lastModified: Date.now(),
    });
  }

  async function accept() {
    if (flattening) return;
    flattening = true;
    try {
      finish(await flatten());
    } catch {
      finish(file);
    }
  }

  function reset() {
    if (shapes.length === 0 && !crop) return;
    snapshot();
    shapes = [];
    crop = null;
    render();
  }
</script>

<svelte:window onkeydown={onKey} />

{#if phase === "prompt"}
  <!-- Deliberately a chip, not a modal: it appears next to nothing, blocks
       nothing, and expires on its own. -->
  <div class="ann-prompt" role="status">
    <span class="ann-prompt__text">Annotate before upload?</span>
    <button type="button" class="ann-prompt__go" onclick={openEditor}>
      Annotate <kbd>Enter</kbd>
    </button>
    <button
      type="button"
      class="ann-prompt__skip"
      aria-label="Skip annotation and upload"
      onclick={() => finish(file)}
    >
      <X size={13} />
    </button>
    <span class="ann-prompt__bar" style:transform={`scaleX(${1 - promptProgress})`}></span>
  </div>
{:else}
  <div
    bind:this={rootEl}
    class="ann"
    role="dialog"
    aria-modal="true"
    aria-label="Annotate image"
    tabindex="-1"
  >
    <div class="ann__bar">
      <div class="ann__group" role="group" aria-label="Tools">
        {#each TOOLS as item (item.id)}
          {@const Icon = item.icon}
          <button
            type="button"
            class="ann__tool"
            class:is-active={tool === item.id}
            title={item.label}
            aria-label={item.label}
            aria-pressed={tool === item.id}
            onclick={() => (tool = item.id)}
          >
            <Icon size={15} />
          </button>
        {/each}
      </div>

      <div class="ann__group" role="group" aria-label="Colour">
        {#each COLORS as swatch (swatch)}
          <button
            type="button"
            class="ann__swatch"
            class:is-active={color === swatch}
            style:background={swatch}
            title={`Use ${swatch}`}
            aria-label={`Use colour ${swatch}`}
            aria-pressed={color === swatch}
            onclick={() => (color = swatch)}
          ></button>
        {/each}
      </div>

      <div class="ann__group">
        <button
          type="button"
          class="ann__tool"
          title="Undo (Ctrl+Z)"
          aria-label="Undo"
          disabled={undoDepth === 0}
          onclick={undo}
        >
          <Undo2 size={15} />
        </button>
        <button
          type="button"
          class="ann__text-btn"
          disabled={shapes.length === 0 && !crop}
          onclick={reset}
        >
          Clear
        </button>
      </div>

      <span class="ann__hint">
        {tool === "crop"
          ? "Drag to set the crop, or drag a handle to adjust"
          : tool === "redact"
            ? "Drag over anything that should not leave this machine"
            : "Drag on the image to draw"}
      </span>
    </div>

    <div class="ann__stage">
      {#if image}
        <canvas
          bind:this={canvasEl}
          class="ann__canvas"
          style:width={`${displayW}px`}
          style:height={`${displayH}px`}
          onpointerdown={onPointerDown}
          onpointermove={onPointerMove}
          onpointerup={onPointerUp}
          onpointercancel={onPointerUp}
        ></canvas>
      {:else}
        <p class="ann__loading">Loading image…</p>
      {/if}
    </div>

    <div class="ann__footer">
      <span class="ann__meta">
        {file.name} · {imageSize.w}×{imageSize.h}
      </span>
      <button type="button" class="ann__cancel" onclick={() => finish(file)}>
        Cancel <kbd>Esc</kbd>
      </button>
      <button type="button" class="ann__accept" disabled={flattening} onclick={accept}>
        {flattening ? "Preparing…" : "Upload"} <kbd>Enter</kbd>
      </button>
    </div>
  </div>
{/if}

<style>
  /* ── prompt chip ───────────────────────────────────────── */
  .ann-prompt {
    position: fixed;
    left: 50%;
    bottom: 1.5rem;
    z-index: 200;
    display: flex;
    align-items: center;
    gap: 0.5rem;
    padding: 0.4375rem 0.5rem 0.4375rem 0.75rem;
    transform: translateX(-50%);
    border: 1px solid var(--border);
    border-radius: 0.625rem;
    background: var(--surface);
    box-shadow: 0 10px 30px rgba(0, 0, 0, 0.18);
    overflow: hidden;
    animation: ann-rise 0.16s var(--ease-out-expo);
  }
  .ann-prompt__text {
    font-size: var(--text-body-sm);
    color: var(--text);
  }
  .ann-prompt__go {
    display: inline-flex;
    align-items: center;
    gap: 0.375rem;
    padding: 0.25rem 0.5rem;
    border: 0;
    border-radius: 0.375rem;
    background: var(--accent);
    color: var(--accent-text);
    font-size: var(--text-caption);
    font-weight: 500;
    cursor: pointer;
  }
  .ann-prompt__skip {
    display: inline-flex;
    align-items: center;
    justify-content: center;
    width: 1.375rem;
    height: 1.375rem;
    border: 0;
    border-radius: 0.375rem;
    background: transparent;
    color: var(--text-faint);
    cursor: pointer;
  }
  .ann-prompt__skip:hover {
    background: var(--bg-subtle);
    color: var(--text);
  }
  .ann-prompt__bar {
    position: absolute;
    left: 0;
    bottom: 0;
    width: 100%;
    height: 2px;
    background: var(--accent);
    transform-origin: left center;
  }

  /* ── editor ────────────────────────────────────────────── */
  .ann {
    position: fixed;
    inset: 0;
    z-index: 200;
    display: flex;
    flex-direction: column;
    background: var(--bg);
    outline: none;
  }

  .ann__bar {
    display: flex;
    flex-wrap: wrap;
    align-items: center;
    gap: 0.75rem;
    padding: 0.5rem 0.75rem;
    border-bottom: 1px solid var(--border);
    background: var(--surface);
  }
  .ann__group {
    display: inline-flex;
    align-items: center;
    gap: 0.125rem;
    padding: 0.1875rem;
    border: 1px solid var(--border);
    border-radius: 0.5rem;
    background: var(--bg-subtle);
  }
  .ann__tool {
    display: inline-flex;
    align-items: center;
    justify-content: center;
    width: 1.875rem;
    height: 1.875rem;
    border: 0;
    border-radius: 0.375rem;
    background: transparent;
    color: var(--text-muted);
    cursor: pointer;
    transition:
      background 0.15s var(--ease-out-expo),
      color 0.15s var(--ease-out-expo);
  }
  .ann__tool:hover:not(:disabled) {
    background: var(--surface);
    color: var(--accent);
  }
  .ann__tool.is-active {
    background: var(--accent);
    color: var(--accent-text);
  }
  .ann__tool:disabled {
    opacity: 0.45;
    cursor: not-allowed;
  }
  .ann__swatch {
    width: 1.375rem;
    height: 1.375rem;
    margin: 0.25rem;
    border: 2px solid transparent;
    border-radius: 999px;
    cursor: pointer;
    box-shadow: 0 0 0 1px var(--border);
  }
  .ann__swatch.is-active {
    border-color: var(--surface);
    box-shadow: 0 0 0 2px var(--text);
  }
  .ann__text-btn {
    padding: 0 0.5rem;
    height: 1.875rem;
    border: 0;
    border-radius: 0.375rem;
    background: transparent;
    color: var(--text-muted);
    font-size: var(--text-caption);
    cursor: pointer;
  }
  .ann__text-btn:hover:not(:disabled) {
    background: var(--surface);
    color: var(--text);
  }
  .ann__text-btn:disabled {
    opacity: 0.45;
    cursor: not-allowed;
  }
  .ann__hint {
    margin-left: auto;
    font-size: var(--text-caption);
    color: var(--text-faint);
  }

  .ann__stage {
    flex: 1;
    display: flex;
    align-items: center;
    justify-content: center;
    min-height: 0;
    padding: 1rem;
    overflow: auto;
    background:
      repeating-conic-gradient(var(--bg-subtle) 0% 25%, var(--bg) 0% 50%) 50% / 20px 20px;
  }
  .ann__canvas {
    display: block;
    touch-action: none;
    cursor: crosshair;
    border-radius: 0.25rem;
    box-shadow: 0 8px 28px rgba(0, 0, 0, 0.22);
  }
  .ann__loading {
    font-size: var(--text-body-sm);
    color: var(--text-faint);
  }

  .ann__footer {
    display: flex;
    align-items: center;
    gap: 0.5rem;
    padding: 0.625rem 0.75rem;
    border-top: 1px solid var(--border);
    background: var(--surface);
  }
  .ann__meta {
    flex: 1;
    min-width: 0;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
    font-size: var(--text-caption);
    color: var(--text-faint);
  }
  .ann__cancel,
  .ann__accept {
    display: inline-flex;
    align-items: center;
    gap: 0.375rem;
    padding: 0.375rem 0.75rem;
    border-radius: 0.375rem;
    font-size: var(--text-body-sm);
    font-weight: 500;
    cursor: pointer;
  }
  .ann__cancel {
    border: 1px solid var(--border);
    background: transparent;
    color: var(--text-muted);
  }
  .ann__cancel:hover {
    color: var(--text);
    border-color: var(--text-faint);
  }
  .ann__accept {
    border: 0;
    background: var(--accent);
    color: var(--accent-text);
  }
  .ann__accept:hover:not(:disabled) {
    background: var(--accent-hover);
  }
  .ann__accept:disabled {
    opacity: 0.6;
    cursor: not-allowed;
  }

  kbd {
    padding: 0.0625rem 0.25rem;
    border-radius: 0.1875rem;
    background: color-mix(in srgb, currentColor 14%, transparent);
    font-family: var(--font-mono);
    font-size: var(--text-micro);
  }

  @keyframes ann-rise {
    from {
      opacity: 0;
      transform: translate(-50%, 6px);
    }
    to {
      opacity: 1;
      transform: translate(-50%, 0);
    }
  }

  @media (prefers-reduced-motion: reduce) {
    .ann-prompt {
      animation: none;
    }
    .ann__tool,
    .ann__text-btn {
      transition: none;
    }
  }
</style>

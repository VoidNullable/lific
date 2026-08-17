// LIF-418: pure geometry + history helpers behind the screenshot annotator.
//
// AnnotateDialog.svelte owns the canvas and the pointer plumbing; every
// calculation it needs that can be reasoned about without a DOM lives here so
// it can be unit-tested directly (web/tests/annotateMath.test.ts).
//
// Coordinate convention: shapes are stored in *image* space (natural pixels of
// the source screenshot), never in CSS pixels. The canvas is displayed at some
// fit-to-viewport scale and pointer positions are divided by that scale on the
// way in. Keeping the model in image space means the flattened export is
// pixel-exact at full resolution regardless of the window size at edit time.

export interface Point {
  x: number;
  y: number;
}

export interface Rect {
  x: number;
  y: number;
  w: number;
  h: number;
}

export interface Size {
  w: number;
  h: number;
}

export function clamp(value: number, min: number, max: number): number {
  return value < min ? min : value > max ? max : value;
}

/** Rect spanned by two drag points, always with positive width/height so a
 *  right-to-left or bottom-to-top drag behaves like any other. */
export function normalizeRect(a: Point, b: Point): Rect {
  const x = Math.min(a.x, b.x);
  const y = Math.min(a.y, b.y);
  return { x, y, w: Math.abs(a.x - b.x), h: Math.abs(a.y - b.y) };
}

/** Clip a rect to the image bounds, collapsing to zero size when it falls
 *  entirely outside. Used before every pixelate pass so drawImage never gets
 *  a source rectangle the browser would reject. */
export function clampRect(rect: Rect, bounds: Size): Rect {
  const left = clamp(rect.x, 0, bounds.w);
  const top = clamp(rect.y, 0, bounds.h);
  const right = clamp(rect.x + rect.w, 0, bounds.w);
  const bottom = clamp(rect.y + rect.h, 0, bounds.h);
  return { x: left, y: top, w: Math.max(0, right - left), h: Math.max(0, bottom - top) };
}

/**
 * Block edge (in image pixels) used to redact a region.
 *
 * Scaled off the region's short side so a small redaction over a single word
 * still loses its glyphs instead of getting a cosmetic 4px mosaic, and a large
 * one does not turn into four giant squares. Floored at 8px: below that,
 * downscale-then-upscale leaves enough structure for text to be legible, which
 * would make the redaction a lie.
 */
export function pixelBlockSize(rect: Rect): number {
  const short = Math.min(Math.abs(rect.w), Math.abs(rect.h));
  return clamp(Math.round(short / 4), 8, 48);
}

export interface PixelateSteps {
  /** Source rect to sample, already clipped to the image. */
  source: Rect;
  /** Intermediate buffer size: the region at one pixel per block. */
  small: Size;
  /** Destination rect the upscaled buffer is painted back into. */
  dest: Rect;
  block: number;
}

/**
 * The two drawImage passes that implement redaction: downsample the region to
 * one pixel per block, then blow it back up with image smoothing disabled.
 *
 * This is destructive by construction. The intermediate buffer physically
 * cannot hold more than `small.w * small.h` samples, so the original pixels are
 * gone from the flattened output. That is the whole point: an overlay rectangle
 * can be peeled off a PNG, an averaged-down region cannot.
 *
 * Returns null when the region is degenerate (zero area after clipping).
 */
export function pixelateSteps(rect: Rect, bounds: Size): PixelateSteps | null {
  const source = clampRect(rect, bounds);
  if (source.w < 1 || source.h < 1) return null;
  const block = pixelBlockSize(source);
  const small: Size = {
    w: Math.max(1, Math.floor(source.w / block)),
    h: Math.max(1, Math.floor(source.h / block)),
  };
  return { source, small, dest: { ...source }, block };
}

/** Stroke width for pens, arrows and outlines, derived once from the image so
 *  a 4K screenshot does not get hairlines and a 320px thumbnail does not get
 *  clobbered. Deliberately not user-adjustable. */
export function strokeWidthFor(size: Size): number {
  const short = Math.min(size.w, size.h);
  return clamp(Math.round(short / 200), 3, 10);
}

/** Scale that fits an image inside the available viewport box, never
 *  upscaling past 1:1 (a small screenshot blown up would just be blurry). */
export function fitScale(image: Size, available: Size): number {
  if (image.w <= 0 || image.h <= 0) return 1;
  return Math.min(1, available.w / image.w, available.h / image.h);
}

export type CropHandle = "nw" | "n" | "ne" | "e" | "se" | "s" | "sw" | "w";

/**
 * Move one edge or corner of the crop rect to `point`, keeping the rect inside
 * the image and never letting it collapse below `min` on either axis. Dragging
 * the west handle past the east edge pins it at `east - min` rather than
 * flipping the rect inside out.
 */
export function resizeCrop(
  rect: Rect,
  handle: CropHandle,
  point: Point,
  bounds: Size,
  min = 16,
): Rect {
  let left = rect.x;
  let top = rect.y;
  let right = rect.x + rect.w;
  let bottom = rect.y + rect.h;
  const px = clamp(point.x, 0, bounds.w);
  const py = clamp(point.y, 0, bounds.h);

  if (handle.includes("w")) left = clamp(Math.min(px, right - min), 0, bounds.w);
  if (handle.includes("e")) right = clamp(Math.max(px, left + min), 0, bounds.w);
  if (handle.includes("n")) top = clamp(Math.min(py, bottom - min), 0, bounds.h);
  if (handle.includes("s")) bottom = clamp(Math.max(py, top + min), 0, bounds.h);

  return { x: left, y: top, w: Math.max(0, right - left), h: Math.max(0, bottom - top) };
}

export interface UndoStack<T> {
  /** Record a state that `undo()` can return to. */
  push: (snapshot: T) => void;
  /** Pop the most recent snapshot, or undefined when the stack is empty. */
  undo: () => T | undefined;
  canUndo: () => boolean;
  size: () => number;
  clear: () => void;
}

/**
 * Bounded snapshot history. Every mutating gesture pushes the state that
 * preceded it, so undo is "restore the previous snapshot" rather than "invert
 * the last operation" — which matters because crop, redact and freehand invert
 * in wildly different ways.
 *
 * Oldest entries are dropped past `limit` so a long pen session cannot pin
 * dozens of full shape arrays in memory.
 */
export function createUndoStack<T>(limit = 60): UndoStack<T> {
  const stack: T[] = [];
  return {
    push(snapshot) {
      stack.push(snapshot);
      if (stack.length > limit) stack.splice(0, stack.length - limit);
    },
    undo() {
      return stack.pop();
    },
    canUndo() {
      return stack.length > 0;
    },
    size() {
      return stack.length;
    },
    clear() {
      stack.length = 0;
    },
  };
}

/** JPEG survives as JPEG (re-encoding a photo to PNG bloats it); everything
 *  else flattens to PNG, which is what a screenshot wants anyway. */
export function outputMime(sourceMime: string): "image/jpeg" | "image/png" {
  return sourceMime === "image/jpeg" || sourceMime === "image/jpg" ? "image/jpeg" : "image/png";
}

/** Rename the annotated result so the original and the marked-up copy are
 *  distinguishable in an attachment list. */
export function outputFilename(sourceName: string, mime: string): string {
  const ext = mime === "image/jpeg" ? "jpg" : "png";
  const dot = sourceName.lastIndexOf(".");
  const stem = dot > 0 ? sourceName.slice(0, dot) : sourceName || "image";
  return `${stem}-annotated.${ext}`;
}

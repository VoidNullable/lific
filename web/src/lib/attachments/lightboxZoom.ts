// Zoom and pan math for the image lightbox. Pure, so it can be tested
// without a browser; ImageLightbox.svelte owns the pointer events.
//
// A view is the image's scale relative to its natural pixel size, plus an
// offset (x, y) of its centre from the centre of the viewport, in CSS pixels.

export interface Size {
  width: number;
  height: number;
}

export interface View {
  scale: number;
  x: number;
  y: number;
}

/** Furthest the lightbox zooms in, as a multiple of natural size. */
export const MAX_SCALE = 8;

/** The scale that fits the whole image in the viewport, never above 100%:
 *  a small image is shown at its own size rather than blown up. */
export function fitScale(image: Size, viewport: Size): number {
  if (image.width <= 0 || image.height <= 0) return 1;
  return Math.min(1, viewport.width / image.width, viewport.height / image.height);
}

export function fitView(image: Size, viewport: Size): View {
  return { scale: fitScale(image, viewport), x: 0, y: 0 };
}

/** Keep the image covering the viewport where it is larger than it, and
 *  centred along any axis where it is smaller. */
export function clampView(view: View, image: Size, viewport: Size): View {
  const slackX = Math.max(0, (image.width * view.scale - viewport.width) / 2);
  const slackY = Math.max(0, (image.height * view.scale - viewport.height) / 2);
  return {
    scale: view.scale,
    x: slackX === 0 ? 0 : Math.min(slackX, Math.max(-slackX, view.x)),
    y: slackY === 0 ? 0 : Math.min(slackY, Math.max(-slackY, view.y)),
  };
}

/**
 * Zoom by `factor` about `point` (relative to the viewport centre), keeping
 * the image pixel under the point where it is. The scale stays between the
 * fit scale and {@link MAX_SCALE}.
 */
export function zoomAt(
  view: View,
  factor: number,
  point: { x: number; y: number },
  image: Size,
  viewport: Size,
): View {
  const min = fitScale(image, viewport);
  const scale = Math.min(MAX_SCALE, Math.max(min, view.scale * factor));
  const ratio = scale / view.scale;
  return clampView(
    {
      scale,
      x: point.x - (point.x - view.x) * ratio,
      y: point.y - (point.y - view.y) * ratio,
    },
    image,
    viewport,
  );
}

/** What a click does: from the fitted view, zoom to 100% (or to 200% for an
 *  image that already fits at its own size) about the click; from any zoomed
 *  view, go back to fit. */
export function toggleZoom(
  view: View,
  point: { x: number; y: number },
  image: Size,
  viewport: Size,
): View {
  const fit = fitScale(image, viewport);
  if (view.scale > fit + 1e-6) return fitView(image, viewport);
  const target = fit < 1 ? 1 : 2;
  return zoomAt(view, target / view.scale, point, image, viewport);
}

/** Move by a drag delta, within bounds. */
export function panBy(view: View, dx: number, dy: number, image: Size, viewport: Size): View {
  return clampView({ ...view, x: view.x + dx, y: view.y + dy }, image, viewport);
}

/** Step `index` by `delta` through `count` items, wrapping at both ends. */
export function stepIndex(index: number, delta: number, count: number): number {
  if (count <= 0) return 0;
  return (((index + delta) % count) + count) % count;
}

// LIF-418: client-side image downscaling offer.
//
// A modern phone screenshot or a 6000px export routinely lands between "wildly
// bigger than anyone will ever view it at" and "the server refuses it". Before
// LIF-418 the only feedback was a red chip reading "file too large" after the
// whole payload had already crossed the wire.
//
// So: measure the image before uploading, and when it is either enormous in
// pixels or close to the instance's byte cap, offer a resize on the pending
// chip. The decision itself is a pure function (`decideDownscale`) so the
// thresholds are unit tested; the canvas work lives behind `downscaleImage`
// and is the only part that needs a browser.
//
// The cap is discoverable: the server rejects oversize uploads with
// "file too large: N bytes (max M)", and `parseUploadCap` lifts M out of that
// message so the next offer uses the real number instead of the 10 MiB default.

/** Server default (`AttachmentConfig::max_bytes`). Overridden at runtime the
 *  first time an upload comes back with a cap in its error message. */
export const DEFAULT_UPLOAD_CAP_BYTES = 10 * 1024 * 1024;

/** Long edge above which we offer a resize regardless of byte size. */
export const OFFER_EDGE_PX = 4096;

/** Fraction of the server cap above which we offer a resize. */
export const OFFER_CAP_FRACTION = 0.8;

/** Long edge we resize down to. Comfortably above any retina viewport width
 *  the app renders an inline image at, so the attachment still looks sharp. */
export const DOWNSCALE_EDGE_PX = 2560;

/** Re-encode quality for the lossy formats. */
export const DOWNSCALE_QUALITY = 0.85;

export type DownscaleReason = "dimensions" | "size";

export interface ImageFacts {
  width: number;
  height: number;
  bytes: number;
  mime: string;
}

export interface Dimensions {
  width: number;
  height: number;
}

export interface DownscaleOffer {
  /** Why we are offering: over the byte cap, or just enormous. */
  reason: DownscaleReason;
  /** Long edge of the resized image. */
  targetEdge: number;
  /** Resized pixel dimensions. */
  width: number;
  height: number;
  /** Rough post-resize byte size, for the "~1.2 MB" hint on the chip. */
  estimatedBytes: number;
  /** Mime the canvas will encode to. */
  outputMime: string;
}

/** Which mime a resized copy would carry. Type is preserved where the canvas
 *  can encode it; anything else (gif, svg, avif, heic) is left alone because a
 *  silent format change is worse than a big file. */
export function outputMimeFor(mime: string): string | null {
  const m = mime.toLowerCase();
  if (m === "image/jpeg" || m === "image/jpg") return "image/jpeg";
  if (m === "image/webp") return "image/webp";
  if (m === "image/png") return "image/png";
  return null;
}

/** Fit `width`x`height` inside a square of `edge` px, preserving aspect. */
export function scaledDimensions(width: number, height: number, edge: number): Dimensions {
  const long = Math.max(width, height);
  if (long <= 0 || long <= edge) return { width, height };
  const ratio = edge / long;
  return {
    width: Math.max(1, Math.round(width * ratio)),
    height: Math.max(1, Math.round(height * ratio)),
  };
}

/** Rough byte estimate for the resized copy. Encoded size tracks pixel count
 *  closely enough for a "~size" hint; lossy formats get a small extra discount
 *  for the re-encode at DOWNSCALE_QUALITY. */
export function estimateDownscaledBytes(facts: ImageFacts, target: Dimensions): number {
  const srcPixels = facts.width * facts.height;
  const dstPixels = target.width * target.height;
  if (srcPixels <= 0) return facts.bytes;
  const ratio = Math.min(1, dstPixels / srcPixels);
  const lossless = outputMimeFor(facts.mime) === "image/png";
  const encoding = lossless ? 1 : 0.9;
  return Math.max(1024, Math.round(facts.bytes * ratio * encoding));
}

/** Decide whether to offer a resize for this image. Returns null when the
 *  upload should just go as-is. */
export function decideDownscale(
  facts: ImageFacts,
  cap: number = DEFAULT_UPLOAD_CAP_BYTES,
): DownscaleOffer | null {
  const outputMime = outputMimeFor(facts.mime);
  if (!outputMime) return null;
  if (facts.width <= 0 || facts.height <= 0) return null;

  const longEdge = Math.max(facts.width, facts.height);
  // Nothing to gain: resizing to 2560 would upscale or no-op, so the offer
  // would be a lie. An oversize-but-small image still fails server-side with
  // the exact reason, which is the honest outcome.
  if (longEdge <= DOWNSCALE_EDGE_PX) return null;

  const overSize = facts.bytes > cap * OFFER_CAP_FRACTION;
  const overEdge = longEdge > OFFER_EDGE_PX;
  if (!overSize && !overEdge) return null;

  const target = scaledDimensions(facts.width, facts.height, DOWNSCALE_EDGE_PX);
  return {
    reason: overSize ? "size" : "dimensions",
    targetEdge: DOWNSCALE_EDGE_PX,
    width: target.width,
    height: target.height,
    estimatedBytes: estimateDownscaledBytes(facts, target),
    outputMime,
  };
}

/** Lift the byte cap out of a server rejection. Handles the Lific wording
 *  ("file too large: 12345 bytes (max 10485760)") and the generic proxy 413
 *  phrasing ("maximum 10485760 bytes"). Returns null when no number is there. */
export function parseUploadCap(message: string): number | null {
  const paren = message.match(/\(\s*max(?:imum)?[:\s]+(\d+)/i);
  if (paren) return Number(paren[1]);
  const bare = message.match(/max(?:imum)?(?:\s+size)?[:\s]+(\d+)\s*bytes/i);
  if (bare) return Number(bare[1]);
  return null;
}

// ── Browser side ─────────────────────────────────────────────

/** Measure an image file. Resolves null when the browser cannot decode it
 *  (corrupt bytes, or a format with no decoder), which simply means no offer. */
export async function readImageFacts(file: File): Promise<ImageFacts | null> {
  if (!file.type.startsWith("image/")) return null;
  const url = URL.createObjectURL(file);
  try {
    const size = await new Promise<Dimensions | null>((resolve) => {
      const img = new Image();
      img.onload = () => resolve({ width: img.naturalWidth, height: img.naturalHeight });
      img.onerror = () => resolve(null);
      img.src = url;
    });
    if (!size) return null;
    return { width: size.width, height: size.height, bytes: file.size, mime: file.type };
  } finally {
    URL.revokeObjectURL(url);
  }
}

/** Draw `file` into a canvas at the offer's dimensions and re-encode. Returns
 *  the original file untouched if anything fails, so a browser quirk can never
 *  block an upload. */
export async function downscaleImage(file: File, offer: DownscaleOffer): Promise<File> {
  const url = URL.createObjectURL(file);
  try {
    const img = await new Promise<HTMLImageElement | null>((resolve) => {
      const el = new Image();
      el.onload = () => resolve(el);
      el.onerror = () => resolve(null);
      el.src = url;
    });
    if (!img) return file;

    const canvas = document.createElement("canvas");
    canvas.width = offer.width;
    canvas.height = offer.height;
    const ctx = canvas.getContext("2d");
    if (!ctx) return file;
    ctx.drawImage(img, 0, 0, offer.width, offer.height);

    const blob = await new Promise<Blob | null>((resolve) => {
      canvas.toBlob((b) => resolve(b), offer.outputMime, DOWNSCALE_QUALITY);
    });
    // A resize that ends up bigger than the original (small PNGs re-encode
    // badly) is a regression, so keep whichever is smaller.
    if (!blob || blob.size >= file.size) return file;
    return new File([blob], file.name, { type: offer.outputMime, lastModified: Date.now() });
  } catch {
    return file;
  } finally {
    URL.revokeObjectURL(url);
  }
}

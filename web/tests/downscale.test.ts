import { expect, test } from "bun:test";
import {
  DEFAULT_UPLOAD_CAP_BYTES,
  DOWNSCALE_EDGE_PX,
  decideDownscale,
  estimateDownscaledBytes,
  outputMimeFor,
  parseUploadCap,
  scaledDimensions,
  type ImageFacts,
} from "../src/lib/attachments/downscale";

const MB = 1024 * 1024;

function facts(over: Partial<ImageFacts> = {}): ImageFacts {
  return { width: 6000, height: 4000, bytes: 3 * MB, mime: "image/jpeg", ...over };
}

test("offers a resize for an image past the long-edge threshold", () => {
  const offer = decideDownscale(facts({ width: 6000, height: 4000, bytes: 2 * MB }));
  expect(offer).not.toBeNull();
  expect(offer?.reason).toBe("dimensions");
  expect(offer?.targetEdge).toBe(DOWNSCALE_EDGE_PX);
  expect(offer?.width).toBe(2560);
  expect(offer?.height).toBe(1707);
  expect(offer?.outputMime).toBe("image/jpeg");
});

test("offers a resize once the file passes 80% of the cap", () => {
  const cap = DEFAULT_UPLOAD_CAP_BYTES; // 10 MiB
  // Under 4096px, so only the byte rule can fire.
  const under = decideDownscale(facts({ width: 3000, height: 2000, bytes: 7 * MB }), cap);
  expect(under).toBeNull();

  const over = decideDownscale(facts({ width: 3000, height: 2000, bytes: 9 * MB }), cap);
  expect(over?.reason).toBe("size");
});

test("declines to offer when resizing would change nothing", () => {
  // Enormous file, but already smaller than the resize target.
  const offer = decideDownscale(facts({ width: 2000, height: 1200, bytes: 9 * MB }));
  expect(offer).toBeNull();
});

test("leaves ordinary images alone", () => {
  expect(decideDownscale(facts({ width: 1920, height: 1080, bytes: 400_000 }))).toBeNull();
  expect(decideDownscale(facts({ width: 3200, height: 1800, bytes: 2 * MB }))).toBeNull();
});

test("never offers for formats the canvas cannot re-encode", () => {
  const big = { width: 8000, height: 6000, bytes: 9 * MB };
  expect(decideDownscale({ ...big, mime: "image/gif" })).toBeNull();
  expect(decideDownscale({ ...big, mime: "image/svg+xml" })).toBeNull();
  expect(decideDownscale({ ...big, mime: "image/avif" })).toBeNull();
  expect(decideDownscale({ ...big, mime: "image/png" })).not.toBeNull();
  expect(decideDownscale({ ...big, mime: "image/webp" })).not.toBeNull();
});

test("ignores images with no measurable dimensions", () => {
  expect(decideDownscale(facts({ width: 0, height: 0, bytes: 9 * MB }))).toBeNull();
});

test("a smaller cap pulls the offer threshold down with it", () => {
  const small = { width: 3000, height: 2000, bytes: 2 * MB, mime: "image/jpeg" };
  expect(decideDownscale(small, DEFAULT_UPLOAD_CAP_BYTES)).toBeNull();
  expect(decideDownscale(small, 2 * MB)?.reason).toBe("size");
});

test("output mime preserves the source type where the canvas can", () => {
  expect(outputMimeFor("image/jpeg")).toBe("image/jpeg");
  expect(outputMimeFor("image/JPG")).toBe("image/jpeg");
  expect(outputMimeFor("image/png")).toBe("image/png");
  expect(outputMimeFor("image/webp")).toBe("image/webp");
  expect(outputMimeFor("image/heic")).toBeNull();
});

test("scaled dimensions keep the aspect ratio and never upscale", () => {
  expect(scaledDimensions(6000, 4000, 2560)).toEqual({ width: 2560, height: 1707 });
  expect(scaledDimensions(4000, 6000, 2560)).toEqual({ width: 1707, height: 2560 });
  expect(scaledDimensions(800, 600, 2560)).toEqual({ width: 800, height: 600 });
});

test("the size estimate tracks pixel count and stays plausible", () => {
  const source = facts({ width: 6000, height: 4000, bytes: 12 * MB });
  const target = scaledDimensions(source.width, source.height, DOWNSCALE_EDGE_PX);
  const estimate = estimateDownscaledBytes(source, target);
  expect(estimate).toBeGreaterThan(0);
  expect(estimate).toBeLessThan(source.bytes);
  // ~18% of the pixels, discounted a little more for the lossy re-encode.
  expect(estimate).toBeGreaterThan(1.5 * MB);
  expect(estimate).toBeLessThan(2.5 * MB);
});

test("reads the byte cap out of the server's rejection", () => {
  expect(parseUploadCap("file too large: 12345678 bytes (max 10485760)")).toBe(10485760);
  expect(parseUploadCap("Payload too large, maximum 5242880 bytes")).toBe(5242880);
  expect(parseUploadCap("upload rate limit exceeded")).toBeNull();
});

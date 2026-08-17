import { describe, expect, test } from "bun:test";
import {
  clampRect,
  createUndoStack,
  fitScale,
  normalizeRect,
  outputFilename,
  outputMime,
  pixelBlockSize,
  pixelateSteps,
  resizeCrop,
  strokeWidthFor,
} from "../src/lib/attachments/annotateMath";

describe("rect helpers", () => {
  test("normalizes a drag in any direction to a positive rect", () => {
    expect(normalizeRect({ x: 40, y: 30 }, { x: 10, y: 5 })).toEqual({
      x: 10,
      y: 5,
      w: 30,
      h: 25,
    });
  });

  test("clips a rect that overhangs the image", () => {
    expect(clampRect({ x: -20, y: 10, w: 100, h: 500 }, { w: 60, h: 80 })).toEqual({
      x: 0,
      y: 10,
      w: 60,
      h: 70,
    });
  });

  test("collapses a rect that lies entirely outside the image", () => {
    expect(clampRect({ x: 200, y: 200, w: 50, h: 50 }, { w: 100, h: 100 })).toEqual({
      x: 100,
      y: 100,
      w: 0,
      h: 0,
    });
  });
});

describe("pixelation region math", () => {
  test("block size scales with the region's short side, within bounds", () => {
    expect(pixelBlockSize({ x: 0, y: 0, w: 400, h: 80 })).toBe(20);
    // Floored so a tiny redaction still destroys glyphs.
    expect(pixelBlockSize({ x: 0, y: 0, w: 12, h: 9 })).toBe(8);
    // Capped so a full-image redaction is not four giant squares.
    expect(pixelBlockSize({ x: 0, y: 0, w: 4000, h: 3000 })).toBe(48);
  });

  test("downsamples to one pixel per block and paints back over the region", () => {
    const steps = pixelateSteps({ x: 10, y: 20, w: 400, h: 80 }, { w: 1000, h: 1000 });
    expect(steps).not.toBeNull();
    expect(steps!.block).toBe(20);
    expect(steps!.small).toEqual({ w: 20, h: 4 });
    expect(steps!.source).toEqual({ x: 10, y: 20, w: 400, h: 80 });
    expect(steps!.dest).toEqual({ x: 10, y: 20, w: 400, h: 80 });
  });

  test("the intermediate buffer holds far fewer samples than the region", () => {
    const region = { x: 0, y: 0, w: 600, h: 200 };
    const steps = pixelateSteps(region, { w: 600, h: 200 })!;
    const kept = steps.small.w * steps.small.h;
    // Redaction is only real if the information is physically gone.
    expect(kept).toBeLessThan((region.w * region.h) / 100);
  });

  test("clips the sampled region to the image before sampling", () => {
    const steps = pixelateSteps({ x: -50, y: -50, w: 200, h: 200 }, { w: 100, h: 100 })!;
    expect(steps.source).toEqual({ x: 0, y: 0, w: 100, h: 100 });
  });

  test("returns null for a degenerate region", () => {
    expect(pixelateSteps({ x: 5, y: 5, w: 0, h: 40 }, { w: 100, h: 100 })).toBeNull();
    expect(pixelateSteps({ x: 500, y: 5, w: 40, h: 40 }, { w: 100, h: 100 })).toBeNull();
  });

  test("never produces a zero-dimension buffer for a sub-block region", () => {
    const steps = pixelateSteps({ x: 0, y: 0, w: 6, h: 3 }, { w: 100, h: 100 })!;
    expect(steps.small.w).toBeGreaterThanOrEqual(1);
    expect(steps.small.h).toBeGreaterThanOrEqual(1);
  });
});

describe("display scaling", () => {
  test("fits a large image into the viewport and never upscales a small one", () => {
    expect(fitScale({ w: 2000, h: 1000 }, { w: 1000, h: 1000 })).toBe(0.5);
    expect(fitScale({ w: 200, h: 100 }, { w: 1000, h: 1000 })).toBe(1);
  });

  test("stroke width tracks the image's short side within sane limits", () => {
    expect(strokeWidthFor({ w: 3840, h: 2160 })).toBe(10);
    expect(strokeWidthFor({ w: 1200, h: 800 })).toBe(4);
    expect(strokeWidthFor({ w: 120, h: 90 })).toBe(3);
  });
});

describe("crop handles", () => {
  const bounds = { w: 200, h: 100 };
  const rect = { x: 20, y: 20, w: 100, h: 50 };

  test("moves a single edge and leaves the others alone", () => {
    expect(resizeCrop(rect, "w", { x: 50, y: 999 }, bounds)).toEqual({
      x: 50,
      y: 20,
      w: 70,
      h: 50,
    });
  });

  test("moves both axes for a corner handle", () => {
    expect(resizeCrop(rect, "se", { x: 150, y: 90 }, bounds)).toEqual({
      x: 20,
      y: 20,
      w: 130,
      h: 70,
    });
  });

  test("pins the rect at the minimum instead of inverting it", () => {
    const flipped = resizeCrop(rect, "w", { x: 190, y: 40 }, bounds, 16);
    expect(flipped.w).toBe(16);
    expect(flipped.x).toBe(104);
  });

  test("keeps the rect inside the image", () => {
    const out = resizeCrop(rect, "ne", { x: 999, y: -999 }, bounds);
    expect(out).toEqual({ x: 20, y: 0, w: 180, h: 70 });
  });
});

describe("undo stack", () => {
  test("returns snapshots newest first and reports emptiness", () => {
    const stack = createUndoStack<string>();
    expect(stack.canUndo()).toBe(false);
    expect(stack.undo()).toBeUndefined();

    stack.push("a");
    stack.push("b");
    expect(stack.size()).toBe(2);
    expect(stack.undo()).toBe("b");
    expect(stack.undo()).toBe("a");
    expect(stack.canUndo()).toBe(false);
  });

  test("drops the oldest entries past the limit", () => {
    const stack = createUndoStack<number>(3);
    for (const n of [1, 2, 3, 4, 5]) stack.push(n);
    expect(stack.size()).toBe(3);
    expect([stack.undo(), stack.undo(), stack.undo()]).toEqual([5, 4, 3]);
    expect(stack.undo()).toBeUndefined();
  });

  test("clear empties the history", () => {
    const stack = createUndoStack<number>();
    stack.push(1);
    stack.clear();
    expect(stack.canUndo()).toBe(false);
  });
});

describe("output naming", () => {
  test("keeps jpeg as jpeg and flattens everything else to png", () => {
    expect(outputMime("image/jpeg")).toBe("image/jpeg");
    expect(outputMime("image/png")).toBe("image/png");
    expect(outputMime("image/webp")).toBe("image/png");
  });

  test("marks the annotated copy and fixes the extension", () => {
    expect(outputFilename("Screenshot 2026-08-17.png", "image/png")).toBe(
      "Screenshot 2026-08-17-annotated.png",
    );
    expect(outputFilename("photo.jpeg", "image/jpeg")).toBe("photo-annotated.jpg");
    expect(outputFilename("clipboard", "image/png")).toBe("clipboard-annotated.png");
  });
});

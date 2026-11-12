import { describe, expect, test } from "bun:test";
import {
  MAX_SCALE,
  clampView,
  fitScale,
  fitView,
  panBy,
  stepIndex,
  toggleZoom,
  zoomAt,
} from "../src/lib/attachments/lightboxZoom";

const big = { width: 4000, height: 2000 };
const screen = { width: 1000, height: 800 };
const centre = { x: 0, y: 0 };

describe("fit", () => {
  test("a large image fits the viewport", () => {
    expect(fitScale(big, screen)).toBe(0.25);
    expect(fitView(big, screen)).toEqual({ scale: 0.25, x: 0, y: 0 });
  });

  test("a small image is shown at its own size, not blown up", () => {
    expect(fitScale({ width: 200, height: 100 }, screen)).toBe(1);
  });
});

describe("click to zoom", () => {
  test("from fit a click goes to 100% about the click, and back", () => {
    const fit = fitView(big, screen);
    // A click 100 px right of centre lands on image pixel 400 right of centre.
    const zoomed = toggleZoom(fit, { x: 100, y: 0 }, big, screen);
    expect(zoomed.scale).toBe(1);
    // That pixel stays under the pointer: 400 px right of the image centre,
    // so the centre moves 300 px left.
    expect(zoomed.x).toBe(-300);
    expect(toggleZoom(zoomed, { x: 0, y: 0 }, big, screen)).toEqual(fit);
  });

  test("an image that already fits at 100% zooms to 200%", () => {
    const small = { width: 400, height: 300 };
    expect(toggleZoom(fitView(small, screen), centre, small, screen).scale).toBe(2);
  });
});

describe("wheel and pinch zoom", () => {
  test("stays between fit and the maximum", () => {
    const fit = fitView(big, screen);
    expect(zoomAt(fit, 0.1, centre, big, screen).scale).toBe(0.25);
    expect(zoomAt({ scale: 1, x: 0, y: 0 }, 100, centre, big, screen).scale).toBe(MAX_SCALE);
  });

  test("keeps the point under the pointer fixed", () => {
    const view = zoomAt({ scale: 1, x: 0, y: 0 }, 2, { x: 200, y: 100 }, big, screen);
    // Image pixel under (200, 100) was (200, 100) from the image centre; at
    // 2x it is at (400, 200), so the centre shifts by (-200, -100).
    expect(view).toEqual({ scale: 2, x: -200, y: -100 });
  });
});

describe("pan", () => {
  test("moves within the image's overhang and no further", () => {
    const view = { scale: 1, x: 0, y: 0 };
    expect(panBy(view, 500, 100, big, screen)).toEqual({ scale: 1, x: 500, y: 100 });
    // Overhang at 100%: (4000 - 1000) / 2 = 1500 across, (2000 - 800) / 2 = 600 down.
    expect(panBy(view, 9999, -9999, big, screen)).toEqual({ scale: 1, x: 1500, y: -600 });
  });

  test("an axis that fits stays centred", () => {
    expect(clampView({ scale: 0.25, x: 80, y: -40 }, big, screen)).toEqual({
      scale: 0.25,
      x: 0,
      y: 0,
    });
  });
});

describe("stepping", () => {
  test("wraps at both ends", () => {
    expect(stepIndex(0, -1, 3)).toBe(2);
    expect(stepIndex(2, 1, 3)).toBe(0);
    expect(stepIndex(1, 1, 3)).toBe(2);
    expect(stepIndex(0, 1, 0)).toBe(0);
  });
});

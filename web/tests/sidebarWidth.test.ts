import { afterEach, describe, expect, test } from "bun:test";
import { loadSidebarWidthPreference, saveSidebarWidth, sidebarSizing } from "../src/lib/sidebarWidth";

const originalStorage = Object.getOwnPropertyDescriptor(globalThis, "localStorage");
afterEach(() => {
  if (originalStorage) Object.defineProperty(globalThis, "localStorage", originalStorage);
  else Reflect.deleteProperty(globalThis, "localStorage");
});

function storage(initial?: string) {
  const values = new Map(initial === undefined ? [] : [["lific:sidebar:width", initial]]);
  Object.defineProperty(globalThis, "localStorage", { configurable: true, value: {
    getItem: (key: string) => values.get(key) ?? null,
    setItem: (key: string, value: string) => values.set(key, value),
    removeItem: (key: string) => values.delete(key),
  } });
  return values;
}

describe("sidebar text sizing and pixel preferences", () => {
  test("default follows text size without creating a manual preference", () => {
    const values = storage();
    const normal = sidebarSizing(loadSidebarWidthPreference(), 16);
    const large = sidebarSizing(loadSidebarWidthPreference(), 18);
    expect(large.width / normal.width).toBe(18 / 16);
    expect(large.min / normal.min).toBe(18 / 16);
    expect(values.size).toBe(0);
  });
  test("legacy pixel widths survive load, text changes and reload unchanged", () => {
    const values = storage("300");
    for (const root of [18, 15, 16, 18]) expect(sidebarSizing(loadSidebarWidthPreference(), root).width).toBe(300);
    expect(values.get("lific:sidebar:width")).toBe("300");
  });
  test("a temporary minimum restores the user's narrow width on return", () => {
    const values = storage("190");
    expect(sidebarSizing(loadSidebarWidthPreference(), 18).width).toBeGreaterThan(190);
    expect(values.get("lific:sidebar:width")).toBe("190");
    expect(sidebarSizing(loadSidebarWidthPreference(), 16).width).toBe(190);
  });
  test("deliberate resizing persists; reset returns to proportional defaults", () => {
    storage("190");
    const current = sidebarSizing(loadSidebarWidthPreference(), 18).width;
    saveSidebarWidth(current + 10);
    expect(loadSidebarWidthPreference()).toBe(current + 10);
    saveSidebarWidth(null);
    expect(loadSidebarWidthPreference()).toBeNull();
    expect(sidebarSizing(loadSidebarWidthPreference(), 18).width).toBeGreaterThan(sidebarSizing(null, 16).width);
  });
  test("invalid or unavailable storage and font sizes use safe defaults", () => {
    for (const value of ["", " ", "NaN", "no width", "Infinity"]) {
      storage(value);
      expect(loadSidebarWidthPreference()).toBeNull();
    }
    Object.defineProperty(globalThis, "localStorage", { configurable: true, get() { throw new Error("Unavailable"); } });
    expect(loadSidebarWidthPreference()).toBeNull();
    expect(() => saveSidebarWidth(null)).not.toThrow();
    expect(() => saveSidebarWidth(250)).not.toThrow();
    expect(sidebarSizing(null, NaN)).toEqual(sidebarSizing(null, 16));
  });
});

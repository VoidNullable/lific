import { afterEach, describe, expect, test } from "bun:test";
import {
  loadRightSidebarCollapsed,
  saveRightSidebarCollapsed,
} from "../src/lib/issues/rightSidebarPrefs";

const originalStorage = Object.getOwnPropertyDescriptor(globalThis, "localStorage");
afterEach(() => {
  if (originalStorage) Object.defineProperty(globalThis, "localStorage", originalStorage);
  else Reflect.deleteProperty(globalThis, "localStorage");
});

function storage(initial?: string) {
  const values = new Map(initial === undefined ? [] : [["lific:rightSidebar:collapsed", initial]]);
  Object.defineProperty(globalThis, "localStorage", { configurable: true, value: {
    getItem: (key: string) => values.get(key) ?? null,
    setItem: (key: string, value: string) => values.set(key, value),
    removeItem: (key: string) => values.delete(key),
  } });
  return values;
}

describe("right sidebar collapsed persistence", () => {
  test("defaults to expanded without a stored preference", () => {
    const values = storage();
    expect(loadRightSidebarCollapsed()).toBe(false);
    expect(values.size).toBe(0);
  });
  test("persists and reloads the collapsed state", () => {
    const values = storage();
    saveRightSidebarCollapsed(true);
    expect(loadRightSidebarCollapsed()).toBe(true);
    expect(values.get("lific:rightSidebar:collapsed")).toBe("1");
    saveRightSidebarCollapsed(false);
    expect(loadRightSidebarCollapsed()).toBe(false);
    expect(values.get("lific:rightSidebar:collapsed")).toBe("0");
  });
  test("anything but '1' reads as expanded", () => {
    for (const value of ["0", "", "true", "yes"]) {
      storage(value);
      expect(loadRightSidebarCollapsed()).toBe(false);
    }
  });
  test("unavailable storage never throws and stays expanded", () => {
    Object.defineProperty(globalThis, "localStorage", { configurable: true, get() { throw new Error("Unavailable"); } });
    expect(loadRightSidebarCollapsed()).toBe(false);
    expect(() => saveRightSidebarCollapsed(true)).not.toThrow();
  });
});

import { describe, expect, test } from "bun:test";
import { projectIcon } from "../src/lib/projectIcons";

describe("project icons", () => {
  test("accepts installed icons, emoji sequences, and the logo", () => {
    expect(Array.isArray(projectIcon("lucide:Terminal"))).toBe(true);
    expect(projectIcon("\u{1f680}")).toBe("\u{1f680}");
    expect(projectIcon("\u{1f469}\u200d\u{1f4bb}")).toBe("\u{1f469}\u200d\u{1f4bb}");
    expect(projectIcon("lific:logo")).toBe("lific:logo");
  });

  test("rejects arbitrary text and inherited properties instead of rendering them", () => {
    for (const value of [null, undefined, "", "Lucide:Terminal", "lucide:MissingIcon", "lucide:constructor", "lucide:__proto__", "constructor", "hello".repeat(1000)]) {
      expect(projectIcon(value)).toBeNull();
    }
  });
});

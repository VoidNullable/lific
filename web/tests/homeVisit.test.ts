import { describe, expect, test } from "bun:test";
import { nextVisit, VISIT_GAP_MS } from "../src/lib/home/lastVisit";
import { heat, importanceScore } from "../src/lib/issues/importance";

describe("home visit", () => {
  test("a first visit has nothing to compare against", () => {
    expect(nextVisit(null, 1_000)).toEqual({ seen: 1_000, since: null });
  });

  test("returning after a gap measures from the end of the last visit", () => {
    const prev = { seen: 1_000, since: null };
    expect(nextVisit(prev, 1_000 + VISIT_GAP_MS + 1)).toEqual({
      seen: 1_000 + VISIT_GAP_MS + 1,
      since: 1_000,
    });
  });

  test("views within one visit keep the same starting point", () => {
    const prev = { seen: 5_000, since: 1_000 };
    expect(nextVisit(prev, 5_000 + 60_000)).toEqual({ seen: 65_000, since: 1_000 });
  });
});

describe("importance", () => {
  const now = Date.parse("2026-10-08T12:00:00Z");
  const issue = (priority: string, status: string, createdDaysAgo: number, idleDays: number) => ({
    priority,
    status,
    created_at: new Date(now - createdDaysAgo * 86400000).toISOString().slice(0, 19).replace("T", " "),
    updated_at: new Date(now - idleDays * 86400000).toISOString().slice(0, 19).replace("T", " "),
  });

  test("an old idle urgent todo outranks a fresh one", () => {
    expect(importanceScore(issue("urgent", "todo", 40, 30), now)).toBeGreaterThan(
      importanceScore(issue("urgent", "todo", 0, 0), now),
    );
  });

  test("heat stays within 0..1 and rises with the score", () => {
    expect(heat(0)).toBe(0);
    expect(heat(-5)).toBe(0);
    expect(heat(500)).toBeLessThan(1);
    expect(heat(125)).toBeGreaterThan(heat(30));
  });
});

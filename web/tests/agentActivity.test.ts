import { describe, expect, test } from "bun:test";
import {
  SERIES,
  bucketLabel,
  bucketRange,
  countTicks,
  countNoun,
  isAgentWindow,
  isQuiet,
  labelIndices,
  lastSeenAgo,
  shortAgo,
  possessive,
  recentVerb,
  statusDetail,
} from "../src/lib/agents/activity";

const UTC = "UTC";

describe("bucket labels", () => {
  test("1h and 24h buckets read as local HH:MM", () => {
    expect(bucketLabel("2026-11-12T15:05:00Z", "1h", UTC)).toBe("15:05");
    expect(bucketLabel("2026-11-12T09:00:00Z", "24h", UTC)).toBe("09:00");
    expect(bucketLabel("2026-11-12T15:05:00Z", "1h", "America/Chicago")).toBe("09:05");
  });

  test("7d buckets read as the weekday", () => {
    expect(bucketLabel("2026-11-12T00:00:00Z", "7d", UTC)).toBe("Thu");
  });

  test("an unparseable start yields no label", () => {
    expect(bucketLabel("nope", "1h", UTC)).toBe("");
  });

  test("tooltip ranges end at the next bucket, or now for the last one", () => {
    expect(bucketRange("2026-11-12T10:05:00Z", "1h", 300, false, UTC)).toBe("10:05 to 10:10");
    expect(bucketRange("2026-11-12T23:00:00Z", "24h", 3600, false, UTC)).toBe("23:00 to 00:00");
    expect(bucketRange("2026-11-12T16:00:00Z", "1h", 300, true, UTC)).toBe("16:00 to now");
    expect(bucketRange("2026-11-12T00:00:00Z", "7d", 86400, false, UTC)).toBe("Thu, Nov 12");
    expect(bucketRange("2026-11-12T00:00:00Z", "7d", 86400, true, UTC)).toBe("Thu, Nov 12 (today)");
  });
});

describe("y-axis ticks", () => {
  test("whole, round steps that cover the max", () => {
    expect(countTicks(25)).toEqual([0, 10, 20, 30]);
    expect(countTicks(50)).toEqual([0, 20, 40, 60]);
    expect(countTicks(3)).toEqual([0, 1, 2, 3]);
    expect(countTicks(7)).toEqual([0, 2, 4, 6, 8]);
    expect(countTicks(400)).toEqual([0, 100, 200, 300, 400]);
  });

  test("an empty chart still gets a small axis", () => {
    expect(countTicks(0)).toEqual([0, 1, 2, 3, 4]);
  });
});

describe("label thinning", () => {
  test("small series label every bucket", () => {
    expect(labelIndices(7, 7)).toEqual([0, 1, 2, 3, 4, 5, 6]);
  });

  test("12 buckets label every other one, always including the newest", () => {
    expect(labelIndices(12)).toEqual([1, 3, 5, 7, 9, 11]);
  });

  test("24 buckets keep at most 8 labels", () => {
    const idx = labelIndices(24);
    expect(idx.length).toBeLessThanOrEqual(8);
    expect(idx[idx.length - 1]).toBe(23);
  });

  test("no buckets, no labels", () => {
    expect(labelIndices(0)).toEqual([]);
  });
});

describe("status copy", () => {
  const now = Date.parse("2026-11-12T16:00:00Z");
  const ago = (ms: number) => new Date(now - ms).toISOString();

  test("relative last seen", () => {
    expect(lastSeenAgo(ago(20_000), now)).toBe("just now");
    expect(lastSeenAgo(ago(38 * 60_000), now)).toBe("38 min ago");
    expect(lastSeenAgo(ago(2 * 3_600_000 + 5 * 60_000), now)).toBe("2 h ago");
    expect(lastSeenAgo(ago(30 * 3_600_000), now)).toBe("yesterday");
    expect(lastSeenAgo(ago(3 * 86_400_000), now)).toBe("3 days ago");
  });

  test("compact age for the recent feed", () => {
    expect(shortAgo(ago(20_000), now)).toBe("now");
    expect(shortAgo(ago(2 * 60_000), now)).toBe("2m");
    expect(shortAgo(ago(38 * 60_000), now)).toBe("38m");
    expect(shortAgo(ago(3 * 3_600_000), now)).toBe("3h");
    expect(shortAgo(ago(2 * 86_400_000), now)).toBe("2d");
  });

  test("active agents get no detail line", () => {
    expect(statusDetail("active", ago(60_000), now)).toBeNull();
  });

  test("idle and inactive agents say when they were last seen", () => {
    expect(statusDetail("idle", ago(38 * 60_000), now)).toBe("Last seen 38 min ago");
    expect(statusDetail("inactive", ago(2 * 3_600_000), now)).toBe("Last seen 2 h ago");
  });

  test("an agent that never acted says so", () => {
    expect(statusDetail("inactive", null, now)).toBe("No activity yet");
  });

  test("owner possessive", () => {
    expect(possessive("Blake")).toBe("Blake's");
    expect(possessive("Chris")).toBe("Chris'");
  });
});

describe("counts", () => {
  test("singular only for exactly one", () => {
    expect(countNoun("read", 1)).toBe("Read");
    expect(countNoun("read", 0)).toBe("Reads");
    expect(countNoun("close", 2)).toBe("Closes");
    expect(countNoun("create", 1)).toBe("Create");
  });

  test("quiet when every total is zero", () => {
    expect(isQuiet({ read: 0, edit: 0, create: 0, close: 0 })).toBe(true);
    expect(isQuiet({ read: 0, edit: 0, create: 0, close: 1 })).toBe(false);
  });

  test("series stack read, edit, create, close and never use the accent", () => {
    expect(SERIES.map((s) => s.key)).toEqual(["read", "edit", "create", "close"]);
    for (const s of SERIES) expect(s.color).not.toContain("--accent");
  });

  test("window guard", () => {
    expect(isAgentWindow("24h")).toBe(true);
    expect(isAgentWindow("30d")).toBe(false);
    expect(isAgentWindow(null)).toBe(false);
  });
});

describe("recent verbs", () => {
  const item = (kind: "read" | "edit" | "create" | "close", entity_type: string, detail: string | null = null) => ({
    kind,
    entity_type,
    detail,
  });

  test("issue actions need no noun", () => {
    expect(recentVerb(item("close", "issue", "status"))).toBe("closed");
    expect(recentVerb(item("create", "issue"))).toBe("created");
    expect(recentVerb(item("edit", "issue", "description"))).toBe("edited");
    expect(recentVerb(item("read", "issue"))).toBe("read");
  });

  test("details pick a sharper verb", () => {
    expect(recentVerb(item("edit", "issue", "comment"))).toBe("commented on");
    expect(recentVerb(item("edit", "page", "comment"))).toBe("commented on page");
    expect(recentVerb(item("edit", "issue", "label"))).toBe("relabeled");
    expect(recentVerb(item("edit", "issue", "link"))).toBe("linked");
    expect(recentVerb(item("edit", "module", "delete"))).toBe("deleted module");
    expect(recentVerb(item("edit", "issue", "status"))).toBe("changed status of");
  });

  test("other entities name themselves", () => {
    expect(recentVerb(item("read", "page"))).toBe("read page");
    expect(recentVerb(item("create", "plan"))).toBe("created plan");
    expect(recentVerb(item("read", "attachment"))).toBe("read file");
  });
});

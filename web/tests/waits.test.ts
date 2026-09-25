import { describe, expect, test } from "bun:test";
import type { IssueWait } from "../src/lib/api";
import {
  dayAfter,
  describeWait,
  formatWindow,
  localDay,
  summarizeWaits,
  waitState,
} from "../src/lib/issues/waits";

function dateWait(earliest: string, latest = earliest, note = ""): IssueWait {
  return {
    id: 1,
    issue_id: 1,
    kind: "date",
    earliest,
    latest,
    note,
    state: "holding",
    created_at: "2026-09-01 00:00:00",
  };
}

const userWait: IssueWait = {
  id: 2,
  issue_id: 1,
  kind: "user",
  user_id: 7,
  username: "blake",
  display_name: "Blake",
  note: "real-device check",
  state: "holding",
  created_at: "2026-09-01 00:00:00",
};

describe("wait state", () => {
  test("a date wait holds, comes due on its first day, and goes overdue after its last", () => {
    const wait = dateWait("2026-09-28", "2026-09-29");
    expect(waitState(wait, "2026-09-27")).toBe("holding");
    expect(waitState(wait, "2026-09-28")).toBe("due");
    expect(waitState(wait, "2026-09-29")).toBe("due");
    expect(waitState(wait, "2026-09-30")).toBe("overdue");
  });

  test("a user wait holds until cleared", () => {
    expect(waitState(userWait, "2099-01-01")).toBe("holding");
  });

  test("today is the local calendar day", () => {
    expect(localDay(new Date(2026, 8, 5, 23, 59))).toBe("2026-09-05");
    expect(dayAfter("2026-12-31")).toBe("2027-01-01");
  });
});

describe("wait text", () => {
  test("windows collapse a shared month", () => {
    const same = formatWindow("2026-09-28", "2026-09-29", 2026);
    expect(same).toContain("28");
    expect(same.endsWith(" to 29")).toBe(true);
    expect(formatWindow("2026-09-28", "2026-10-02", 2026)).toContain(" to ");
  });

  test("the detail headline names the state", () => {
    expect(describeWait(userWait).headline).toBe("Waiting on Blake (@blake)");
    expect(describeWait(dateWait("2026-09-28"), "2026-09-01").headline).toMatch(/^Waiting until /);
    expect(describeWait(dateWait("2026-09-28"), "2026-09-28").headline).toMatch(/^Due to check since /);
    expect(describeWait(dateWait("2026-09-28", "2026-09-29"), "2026-10-01").headline).toMatch(
      /^Overdue since .*, expected /,
    );
  });

  test("the chip shows the most pressing wait and counts the rest", () => {
    expect(summarizeWaits([], "2026-09-01")).toBeNull();
    const summary = summarizeWaits(
      [userWait, dateWait("2026-09-28", "2026-09-29", "filing office")],
      "2026-10-01",
    );
    expect(summary?.state).toBe("overdue");
    expect(summary?.label).toMatch(/^overdue /);
    expect(summary?.more).toBe(1);
    expect(summary?.title).toContain("(filing office)");
    expect(summary?.title).toContain("Waiting on Blake (@blake)");
    expect(summarizeWaits([userWait], "2026-10-01")?.label).toBe("@blake");
  });
});

import { describe, expect, test } from "bun:test";
import type { Activity } from "../src/lib/api";
import { byDay, describe as sentence, digest, excerpt, FOLD_GAP_MS } from "../src/lib/overview/activity";

let nextId = 1000;
function row(partial: Partial<Activity>): Activity {
  return {
    id: nextId--,
    ts: "2026-11-20 12:00:00",
    actor_user_id: 1,
    actor_username: "blake",
    actor_display_name: "Blake",
    actor_is_bot: false,
    transport: "web",
    entity_type: "issue",
    entity_id: 4,
    entity_label: "LIF-4",
    project_id: 1,
    issue_id: 4,
    page_id: null,
    action: "update",
    field: null,
    old_value: null,
    new_value: null,
    ...partial,
  };
}

function minutesBefore(ts: string, minutes: number): string {
  const t = new Date(ts.replace(" ", "T") + "Z").getTime() - minutes * 60_000;
  return new Date(t).toISOString().slice(0, 19).replace("T", " ");
}

describe("activity sentences", () => {
  test("status changes read as what happened to the issue", () => {
    expect(sentence(row({ field: "status", old_value: "todo", new_value: "done" })).verb).toBe("closed");
    expect(sentence(row({ field: "status", old_value: "done", new_value: "todo" })).verb).toBe("reopened");
    expect(sentence(row({ field: "status", old_value: "todo", new_value: "active" }))).toEqual({
      verb: "moved",
      target: "LIF-4",
      detail: "to Active",
    });
  });

  test("plan steps name the plan, not a raw entity type", () => {
    const step = row({ entity_type: "plan_step", entity_label: "LIF-PLAN-2", action: "create", issue_id: null });
    expect(sentence(step)).toEqual({ verb: "added a step to", target: "LIF-PLAN-2", detail: null });
    const done = row({ entity_type: "plan_step", entity_label: "LIF-PLAN-2", field: "done", old_value: "0", new_value: "1" });
    expect(sentence(done).verb).toBe("checked off a step in");
  });

  test("assignment keeps the shared any-person wording", () => {
    expect(sentence(row({ action: "assign", field: "assignee", new_value: "human" })).verb).toBe("marked for any person");
    expect(sentence(row({ action: "assign", field: "assignee", new_value: "@sam" })).detail).toBe("@sam");
  });

  test("a comment quotes its opening words on one line", () => {
    const long = row({ entity_type: "comment", action: "create", new_value: "First line\n\nsecond   line " + "x".repeat(200) });
    const quote = excerpt(long, 30)!;
    expect(quote.startsWith("First line second line")).toBe(true);
    expect(quote.length).toBeLessThanOrEqual(30);
    expect(quote.endsWith("…")).toBe(true);
    expect(excerpt(row({}))).toBeNull();
  });
});

describe("activity digest", () => {
  test("a run of edits to one issue folds into its most telling line", () => {
    const ts = "2026-11-20 12:00:00";
    const lines = digest([
      row({ ts, action: "attach", field: "label", new_value: "bug" }),
      row({ ts: minutesBefore(ts, 1), field: "status", old_value: "active", new_value: "done" }),
      row({ ts: minutesBefore(ts, 2), field: "title" }),
    ]);
    expect(lines).toHaveLength(1);
    expect(lines[0].verb).toBe("closed");
    expect(lines[0].count).toBe(3);
    expect(lines[0].ts).toBe(ts);
  });

  test("a comment and its issue fold together, plan steps fold into their plan", () => {
    const ts = "2026-11-20 12:00:00";
    const lines = digest([
      row({ ts, entity_type: "comment", entity_id: 9, action: "create", new_value: "On it" }),
      row({ ts, field: "status", old_value: "todo", new_value: "active" }),
      row({ ts, entity_type: "plan_step", entity_id: 3, entity_label: "LIF-PLAN-1", action: "create", issue_id: 4 }),
      row({ ts, entity_type: "plan_step", entity_id: 2, entity_label: "LIF-PLAN-1", action: "create", issue_id: null }),
      row({ ts, entity_type: "plan", entity_id: 1, entity_label: "LIF-PLAN-1", action: "create", issue_id: null }),
    ]);
    expect(lines.map((l) => [l.verb, l.count])).toEqual([
      ["moved", 2],
      ["created plan", 3],
    ]);
  });

  test("different people, subjects or a long gap keep lines apart", () => {
    const ts = "2026-11-20 12:00:00";
    const lines = digest([
      row({ ts, field: "title" }),
      row({ ts, field: "title", actor_user_id: 2, actor_display_name: "Sam" }),
      row({ ts, field: "title", actor_user_id: 2, actor_display_name: "Sam", issue_id: 5, entity_label: "LIF-5" }),
      row({ ts: minutesBefore(ts, FOLD_GAP_MS / 60_000 + 1), field: "title", actor_user_id: 2, issue_id: 5 }),
    ]);
    expect(lines).toHaveLength(4);
    expect(lines[1].actor).toBe("Sam");
  });

  test("days that recur in an id-ordered feed get distinct keys", () => {
    const now = new Date("2026-11-20T18:00:00Z");
    const lines = digest([
      row({ ts: "2026-11-20 12:00:00", issue_id: 1 }),
      row({ ts: "2026-11-18 12:00:00", issue_id: 2 }),
      row({ ts: "2026-11-20 11:00:00", issue_id: 3 }),
    ]);
    const days = byDay(lines, now);
    expect(days.map((d) => d.label)).toEqual(["Today", "Wed, Nov 18", "Today"]);
    expect(new Set(days.map((d) => d.key)).size).toBe(3);
  });
});

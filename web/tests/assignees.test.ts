import { describe, expect, test } from "bun:test";
import type { IssueAssignee } from "../src/lib/api";
import {
  assignActivity,
  assigneesIn,
  assignmentFields,
  assignmentKind,
  assignmentNames,
  describeAssignment,
  describeNames,
  initials,
  matchesAssigneeFilter,
  matchesPerson,
  normalizeAssigneeFilter,
  orderPeople,
  sameNames,
} from "../src/lib/issues/assignees";

const alice: IssueAssignee = { user_id: 2, username: "alice", display_name: "Alice Liddell" };
const bob: IssueAssignee = { user_id: 3, username: "bob" };

const agents = { needs_human: false };
const human = { needs_human: true };
const people = { needs_human: true, assignees: [alice, bob] };

describe("assignment state", () => {
  test("reads the three states from the wire shape", () => {
    expect(assignmentKind(agents)).toBe("agents");
    expect(assignmentKind({})).toBe("agents");
    expect(assignmentKind(human)).toBe("human");
    expect(assignmentKind(people)).toBe("people");
  });

  test("turns each state back into the write value", () => {
    expect(assignmentNames(agents)).toEqual([]);
    expect(assignmentNames(human)).toEqual(["human"]);
    expect(assignmentNames(people)).toEqual(["alice", "bob"]);
  });

  test("describes each state in words", () => {
    expect(describeAssignment(agents)).toBe("Any agent");
    expect(describeAssignment(human)).toBe("Any person");
    expect(describeAssignment(people)).toBe("Alice Liddell, bob");
    expect(describeNames([])).toBe("Any agent");
    expect(describeNames(["human"])).toBe("Any person");
    expect(describeNames(["alice", "carol"], [alice])).toBe("Alice Liddell, @carol");
  });

  test("builds local fields for a write", () => {
    expect(assignmentFields([])).toEqual({ needs_human: false, assignees: undefined });
    expect(assignmentFields(["human"])).toEqual({ needs_human: true, assignees: undefined });
    expect(assignmentFields(["ALICE"], [alice, bob])).toEqual({ needs_human: true, assignees: [alice] });
  });

  test("compares write values without caring about order or case", () => {
    expect(sameNames(["bob", "alice"], ["Alice", "bob"])).toBe(true);
    expect(sameNames([], ["human"])).toBe(false);
    expect(sameNames(["alice"], ["alice", "bob"])).toBe(false);
  });

  test("makes initials from names", () => {
    expect(initials("Alice Liddell")).toBe("AL");
    expect(initials("bob")).toBe("B");
    expect(initials("mary_jane-watson")).toBe("MJ");
  });
});

describe("picker candidates", () => {
  test("puts the signed-in person first and adds them when missing", () => {
    const me = { user_id: 9, username: "zed" };
    expect(orderPeople([bob, alice], me).map((p) => p.username)).toEqual(["zed", "alice", "bob"]);
    expect(orderPeople([bob, alice, me], me).map((p) => p.username)).toEqual(["zed", "alice", "bob"]);
    expect(orderPeople([bob, alice], null).map((p) => p.username)).toEqual(["alice", "bob"]);
  });

  test("matches by username or display name", () => {
    expect(matchesPerson(alice, "lidd")).toBe(true);
    expect(matchesPerson(alice, "@ali")).toBe(true);
    expect(matchesPerson(bob, "alice")).toBe(false);
    expect(matchesPerson(bob, "  ")).toBe(true);
  });
});

describe("list filter", () => {
  const me = { id: 2, username: "alice" };

  test("an empty filter matches everything", () => {
    expect(matchesAssigneeFilter(agents, "", me)).toBe(true);
  });

  test("none matches only unassigned issues", () => {
    expect(matchesAssigneeFilter(agents, "none", me)).toBe(true);
    expect(matchesAssigneeFilter(human, "none", me)).toBe(false);
    expect(matchesAssigneeFilter(people, "none", me)).toBe(false);
  });

  test("human matches every issue a person must do, like the REST filter", () => {
    expect(matchesAssigneeFilter(agents, "human", me)).toBe(false);
    expect(matchesAssigneeFilter(human, "human", me)).toBe(true);
    expect(matchesAssigneeFilter(people, "human", me)).toBe(true);
  });

  test("me matches issues that name the signed-in person", () => {
    expect(matchesAssigneeFilter(people, "me", me)).toBe(true);
    expect(matchesAssigneeFilter(people, "me", { id: 7, username: "carol" })).toBe(false);
    expect(matchesAssigneeFilter(human, "me", me)).toBe(false);
    expect(matchesAssigneeFilter(people, "me", null)).toBe(false);
  });

  test("@username matches issues that name that person, ignoring case", () => {
    expect(matchesAssigneeFilter(people, "@BOB", me)).toBe(true);
    expect(matchesAssigneeFilter(people, "@carol", me)).toBe(false);
  });

  test("stored values this build does not know are dropped", () => {
    expect(normalizeAssigneeFilter(undefined)).toBe("");
    expect(normalizeAssigneeFilter(42)).toBe("");
    expect(normalizeAssigneeFilter("bogus")).toBe("");
    expect(normalizeAssigneeFilter("@")).toBe("");
    for (const ok of ["", "none", "human", "me", "@alice"]) {
      expect(normalizeAssigneeFilter(ok)).toBe(ok);
    }
  });

  test("collects each named person once, sorted by name", () => {
    const list = assigneesIn([people, { needs_human: true, assignees: [bob] }, human, agents]);
    expect(list.map((a) => a.username)).toEqual(["alice", "bob"]);
  });
});

describe("activity wording", () => {
  test("reads assign and unassign entries", () => {
    expect(assignActivity("assign", "@alice")).toEqual({ verb: "assigned", subject: "@alice" });
    expect(assignActivity("unassign", "@alice")).toEqual({ verb: "unassigned", subject: "@alice" });
    expect(assignActivity("assign", "human")).toEqual({ verb: "marked for any person", subject: null });
    expect(assignActivity("unassign", "human")).toEqual({ verb: "removed the any-person mark", subject: null });
  });
});

describe("saved views", async () => {
  // views.ts reaches api.ts, which reads window.location at import time.
  (globalThis as { window?: unknown }).window ??= { location: { origin: "http://localhost" } };
  const { parseConfig, configsDiffer } = await import("../src/lib/issues/views");

  test("a view saved before assignment existed applies with no assignee filter", () => {
    const old = parseConfig(JSON.stringify({ version: 1, layout: "list", filterStatus: "todo" }));
    expect(old?.filterAssignee).toBe("");
    expect(old?.filterStatus).toBe("todo");
  });

  test("the assignee filter round-trips and counts as a change", () => {
    const mine = parseConfig(JSON.stringify({ version: 1, filterAssignee: "me" }))!;
    const none = parseConfig(JSON.stringify({ version: 1 }))!;
    expect(mine.filterAssignee).toBe("me");
    expect(configsDiffer(mine, none)).toBe(true);
    expect(parseConfig(JSON.stringify({ filterAssignee: "nonsense" }))!.filterAssignee).toBe("");
  });
});

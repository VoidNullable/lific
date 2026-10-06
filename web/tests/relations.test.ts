import { describe, expect, test } from "bun:test";
import type { Issue } from "../src/lib/api";
import { RELATION_KINDS, hasRelations, linkRequest, relationsOf } from "../src/lib/issues/relations";

const kind = (field: string) => RELATION_KINDS.find((k) => k.field === field)!;

describe("issue relations", () => {
  test("forward kinds link the viewed issue as source", () => {
    expect(linkRequest(kind("blocks"), "LIF-1", "LIF-2")).toEqual({
      source: "LIF-1",
      target: "LIF-2",
      relation_type: "blocks",
    });
    expect(linkRequest(kind("duplicates"), "LIF-1", "LIF-2").relation_type).toBe("duplicate");
  });

  test("inverse kinds link the other issue as source", () => {
    expect(linkRequest(kind("blocked_by"), "LIF-1", "LIF-2")).toEqual({
      source: "LIF-2",
      target: "LIF-1",
      relation_type: "blocks",
    });
    expect(linkRequest(kind("duplicated_by"), "LIF-1", "LIF-2")).toEqual({
      source: "LIF-2",
      target: "LIF-1",
      relation_type: "duplicate",
    });
  });

  test("reads relation lists the API omits when empty", () => {
    const issue = { identifier: "LIF-1", duplicated_by: ["LIF-9"] } as Issue;
    expect(relationsOf(issue, kind("blocks"))).toEqual([]);
    expect(relationsOf(issue, kind("duplicated_by"))).toEqual(["LIF-9"]);
    expect(hasRelations(issue)).toBe(true);
    expect(hasRelations({ identifier: "LIF-2" } as Issue)).toBe(false);
  });

  test("lists a mirrored relation once", () => {
    const issue = { identifier: "LIF-1", relates_to: ["LIF-2", "LIF-2"] } as Issue;
    expect(relationsOf(issue, kind("relates_to"))).toEqual(["LIF-2"]);
  });
});

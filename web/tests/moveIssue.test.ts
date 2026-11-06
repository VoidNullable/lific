import { describe, expect, test } from "bun:test";
import type { Issue, MovedIssue, Project } from "../src/lib/api";
import { filterTargets, moveNotice, moveTargets } from "../src/lib/issues/move";

const project = (id: number, identifier: string, name: string) =>
  ({ id, identifier, name }) as Project;

const app = project(1, "APP", "App");
const ops = project(2, "OPS", "Operations");
const docs = project(3, "DOC", "Docs site");

describe("move targets", () => {
  test("leaves out the current project and projects the caller cannot write to", () => {
    const writable = new Set([1, 2]);
    expect(moveTargets([app, ops, docs], 1, (id) => writable.has(id))).toEqual([ops]);
  });

  test("filters by identifier or name, case-insensitively", () => {
    expect(filterTargets([app, ops, docs], "op")).toEqual([ops]);
    expect(filterTargets([app, ops, docs], "SITE")).toEqual([docs]);
    expect(filterTargets([app, ops, docs], "  ")).toEqual([app, ops, docs]);
  });
});

describe("move notice", () => {
  const moved = (extra: Partial<MovedIssue> = {}): MovedIssue => ({
    issue: { identifier: "OPS-4" } as Issue,
    previous_identifier: "APP-12",
    previous_project_id: 1,
    ...extra,
  });

  test("names the old and new identifier", () => {
    expect(moveNotice(moved())).toBe("Moved APP-12 to OPS-4.");
  });

  test("lists whatever did not come along", () => {
    expect(
      moveNotice(
        moved({
          dropped_labels: ["bug", "ux"],
          dropped_module: "Core",
          dropped_assignees: ["alice"],
        }),
      ),
    ).toBe("Moved APP-12 to OPS-4. Dropped labels bug, ux. Cleared module Core. Unassigned alice.");
    expect(moveNotice(moved({ dropped_labels: ["bug"] }))).toBe(
      "Moved APP-12 to OPS-4. Dropped label bug.",
    );
  });
});

import { describe, expect, test } from "bun:test";
import {
  mergePickerHits,
  projectLabel,
  startsForeignGroup,
  type PickerSearchRow,
} from "../src/lib/issuePickerHits";

const projects = [
  { id: 1, identifier: "ENG", name: "Engine" },
  { id: 2, identifier: "GAME", name: "Game" },
  { id: 3, identifier: "OPS", name: "OPS" },
];

const row = (identifier: string, project_id: number, result_type = "issue"): PickerSearchRow => ({
  result_type,
  id: Number(identifier.split("-")[1]) + project_id * 100,
  identifier,
  title: `Title of ${identifier}`,
  project_id,
});

describe("issue picker hits", () => {
  test("project-scoped mode drops other projects, as before", () => {
    const hits = mergePickerHits({
      projectId: 1,
      crossProject: false,
      scoped: [row("ENG-1", 1), row("GAME-2", 2)],
      global: [row("GAME-3", 2)],
      projects,
    });
    expect(hits.map((h) => h.identifier)).toEqual(["ENG-1"]);
    expect(hits[0].projectLabel).toBeUndefined();
  });

  test("cross-project mode lists the current project first, then labelled others", () => {
    const hits = mergePickerHits({
      projectId: 1,
      crossProject: true,
      scoped: [row("ENG-4", 1)],
      global: [row("GAME-2", 2), row("ENG-4", 1), row("ENG-5", 1), row("OPS-1", 3)],
      projects,
    });
    expect(hits.map((h) => h.identifier)).toEqual(["ENG-4", "ENG-5", "GAME-2", "OPS-1"]);
    expect(hits.map((h) => h.projectLabel)).toEqual([undefined, undefined, "Game (GAME)", "OPS"]);
  });

  test("non-issue results are ignored", () => {
    const hits = mergePickerHits({
      projectId: 1,
      crossProject: true,
      global: [row("GAME-2", 2, "comment"), row("GAME-3", 2, "page")],
      projects,
    });
    expect(hits).toEqual([]);
  });

  test("an exact identifier from another project leads, labelled, and is not repeated", () => {
    const hits = mergePickerHits({
      projectId: 1,
      crossProject: true,
      idHit: { id: 202, identifier: "GAME-2", title: "Named", status: "todo", project_id: 2 },
      scoped: [row("ENG-1", 1)],
      global: [row("GAME-2", 2), row("GAME-7", 2)],
      projects,
    });
    expect(hits.map((h) => h.identifier)).toEqual(["GAME-2", "ENG-1", "GAME-7"]);
    expect(hits[0]).toMatchObject({ status: "todo", projectLabel: "Game (GAME)" });
    expect(hits.map((h, i) => startsForeignGroup(hits, i, 1))).toEqual([false, false, true]);
  });

  test("label falls back to the identifier's code for an unlisted project", () => {
    expect(projectLabel(9, "MISC-3", projects)).toBe("MISC");
  });
});

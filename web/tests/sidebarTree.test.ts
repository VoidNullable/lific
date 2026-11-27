import { describe, expect, test } from "bun:test";
import type { Project, ProjectGroup, SidebarItem, SidebarLayout } from "../src/lib/api";
import {
  CYCLE_REASON, applyMove, buildTree, canHoldSubfolder, checkMove, flatProjects, folderChoices, folderPaths,
  hidingFolder, indexTree, moveOut, newFolderSlot, resolveDrop, stepMove, wrapFits, zoneAt, type FolderNode,
} from "../src/lib/sidebarTree";

const f = (id: number): SidebarItem => ({ kind: "folder", id });
const p = (id: number): SidebarItem => ({ kind: "project", id });
const project = (id: number): Project => ({
  id, name: `P${id}`, identifier: `P${id}`, description: "", emoji: null, lead_user_id: null,
  sort_order: id, created_at: "", updated_at: "", is_public: false,
} as Project);
const folder = (id: number, name: string, items: SidebarItem[], parent: number | null = null, depth = 1): ProjectGroup => ({
  id, user_id: 1, name, parent_id: parent, depth, sort_order: 0, items,
  project_ids: items.filter((i) => i.kind === "project").map((i) => i.id), created_at: "", updated_at: "",
});

// Work(10) > Clients(11) > Acme(12) > Deep(13); Side(20) empty.
// Top level: P1, Work, P2, Side.
function fixture(): { projects: Project[]; layout: SidebarLayout } {
  return {
    projects: [1, 2, 3, 4, 5].map(project),
    layout: {
      root: [p(1), f(10), p(2), f(20)],
      groups: [
        folder(10, "Work", [f(11), p(3)]),
        folder(11, "Clients", [f(12)], 10, 2),
        folder(12, "Acme", [p(4), f(13)], 11, 3),
        folder(13, "Deep", [], 12, 4),
        folder(20, "Side", []),
      ],
      max_depth: 4,
    },
  };
}

function setup() {
  const { projects, layout } = fixture();
  const tree = buildTree(projects, layout);
  return { projects, layout, tree, index: indexTree(tree) };
}

const open = () => true;

describe("buildTree", () => {
  test("nests folders, counts what they hold and never loses a project", () => {
    const { tree } = setup();
    expect(tree.map((n) => `${n.kind}:${n.id}`)).toEqual(["project:1", "folder:10", "project:2", "folder:20", "project:5"]);
    const work = tree[1] as FolderNode;
    expect(work.projectCount).toBe(2);
    expect(work.height).toBe(4);
    expect((tree[3] as FolderNode).projectCount).toBe(0);
    expect(flatProjects(tree).map((x) => x.id)).toEqual([1, 4, 3, 2, 5]);
  });

  test("without a layout every project sits at the top level", () => {
    const tree = buildTree([1, 2].map(project), null);
    expect(tree.map((n) => n.id)).toEqual([1, 2]);
  });

  test("skips projects and folders it cannot resolve, and repeats", () => {
    const tree = buildTree([project(1)], {
      root: [p(1), p(99), f(98), p(1)], groups: [], max_depth: 4,
    });
    expect(tree.map((n) => n.id)).toEqual([1]);
  });
});

describe("drop zones", () => {
  test("projects split in half, folders take the middle as inside", () => {
    expect(zoneAt("project", 4, 20)).toBe("before");
    expect(zoneAt("project", 14, 20)).toBe("after");
    expect(zoneAt("folder", 3, 20)).toBe("before");
    expect(zoneAt("folder", 10, 20)).toBe("inside");
    expect(zoneAt("folder", 17, 20)).toBe("after");
  });
});

describe("resolveDrop", () => {
  test("a project dropped before, after and inside", () => {
    const { tree, index } = setup();
    expect(resolveDrop(index, tree, p(2), p(1), "before", open)).toEqual({
      kind: "move", move: { item: p(2), parent_id: null, before: p(1) },
    });
    expect(resolveDrop(index, tree, p(1), p(2), "after", open)).toEqual({
      kind: "move", move: { item: p(1), parent_id: null, before: f(20) },
    });
    expect(resolveDrop(index, tree, p(1), f(20), "inside", open)).toEqual({
      kind: "move", move: { item: p(1), parent_id: 20, before: null },
    });
    // Out of a nested folder to the top level.
    expect(resolveDrop(index, tree, p(4), p(1), "before", open)).toEqual({
      kind: "move", move: { item: p(4), parent_id: null, before: p(1) },
    });
  });

  test("after an open folder's heading is its first slot; after a closed one is the next sibling", () => {
    const { tree, index } = setup();
    expect(resolveDrop(index, tree, p(1), f(10), "after", open)).toEqual({
      kind: "move", move: { item: p(1), parent_id: 10, before: f(11) },
    });
    expect(resolveDrop(index, tree, p(1), f(10), "after", () => false)).toEqual({
      kind: "move", move: { item: p(1), parent_id: null, before: p(2) },
    });
  });

  test("below the tree is last at the top level", () => {
    const { tree, index } = setup();
    expect(resolveDrop(index, tree, p(3), null, "after", open)).toEqual({
      kind: "move", move: { item: p(3), parent_id: null, before: null },
    });
    expect(resolveDrop(index, tree, p(5), null, "after", open)).toEqual({ kind: "noop" });
  });

  test("dropping in place changes nothing", () => {
    const { tree, index } = setup();
    expect(resolveDrop(index, tree, p(1), p(1), "after", open)).toEqual({ kind: "noop" });
    expect(resolveDrop(index, tree, p(1), f(10), "before", open)).toEqual({ kind: "noop" });
    expect(resolveDrop(index, tree, p(2), f(20), "before", open)).toEqual({ kind: "noop" });
    expect(resolveDrop(index, tree, p(1), f(10), "after", () => false)).toEqual({
      kind: "move", move: { item: p(1), parent_id: null, before: p(2) },
    });
  });

  test("a folder into itself or a descendant is refused with the reason", () => {
    const { tree, index } = setup();
    for (const [target, zone] of [[f(10), "inside"], [f(12), "inside"], [p(4), "before"], [f(13), "after"]] as const) {
      expect(resolveDrop(index, tree, f(10), target, zone, open)).toEqual({ kind: "refused", reason: CYCLE_REASON });
    }
  });

  test("past the depth cap is refused, at the cap is allowed", () => {
    const { tree, index } = setup();
    // Work is four levels tall: inside Side it would make five.
    const deep = resolveDrop(index, tree, f(10), f(20), "inside", open);
    expect(deep).toEqual({ kind: "refused", reason: "Folders can nest 4 levels deep. Moving 'Work' there would make 5 levels." });
    // Side is one level: inside Deep (level 4) it would make five.
    expect(resolveDrop(index, tree, f(20), f(13), "inside", open).kind).toBe("refused");
    // Inside Acme (level 3) it is level 4.
    expect(resolveDrop(index, tree, f(20), f(12), "inside", open)).toEqual({
      kind: "move", move: { item: f(20), parent_id: 12, before: null },
    });
    // A project can sit inside the deepest folder.
    expect(resolveDrop(index, tree, p(1), f(13), "inside", open).kind).toBe("move");
  });
});

describe("menu moves", () => {
  test("up and down among siblings, null at the ends", () => {
    const { index } = setup();
    expect(stepMove(index, f(10), "up")).toEqual({ item: f(10), parent_id: null, before: p(1) });
    expect(stepMove(index, f(10), "down")).toEqual({ item: f(10), parent_id: null, before: f(20) });
    expect(stepMove(index, f(20), "down")).toEqual({ item: f(20), parent_id: null, before: null });
    expect(stepMove(index, p(5), "down")).toBeNull();
    expect(stepMove(index, p(1), "up")).toBeNull();
    expect(stepMove(index, p(3), "up")).toEqual({ item: p(3), parent_id: 10, before: f(11) });
  });

  test("move out lands right after the containing folder", () => {
    const { index } = setup();
    expect(moveOut(index, p(3))).toEqual({ item: p(3), parent_id: null, before: p(2) });
    expect(moveOut(index, f(12))).toEqual({ item: f(12), parent_id: 10, before: p(3) });
    expect(moveOut(index, p(1))).toBeNull();
  });

  test("folder choices explain what can't be picked", () => {
    const { tree, index } = setup();
    const choices = folderChoices(tree, index, f(20));
    expect(choices.map((c) => [c.name, c.level, c.disabled])).toEqual([
      ["Top level", 0, "Here now"], ["Work", 1, null], ["Clients", 2, null], ["Acme", 3, null],
      ["Deep", 4, "Too deep"], ["Side", 1, "Inside itself"],
    ]);
    expect(folderChoices(tree, index, p(4)).find((c) => c.name === "Acme")?.disabled).toBe("Here now");
  });

  test("subfolders fit up to the cap", () => {
    const { index } = setup();
    expect(canHoldSubfolder(index, 12)).toBe(true);
    expect(canHoldSubfolder(index, 13)).toBe(false);
  });

  test("a new folder around an item must fit, and lands after the folders", () => {
    const { tree, index } = setup();
    expect(wrapFits(index, p(4))).toBe(true);
    expect(wrapFits(index, f(13))).toBe(false);
    expect(wrapFits(index, f(10))).toBe(false);
    expect(wrapFits(index, f(20))).toBe(true);
    expect(newFolderSlot(tree)).toBe(4);
    expect(newFolderSlot([])).toBe(0);
  });

  test("checkMove refuses an item that left the tree", () => {
    const { index } = setup();
    expect(checkMove(index, { item: p(99), parent_id: null, before: null }).kind).toBe("refused");
  });
});

describe("applyMove", () => {
  test("moves a folder with its subtree and recomputes depth", () => {
    const { layout } = setup();
    const next = applyMove(layout, { item: f(11), parent_id: null, before: p(1) });
    expect(next.root).toEqual([f(11), p(1), f(10), p(2), f(20)]);
    expect(next.groups.map((g) => [g.id, g.parent_id, g.depth])).toEqual([
      [11, null, 1], [12, 11, 2], [13, 12, 3], [10, null, 1], [20, null, 1],
    ]);
    expect(next.groups.find((g) => g.id === 10)?.items).toEqual([p(3)]);
    expect(layout.root).toEqual([p(1), f(10), p(2), f(20)]);
  });

  test("projects move between folders and keep project_ids in step", () => {
    const { layout } = setup();
    const next = applyMove(layout, { item: p(4), parent_id: 20, before: null });
    expect(next.groups.find((g) => g.id === 20)?.project_ids).toEqual([4]);
    expect(next.groups.find((g) => g.id === 12)?.project_ids).toEqual([]);
  });
});

describe("folderPaths", () => {
  test("names nested folders by their full path", () => {
    const { layout } = setup();
    expect(folderPaths(layout.groups).map((x) => x.label)).toEqual([
      "Work", "Work / Clients", "Work / Clients / Acme", "Work / Clients / Acme / Deep", "Side",
    ]);
  });
});

describe("hidingFolder", () => {
  test("names the outermost collapsed ancestor", () => {
    const { index } = setup();
    expect(hidingFolder(index, p(4), new Set([12, 11]))).toBe(11);
    expect(hidingFolder(index, p(4), new Set())).toBeNull();
    expect(hidingFolder(index, p(1), new Set([10]))).toBeNull();
  });
});

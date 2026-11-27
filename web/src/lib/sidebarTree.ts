// The sidebar's folder tree and the rules for moving things around in it.
// Pure, so the drop logic is unit-tested without a browser. The server makes
// the same checks (src/db/queries/project_groups.rs) and is the authority;
// these exist so a refused drop is explained before anything is sent.

import type { Project, ProjectGroup, SidebarItem, SidebarLayout, SidebarMove } from "./api";

export const DEFAULT_MAX_DEPTH = 4;

export const CYCLE_REASON = "A folder can't go inside itself or one of its own subfolders.";

export function depthReason(name: string, levels: number, max: number): string {
  return `Folders can nest ${max} levels deep. Moving '${name}' there would make ${levels} levels.`;
}

export interface FolderNode {
  kind: "folder";
  id: number;
  folder: ProjectGroup;
  /** Folders above this one; 0 at the top level. */
  level: number;
  children: TreeNode[];
  /** Projects anywhere inside, subfolders included. */
  projectCount: number;
  /** Levels this folder occupies, itself included. */
  height: number;
}

export interface ProjectNode {
  kind: "project";
  id: number;
  project: Project;
  level: number;
}

export type TreeNode = FolderNode | ProjectNode;

export const keyOf = (item: SidebarItem): string => `${item.kind}:${item.id}`;
export const sameItem = (a: SidebarItem | null | undefined, b: SidebarItem | null | undefined): boolean =>
  !!a && !!b && a.kind === b.kind && a.id === b.id;
export const itemOf = (node: TreeNode): SidebarItem => ({ kind: node.kind, id: node.id });

/** Build the rendered tree. Projects the layout does not place (the layout
 *  failed to load, or a project arrived after it) go last at the top level,
 *  so a project is never hidden by a stale or missing layout. */
export function buildTree(projects: Project[], layout: SidebarLayout | null): TreeNode[] {
  const byId = new Map(projects.map((p) => [p.id, p]));
  const folders = new Map((layout?.groups ?? []).map((g) => [g.id, g]));
  const placed = new Set<number>();
  const seenFolders = new Set<number>();
  function build(items: SidebarItem[], level: number): TreeNode[] {
    const out: TreeNode[] = [];
    for (const item of items) {
      if (item.kind === "project") {
        const project = byId.get(item.id);
        if (!project || placed.has(item.id)) continue;
        placed.add(item.id);
        out.push({ kind: "project", id: item.id, project, level });
      } else {
        const folder = folders.get(item.id);
        if (!folder || seenFolders.has(item.id)) continue;
        seenFolders.add(item.id);
        const children = build(folder.items ?? [], level + 1);
        out.push({
          kind: "folder", id: folder.id, folder, level, children,
          projectCount: children.reduce((n, c) => n + (c.kind === "project" ? 1 : c.projectCount), 0),
          height: 1 + Math.max(0, ...children.map((c) => (c.kind === "folder" ? c.height : 0))),
        });
      }
    }
    return out;
  }
  const root = build(layout?.root ?? [], 0);
  for (const project of projects) {
    if (!placed.has(project.id)) root.push({ kind: "project", id: project.id, project, level: 0 });
  }
  return root;
}

export interface Placement {
  node: TreeNode;
  /** The containing folder, or null at the top level. */
  parent: number | null;
  siblings: TreeNode[];
  index: number;
}

export type TreeIndex = Map<string, Placement>;

export function indexTree(nodes: TreeNode[]): TreeIndex {
  const index: TreeIndex = new Map();
  const walk = (siblings: TreeNode[], parent: number | null) => {
    siblings.forEach((node, i) => {
      index.set(keyOf(node), { node, parent, siblings, index: i });
      if (node.kind === "folder") walk(node.children, node.id);
    });
  };
  walk(nodes, null);
  return index;
}

/** Every project in sidebar order, depth-first. */
export function flatProjects(nodes: TreeNode[]): Project[] {
  return nodes.flatMap((n) => (n.kind === "project" ? [n.project] : flatProjects(n.children)));
}

/** Folder ids from the top level down to the one holding `item`. */
export function ancestors(index: TreeIndex, item: SidebarItem): number[] {
  const out: number[] = [];
  let parent = index.get(keyOf(item))?.parent ?? null;
  while (parent !== null) {
    out.unshift(parent);
    parent = index.get(keyOf({ kind: "folder", id: parent }))?.parent ?? null;
  }
  return out;
}

/** The outermost collapsed folder hiding `item`, if any. */
export function hidingFolder(index: TreeIndex, item: SidebarItem, collapsed: Set<number>): number | null {
  return ancestors(index, item).find((id) => collapsed.has(id)) ?? null;
}

/** Levels above and including `folder`; the top level is 0. */
function depthOf(index: TreeIndex, folder: number | null): number {
  return folder === null ? 0 : ancestors(index, { kind: "folder", id: folder }).length + 1;
}

function within(index: TreeIndex, target: number | null, ancestor: number): boolean {
  if (target === null) return false;
  return target === ancestor || ancestors(index, { kind: "folder", id: target }).includes(ancestor);
}

export type DropZone = "before" | "after" | "inside";

/** Where on a row the pointer is. A project takes before or after; a folder
 *  also takes inside across its middle half. */
export function zoneAt(kind: SidebarItem["kind"], offsetY: number, height: number): DropZone {
  const at = height > 0 ? offsetY / height : 0.5;
  if (kind === "project") return at < 0.5 ? "before" : "after";
  return at < 0.25 ? "before" : at > 0.75 ? "after" : "inside";
}

export type DropResult =
  | { kind: "move"; move: SidebarMove }
  | { kind: "noop" }
  | { kind: "refused"; reason: string };

/** Check a move against the tree: a folder can't go inside itself, nothing
 *  nests past `maxDepth`, and a move that changes nothing is a no-op. */
export function checkMove(index: TreeIndex, move: SidebarMove, maxDepth = DEFAULT_MAX_DEPTH): DropResult {
  const { item, parent_id: parent, before } = move;
  const placement = index.get(keyOf(item));
  if (!placement) return { kind: "refused", reason: "That item is no longer in the sidebar." };
  if (item.kind === "folder") {
    if (within(index, parent, item.id)) return { kind: "refused", reason: CYCLE_REASON };
    const node = placement.node as FolderNode;
    const levels = depthOf(index, parent) + node.height;
    if (levels > maxDepth) return { kind: "refused", reason: depthReason(node.folder.name, levels, maxDepth) };
  }
  if (sameItem(before, item)) return { kind: "noop" };
  if (placement.parent === parent) {
    const next = placement.siblings[placement.index + 1];
    if (sameItem(next ? itemOf(next) : null, before) || (!next && before === null)) return { kind: "noop" };
  }
  return { kind: "move", move };
}

/** The move a drop on `target` means. `target` null is the empty space
 *  below the tree: last at the top level. `isOpen` tells whether a folder's
 *  contents are showing, since "after" an open folder's heading is visually
 *  its first child's slot. */
export function resolveDrop(
  index: TreeIndex,
  roots: TreeNode[],
  dragged: SidebarItem,
  target: SidebarItem | null,
  zone: DropZone,
  isOpen: (folder: number) => boolean,
  maxDepth = DEFAULT_MAX_DEPTH,
): DropResult {
  const others = (list: TreeNode[]) => list.filter((n) => !sameItem(itemOf(n), dragged));
  let parent: number | null;
  let before: SidebarItem | null;
  if (target === null) {
    parent = null;
    before = null;
    if (others(roots).length === 0) return { kind: "noop" };
  } else {
    const placement = index.get(keyOf(target));
    if (!placement) return { kind: "noop" };
    const node = placement.node;
    if (zone === "inside" && node.kind === "folder") {
      parent = node.id;
      before = null;
    } else if (zone === "before") {
      if (sameItem(target, dragged)) return { kind: "noop" };
      parent = placement.parent;
      before = target;
    } else if (node.kind === "folder" && isOpen(node.id) && others(node.children).length > 0 && !sameItem(target, dragged)) {
      parent = node.id;
      before = itemOf(others(node.children)[0]);
    } else {
      if (sameItem(target, dragged)) return { kind: "noop" };
      parent = placement.parent;
      const next = placement.siblings.slice(placement.index + 1).find((n) => !sameItem(itemOf(n), dragged));
      before = next ? itemOf(next) : null;
    }
  }
  return checkMove(index, { item: dragged, parent_id: parent, before }, maxDepth);
}

/** Move up or down among siblings, or null at that end. */
export function stepMove(index: TreeIndex, item: SidebarItem, direction: "up" | "down"): SidebarMove | null {
  const placement = index.get(keyOf(item));
  if (!placement) return null;
  const { siblings, index: i, parent } = placement;
  if (direction === "up") {
    if (i === 0) return null;
    return { item, parent_id: parent, before: itemOf(siblings[i - 1]) };
  }
  if (i >= siblings.length - 1) return null;
  const after = siblings[i + 2];
  return { item, parent_id: parent, before: after ? itemOf(after) : null };
}

/** Out of the containing folder, to just after it in its own parent. */
export function moveOut(index: TreeIndex, item: SidebarItem): SidebarMove | null {
  const parent = index.get(keyOf(item))?.parent ?? null;
  if (parent === null) return null;
  const folder = index.get(keyOf({ kind: "folder", id: parent }))!;
  const next = folder.siblings[folder.index + 1];
  return { item, parent_id: folder.parent, before: next ? itemOf(next) : null };
}

export interface FolderChoice {
  /** null is the top level. */
  id: number | null;
  name: string;
  level: number;
  /** Why it can't be chosen, when it can't. */
  disabled: string | null;
}

/** Destinations for "Move to folder": the top level, then every folder
 *  depth-first, each disabled with the reason when the move is refused or
 *  would change nothing. */
export function folderChoices(nodes: TreeNode[], index: TreeIndex, item: SidebarItem, maxDepth = DEFAULT_MAX_DEPTH): FolderChoice[] {
  const reason = (parent: number | null): string | null => {
    const result = checkMove(index, { item, parent_id: parent, before: null }, maxDepth);
    if (result.kind === "refused") return result.reason === CYCLE_REASON ? "Inside itself" : "Too deep";
    if (index.get(keyOf(item))?.parent === parent) return "Here now";
    return null;
  };
  const out: FolderChoice[] = [{ id: null, name: "Top level", level: 0, disabled: reason(null) }];
  const walk = (list: TreeNode[]) => {
    for (const node of list) {
      if (node.kind !== "folder") continue;
      out.push({ id: node.id, name: node.folder.name, level: node.level + 1, disabled: reason(node.id) });
      walk(node.children);
    }
  };
  walk(nodes);
  return out;
}

/** Whether a new subfolder fits inside `folder`. */
export function canHoldSubfolder(index: TreeIndex, folder: number, maxDepth = DEFAULT_MAX_DEPTH): boolean {
  return depthOf(index, folder) + 1 <= maxDepth;
}

/** Each folder's full name, "Work / Clients", in the given (depth-first)
 *  order, for flat pickers that can't show nesting by indentation. */
export function folderPaths(groups: ProjectGroup[]): { id: number; label: string }[] {
  const byId = new Map(groups.map((g) => [g.id, g]));
  return groups.map((g) => {
    const names = [g.name];
    const seen = new Set([g.id]);
    let parent = g.parent_id;
    while (parent !== null && !seen.has(parent) && byId.has(parent)) {
      seen.add(parent);
      const up = byId.get(parent)!;
      names.unshift(up.name);
      parent = up.parent_id;
    }
    return { id: g.id, label: names.join(" / ") };
  });
}

/** Whether a new folder made around `item`, where it sits now, fits. */
export function wrapFits(index: TreeIndex, item: SidebarItem, maxDepth = DEFAULT_MAX_DEPTH): boolean {
  const placement = index.get(keyOf(item));
  if (!placement) return false;
  const height = placement.node.kind === "folder" ? placement.node.height : 0;
  return depthOf(index, placement.parent) + 1 + height <= maxDepth;
}

/** Where a folder being created shows in its container: after the folders
 *  already there, which is where the server puts it. */
export function newFolderSlot(nodes: TreeNode[]): number {
  let slot = 0;
  nodes.forEach((n, i) => { if (n.kind === "folder") slot = i + 1; });
  return slot;
}

/** Apply a move to a layout locally, for the optimistic paint while the
 *  server answers. The server's answer replaces it either way. */
export function applyMove(layout: SidebarLayout, move: SidebarMove): SidebarLayout {
  const lists = new Map<number | null, SidebarItem[]>([[null, [...layout.root]]]);
  for (const g of layout.groups) lists.set(g.id, [...g.items]);
  for (const list of lists.values()) {
    const at = list.findIndex((i) => sameItem(i, move.item));
    if (at >= 0) list.splice(at, 1);
  }
  const target = lists.get(move.parent_id);
  if (!target) return layout;
  const at = move.before ? target.findIndex((i) => sameItem(i, move.before)) : -1;
  target.splice(at >= 0 ? at : target.length, 0, move.item);
  const byId = new Map(layout.groups.map((g) => [g.id, g]));
  const groups: ProjectGroup[] = [];
  const walk = (items: SidebarItem[], parent: number | null, depth: number) => {
    for (const item of items) {
      if (item.kind !== "folder") continue;
      const g = byId.get(item.id);
      if (!g) continue;
      const own = lists.get(g.id) ?? [];
      groups.push({ ...g, parent_id: parent, depth: depth + 1, items: own,
        project_ids: own.filter((i) => i.kind === "project").map((i) => i.id) });
      walk(own, g.id, depth + 1);
    }
  };
  walk(lists.get(null)!, null, 0);
  return { ...layout, root: lists.get(null)!, groups };
}

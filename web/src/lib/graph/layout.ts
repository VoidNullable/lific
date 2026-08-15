/*
 * LIF-363 — layered DAG layout for the dependency graph view.
 *
 * Small hand-rolled Sugiyama-lite: longest-path layering, a few barycenter
 * ordering sweeps, then fixed-size coordinate assignment. Deliberately not a
 * library — dagre and elk are each an order of magnitude more code than this
 * view needs, and the graphs here are small (a project's open blocking
 * structure, tens of nodes, not thousands).
 *
 * Direction convention: an edge source→target means "source blocks target",
 * and layers flow left-to-right, so blockers sit left of the work they hold
 * up. Cycles are technically creatable via link_issues; layering breaks them
 * (back edges found by DFS are ignored for layer assignment) but every edge
 * is still returned for rendering, so a cycle draws as a right-to-left edge
 * rather than crashing or vanishing.
 */

export interface LayoutEdge {
  source: number;
  target: number;
}

export interface PlacedNode {
  id: number;
  x: number;
  y: number;
}

export interface GraphLayout {
  /** Node id → top-left coordinates. */
  positions: Map<number, PlacedNode>;
  width: number;
  height: number;
}

export interface LayoutOptions {
  nodeWidth: number;
  nodeHeight: number;
  /** Horizontal gap between layers. */
  gapX: number;
  /** Vertical gap between nodes in a layer. */
  gapY: number;
  /** Vertical gap between disconnected components. */
  componentGap: number;
}

interface Component {
  nodes: number[];
  edges: LayoutEdge[];
}

/** Split the graph into connected components (undirected reachability). */
function components(nodeIds: number[], edges: LayoutEdge[]): Component[] {
  const neighbors = new Map<number, number[]>();
  for (const id of nodeIds) neighbors.set(id, []);
  for (const e of edges) {
    neighbors.get(e.source)?.push(e.target);
    neighbors.get(e.target)?.push(e.source);
  }
  const seen = new Set<number>();
  const out: Component[] = [];
  for (const start of nodeIds) {
    if (seen.has(start)) continue;
    const members = new Set<number>();
    const stack = [start];
    seen.add(start);
    while (stack.length) {
      const n = stack.pop()!;
      members.add(n);
      for (const next of neighbors.get(n) ?? []) {
        if (!seen.has(next)) {
          seen.add(next);
          stack.push(next);
        }
      }
    }
    out.push({
      nodes: [...members],
      edges: edges.filter((e) => members.has(e.source) && members.has(e.target)),
    });
  }
  // Biggest chains first — the interesting structure lands at the top.
  out.sort((a, b) => b.nodes.length - a.nodes.length);
  return out;
}

/** Iterative DFS marking edges that close a cycle (back edges). */
function findBackEdges(nodes: number[], edges: LayoutEdge[]): Set<LayoutEdge> {
  const out = new Map<number, LayoutEdge[]>();
  for (const n of nodes) out.set(n, []);
  for (const e of edges) out.get(e.source)?.push(e);

  const WHITE = 0, GRAY = 1, BLACK = 2;
  const color = new Map<number, number>(nodes.map((n) => [n, WHITE]));
  const back = new Set<LayoutEdge>();

  for (const root of nodes) {
    if (color.get(root) !== WHITE) continue;
    // Stack frames: [node, next edge index to visit].
    const stack: [number, number][] = [[root, 0]];
    color.set(root, GRAY);
    while (stack.length) {
      const frame = stack[stack.length - 1];
      const [n, i] = frame;
      const outEdges = out.get(n) ?? [];
      if (i >= outEdges.length) {
        color.set(n, BLACK);
        stack.pop();
        continue;
      }
      frame[1] = i + 1;
      const edge = outEdges[i];
      const c = color.get(edge.target);
      if (c === GRAY) {
        back.add(edge);
      } else if (c === WHITE) {
        color.set(edge.target, GRAY);
        stack.push([edge.target, 0]);
      }
    }
  }
  return back;
}

/** Longest-path layering over the acyclic edge subset. */
function assignLayers(nodes: number[], edges: LayoutEdge[]): Map<number, number> {
  const preds = new Map<number, number[]>();
  const succs = new Map<number, number[]>();
  const indegree = new Map<number, number>();
  for (const n of nodes) {
    preds.set(n, []);
    succs.set(n, []);
    indegree.set(n, 0);
  }
  for (const e of edges) {
    succs.get(e.source)!.push(e.target);
    preds.get(e.target)!.push(e.source);
    indegree.set(e.target, (indegree.get(e.target) ?? 0) + 1);
  }

  const layer = new Map<number, number>(nodes.map((n) => [n, 0]));
  const queue = nodes.filter((n) => indegree.get(n) === 0);
  while (queue.length) {
    const n = queue.shift()!;
    for (const next of succs.get(n)!) {
      layer.set(next, Math.max(layer.get(next)!, layer.get(n)! + 1));
      const d = indegree.get(next)! - 1;
      indegree.set(next, d);
      if (d === 0) queue.push(next);
    }
  }
  return layer;
}

/** A few barycenter sweeps to reduce edge crossings between layers. */
function orderLayers(
  layerOf: Map<number, number>,
  edges: LayoutEdge[],
): Map<number, number[]> {
  const layers = new Map<number, number[]>();
  for (const [n, l] of layerOf) {
    if (!layers.has(l)) layers.set(l, []);
    layers.get(l)!.push(n);
  }
  const layerIndices = [...layers.keys()].sort((a, b) => a - b);

  const position = new Map<number, number>();
  const reindex = () => {
    for (const l of layerIndices) {
      layers.get(l)!.forEach((n, i) => position.set(n, i));
    }
  };
  reindex();

  const preds = new Map<number, number[]>();
  const succs = new Map<number, number[]>();
  for (const e of edges) {
    if (!succs.has(e.source)) succs.set(e.source, []);
    if (!preds.has(e.target)) preds.set(e.target, []);
    succs.get(e.source)!.push(e.target);
    preds.get(e.target)!.push(e.source);
  }

  const sweep = (neighborsOf: Map<number, number[]>, order: number[]) => {
    for (const l of order) {
      const row = layers.get(l)!;
      const bary = new Map<number, number>();
      for (const n of row) {
        const neigh = neighborsOf.get(n) ?? [];
        bary.set(
          n,
          neigh.length
            ? neigh.reduce((sum, m) => sum + (position.get(m) ?? 0), 0) / neigh.length
            : (position.get(n) ?? 0),
        );
      }
      row.sort((a, b) => bary.get(a)! - bary.get(b)!);
      reindex();
    }
  };

  for (let i = 0; i < 4; i++) {
    sweep(preds, layerIndices); // left → right, pulled by predecessors
    sweep(succs, [...layerIndices].reverse()); // right → left, pulled by successors
  }
  return layers;
}

/** Lay out one component; coordinates are component-local from (0,0). */
function layoutComponent(
  comp: Component,
  opts: LayoutOptions,
): GraphLayout {
  const back = findBackEdges(comp.nodes, comp.edges);
  const acyclic = comp.edges.filter((e) => !back.has(e));
  const layerOf = assignLayers(comp.nodes, acyclic);
  const layers = orderLayers(layerOf, acyclic);

  const layerIndices = [...layers.keys()].sort((a, b) => a - b);
  const tallest = Math.max(...layerIndices.map((l) => layers.get(l)!.length));
  const fullHeight = tallest * opts.nodeHeight + (tallest - 1) * opts.gapY;

  const positions = new Map<number, PlacedNode>();
  let width = 0;
  for (const l of layerIndices) {
    const row = layers.get(l)!;
    const rowHeight = row.length * opts.nodeHeight + (row.length - 1) * opts.gapY;
    const yOffset = (fullHeight - rowHeight) / 2; // center shorter layers
    row.forEach((n, i) => {
      const x = l * (opts.nodeWidth + opts.gapX);
      const y = yOffset + i * (opts.nodeHeight + opts.gapY);
      positions.set(n, { id: n, x, y });
      width = Math.max(width, x + opts.nodeWidth);
    });
  }
  return { positions, width, height: fullHeight };
}

/**
 * Lay out the full graph: components stacked vertically, largest first.
 * `nodeIds` should already be filtered to the nodes the caller wants drawn.
 *
 * `edges` drive the left-to-right layering (blocks relations). The optional
 * `clusterEdges` (all relation types) only affect which nodes share a
 * component — so two issues joined by a mere relates_to sit near each other
 * without the undirected relation pretending to be a blocking step.
 */
export function layoutGraph(
  nodeIds: number[],
  edges: LayoutEdge[],
  opts: LayoutOptions,
  clusterEdges: LayoutEdge[] = edges,
): GraphLayout {
  if (nodeIds.length === 0) {
    return { positions: new Map(), width: 0, height: 0 };
  }
  const comps = components(nodeIds, clusterEdges);
  const idSetPerComp = comps.map((c) => new Set(c.nodes));
  const positions = new Map<number, PlacedNode>();
  let width = 0;
  let y = 0;
  for (let i = 0; i < comps.length; i++) {
    const layerEdges = edges.filter(
      (e) => idSetPerComp[i].has(e.source) && idSetPerComp[i].has(e.target),
    );
    const laid = layoutComponent({ nodes: comps[i].nodes, edges: layerEdges }, opts);
    for (const p of laid.positions.values()) {
      positions.set(p.id, { id: p.id, x: p.x, y: p.y + y });
    }
    width = Math.max(width, laid.width);
    y += laid.height + opts.componentGap;
  }
  return { positions, width, height: y - opts.componentGap };
}

/**
 * Simple grid for the Unlinked canvas: no edges to respect, so pack the
 * nodes into rows roughly square-ish (slightly wide, matching landscape
 * viewports). Order is the caller's (issue list order).
 */
export function layoutGrid(
  nodeIds: number[],
  opts: Pick<LayoutOptions, "nodeWidth" | "nodeHeight" | "gapX" | "gapY">,
): GraphLayout {
  if (nodeIds.length === 0) {
    return { positions: new Map(), width: 0, height: 0 };
  }
  const cols = Math.max(1, Math.ceil(Math.sqrt(nodeIds.length * 1.6)));
  const positions = new Map<number, PlacedNode>();
  nodeIds.forEach((id, i) => {
    const col = i % cols;
    const row = Math.floor(i / cols);
    positions.set(id, {
      id,
      x: col * (opts.nodeWidth + opts.gapX),
      y: row * (opts.nodeHeight + opts.gapY),
    });
  });
  const rows = Math.ceil(nodeIds.length / cols);
  return {
    positions,
    width: Math.min(nodeIds.length, cols) * (opts.nodeWidth + opts.gapX) - opts.gapX,
    height: rows * (opts.nodeHeight + opts.gapY) - opts.gapY,
  };
}

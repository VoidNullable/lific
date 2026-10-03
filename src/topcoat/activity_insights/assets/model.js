(() => {
'use strict';
// Geometry, bounded line diff, and trend curves preserve the existing frontend algorithms.
function components(nodeIds, edges) {
    const neighbors = new Map();
    for (const id of nodeIds)
        neighbors.set(id, []);
    for (const e of edges) {
        neighbors.get(e.source)?.push(e.target);
        neighbors.get(e.target)?.push(e.source);
    }
    const seen = new Set();
    const out = [];
    for (const start of nodeIds) {
        if (seen.has(start))
            continue;
        const members = new Set();
        const stack = [start];
        seen.add(start);
        while (stack.length) {
            const n = stack.pop();
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
    out.sort((a, b) => b.nodes.length - a.nodes.length);
    return out;
}
function findBackEdges(nodes, edges) {
    const out = new Map();
    for (const n of nodes)
        out.set(n, []);
    for (const e of edges)
        out.get(e.source)?.push(e);
    const WHITE = 0, GRAY = 1, BLACK = 2;
    const color = new Map(nodes.map((n) => [n, WHITE]));
    const back = new Set();
    for (const root of nodes) {
        if (color.get(root) !== WHITE)
            continue;
        const stack = [[root, 0]];
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
            }
            else if (c === WHITE) {
                color.set(edge.target, GRAY);
                stack.push([edge.target, 0]);
            }
        }
    }
    return back;
}
function assignLayers(nodes, edges) {
    const preds = new Map();
    const succs = new Map();
    const indegree = new Map();
    for (const n of nodes) {
        preds.set(n, []);
        succs.set(n, []);
        indegree.set(n, 0);
    }
    for (const e of edges) {
        succs.get(e.source).push(e.target);
        preds.get(e.target).push(e.source);
        indegree.set(e.target, (indegree.get(e.target) ?? 0) + 1);
    }
    const layer = new Map(nodes.map((n) => [n, 0]));
    const queue = nodes.filter((n) => indegree.get(n) === 0);
    while (queue.length) {
        const n = queue.shift();
        for (const next of succs.get(n)) {
            layer.set(next, Math.max(layer.get(next), layer.get(n) + 1));
            const d = indegree.get(next) - 1;
            indegree.set(next, d);
            if (d === 0)
                queue.push(next);
        }
    }
    return layer;
}
function orderLayers(layerOf, edges) {
    const layers = new Map();
    for (const [n, l] of layerOf) {
        if (!layers.has(l))
            layers.set(l, []);
        layers.get(l).push(n);
    }
    const layerIndices = [...layers.keys()].sort((a, b) => a - b);
    const position = new Map();
    const reindex = () => {
        for (const l of layerIndices) {
            layers.get(l).forEach((n, i) => position.set(n, i));
        }
    };
    reindex();
    const preds = new Map();
    const succs = new Map();
    for (const e of edges) {
        if (!succs.has(e.source))
            succs.set(e.source, []);
        if (!preds.has(e.target))
            preds.set(e.target, []);
        succs.get(e.source).push(e.target);
        preds.get(e.target).push(e.source);
    }
    const sweep = (neighborsOf, order) => {
        for (const l of order) {
            const row = layers.get(l);
            const bary = new Map();
            for (const n of row) {
                const neigh = neighborsOf.get(n) ?? [];
                bary.set(n, neigh.length
                    ? neigh.reduce((sum, m) => sum + (position.get(m) ?? 0), 0) / neigh.length
                    : (position.get(n) ?? 0));
            }
            row.sort((a, b) => bary.get(a) - bary.get(b));
            reindex();
        }
    };
    for (let i = 0; i < 4; i++) {
        sweep(preds, layerIndices);
        sweep(succs, [...layerIndices].reverse());
    }
    return layers;
}
function layoutComponent(comp, opts) {
    const back = findBackEdges(comp.nodes, comp.edges);
    const acyclic = comp.edges.filter((e) => !back.has(e));
    const layerOf = assignLayers(comp.nodes, acyclic);
    const layers = orderLayers(layerOf, acyclic);
    const layerIndices = [...layers.keys()].sort((a, b) => a - b);
    const tallest = Math.max(...layerIndices.map((l) => layers.get(l).length));
    const fullHeight = tallest * opts.nodeHeight + (tallest - 1) * opts.gapY;
    const positions = new Map();
    let width = 0;
    for (const l of layerIndices) {
        const row = layers.get(l);
        const rowHeight = row.length * opts.nodeHeight + (row.length - 1) * opts.gapY;
        const yOffset = (fullHeight - rowHeight) / 2;
        row.forEach((n, i) => {
            const x = l * (opts.nodeWidth + opts.gapX);
            const y = yOffset + i * (opts.nodeHeight + opts.gapY);
            positions.set(n, { id: n, x, y });
            width = Math.max(width, x + opts.nodeWidth);
        });
    }
    return { positions, width, height: fullHeight };
}
function layoutGraph(nodeIds, edges, opts, clusterEdges = edges) {
    if (nodeIds.length === 0) {
        return { positions: new Map(), width: 0, height: 0 };
    }
    const comps = components(nodeIds, clusterEdges);
    const idSetPerComp = comps.map((c) => new Set(c.nodes));
    const positions = new Map();
    let width = 0;
    let y = 0;
    for (let i = 0; i < comps.length; i++) {
        const layerEdges = edges.filter((e) => idSetPerComp[i].has(e.source) && idSetPerComp[i].has(e.target));
        const laid = layoutComponent({ nodes: comps[i].nodes, edges: layerEdges }, opts);
        for (const p of laid.positions.values()) {
            positions.set(p.id, { id: p.id, x: p.x, y: p.y + y });
        }
        width = Math.max(width, laid.width);
        y += laid.height + opts.componentGap;
    }
    return { positions, width, height: y - opts.componentGap };
}
function layoutGrid(nodeIds, opts) {
    if (nodeIds.length === 0) {
        return { positions: new Map(), width: 0, height: 0 };
    }
    const cols = Math.max(1, Math.ceil(Math.sqrt(nodeIds.length * 1.6)));
    const positions = new Map();
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

const MAX_DIFF_CELLS = 4_000_000;
const MAX_DIFF_LINES = 100_000;
const DEFAULT_CONTEXT_LINES = 3;
const MIN_FOLD_LINES = 2;
function splitLines(text) {
    if (text === "")
        return [];
    const normalized = text.replace(/\r\n?/g, "\n").replace(/\n$/, "");
    if (normalized === "")
        return [""];
    return normalized.split("\n");
}
function asLines(texts, kind) {
    return texts.map((text) => ({ kind, text }));
}
function diffLines(oldText, newText) {
    const oldLines = splitLines(oldText);
    const newLines = splitLines(newText);
    if (oldLines.length > MAX_DIFF_LINES || newLines.length > MAX_DIFF_LINES) {
        return null;
    }
    if (oldLines.length === 0)
        return asLines(newLines, "added");
    if (newLines.length === 0)
        return asLines(oldLines, "removed");
    let head = 0;
    while (head < oldLines.length &&
        head < newLines.length &&
        oldLines[head] === newLines[head]) {
        head++;
    }
    let oldEnd = oldLines.length;
    let newEnd = newLines.length;
    while (oldEnd > head &&
        newEnd > head &&
        oldLines[oldEnd - 1] === newLines[newEnd - 1]) {
        oldEnd--;
        newEnd--;
    }
    if ((oldEnd - head + 1) * (newEnd - head + 1) > MAX_DIFF_CELLS) {
        return null;
    }
    return [
        ...asLines(oldLines.slice(0, head), "context"),
        ...lcsDiff(oldLines.slice(head, oldEnd), newLines.slice(head, newEnd)),
        ...asLines(oldLines.slice(oldEnd), "context"),
    ];
}
function lcsDiff(oldLines, newLines) {
    const n = oldLines.length;
    const m = newLines.length;
    if (n === 0)
        return asLines(newLines, "added");
    if (m === 0)
        return asLines(oldLines, "removed");
    const stride = m + 1;
    const table = new Int32Array((n + 1) * stride);
    for (let i = n - 1; i >= 0; i--) {
        for (let j = m - 1; j >= 0; j--) {
            table[i * stride + j] =
                oldLines[i] === newLines[j]
                    ? table[(i + 1) * stride + j + 1] + 1
                    : Math.max(table[(i + 1) * stride + j], table[i * stride + j + 1]);
        }
    }
    const rows = [];
    let i = 0;
    let j = 0;
    while (i < n && j < m) {
        if (oldLines[i] === newLines[j]) {
            rows.push({ kind: "context", text: oldLines[i] });
            i++;
            j++;
        }
        else if (table[(i + 1) * stride + j] >= table[i * stride + j + 1]) {
            rows.push({ kind: "removed", text: oldLines[i] });
            i++;
        }
        else {
            rows.push({ kind: "added", text: newLines[j] });
            j++;
        }
    }
    while (i < n)
        rows.push({ kind: "removed", text: oldLines[i++] });
    while (j < m)
        rows.push({ kind: "added", text: newLines[j++] });
    return rows;
}
function isUnchanged(lines) {
    return lines.every((line) => line.kind === "context");
}
function foldContext(lines, contextLines = DEFAULT_CONTEXT_LINES) {
    const keep = new Array(lines.length).fill(false);
    for (let i = 0; i < lines.length; i++) {
        if (lines[i].kind === "context")
            continue;
        keep[i] = true;
        for (let d = 1; d <= contextLines; d++) {
            if (i - d >= 0)
                keep[i - d] = true;
            if (i + d < lines.length)
                keep[i + d] = true;
        }
    }
    const rows = [];
    let run = 0;
    let runStart = 0;
    const flush = () => {
        if (run === 0)
            return;
        if (run >= MIN_FOLD_LINES) {
            rows.push({ kind: "fold", count: run });
        }
        else {
            rows.push(...lines.slice(runStart, runStart + run));
        }
        run = 0;
    };
    for (let i = 0; i < lines.length; i++) {
        if (keep[i]) {
            flush();
            rows.push(lines[i]);
        }
        else {
            if (run === 0)
                runStart = i;
            run++;
        }
    }
    flush();
    return rows;
}

function smoothPath(points) {
    if (points.length === 0)
        return "";
    if (points.length === 1)
        return `M ${points[0].x} ${points[0].y}`;
    let d = `M ${points[0].x} ${points[0].y}`;
    for (let i = 0; i < points.length - 1; i++) {
        const p0 = points[i === 0 ? 0 : i - 1];
        const p1 = points[i];
        const p2 = points[i + 1];
        const p3 = points[i + 2 < points.length ? i + 2 : points.length - 1];
        const cp1x = p1.x + (p2.x - p0.x) / 6;
        const cp1y = p1.y + (p2.y - p0.y) / 6;
        const cp2x = p2.x - (p3.x - p1.x) / 6;
        const cp2y = p2.y - (p3.y - p1.y) / 6;
        d += ` C ${cp1x} ${cp1y}, ${cp2x} ${cp2y}, ${p2.x} ${p2.y}`;
    }
    return d;
}
function smoothAreaPath(points, baselineY) {
    if (points.length === 0)
        return "";
    if (points.length === 1) {
        const p = points[0];
        return `M ${p.x} ${baselineY} L ${p.x} ${p.y} L ${p.x} ${baselineY} Z`;
    }
    const line = smoothPath(points);
    const first = points[0];
    const last = points[points.length - 1];
    return `${line} L ${last.x} ${baselineY} L ${first.x} ${baselineY} Z`;
}
function niceMax(v) {
    if (v <= 0)
        return 4;
    const exp = Math.floor(Math.log10(v));
    const base = 10 ** exp;
    const norm = v / base;
    const niceNorm = norm <= 1 ? 1 : norm <= 2 ? 2 : norm <= 5 ? 5 : 10;
    return niceNorm * base;
}
function niceTicks(max) {
    const m = niceMax(max);
    if (m <= 5)
        return Array.from({ length: m + 1 }, (_, i) => i);
    const step = m / 4;
    return [0, 1, 2, 3, 4].map((i) => Math.round(step * i));
}

const timestamp = value => new Date(/[zZ]|[+-]\d\d:\d\d$/.test(value || '') ? value : `${String(value || '').replace(' ', 'T')}Z`);
const actorName = row => row.actor_display_name || row.display_name || row.actor_username || row.username || 'system';
const activityHref = (project, row) => {
  const base=`/${encodeURIComponent(project)}`;
  switch(row.entity_type) {
    case 'issue': return row.entity_label ? `${base}/issues/${encodeURIComponent(row.entity_label)}` : null;
    case 'page': return `${base}/pages/${row.entity_id}`;
    case 'module': return `${base}/modules/${row.entity_id}`;
    case 'comment': return row.issue_id != null && row.entity_label ? `${base}/issues/${encodeURIComponent(row.entity_label)}` : row.page_id != null ? `${base}/pages/${row.page_id}` : null;
    default: return null;
  }
};
const activityVerb = row => {
  switch(row.action) {
    case 'create': return row.entity_type==='comment'?'commented on':`created ${row.entity_type}`;
    case 'delete': return row.entity_type==='comment'?'deleted a comment on':`deleted ${row.entity_type}`;
    case 'update': return row.entity_type==='comment'?'edited a comment on':`changed ${row.field} on`;
    case 'attach': return 'labeled';case 'detach':return 'unlabeled';
    case 'link': return `linked ${(row.field||'relates_to').replaceAll('_',' ')}`;
    case 'unlink': return `unlinked ${(row.field||'relates_to').replaceAll('_',' ')}`;
    default: return row.action;
  }
};
const filterActivity = (rows,{projectId,actor='all',query='',start='',end=''}={}) => rows.filter(row => {
  const day=timestamp(row.ts).toLocaleDateString('en-CA');
  return (projectId==null||row.project_id===projectId) &&
    (actor==='all'||(actor==='system'?row.actor_user_id==null:row.actor_user_id===Number(actor))) &&
    (!start||day>=start)&&(!end||day<=end)&&
    (!query||[actorName(row),row.entity_label,row.entity_type,row.action,row.field,row.old_value,row.new_value,row.transport].join(' ').toLowerCase().includes(query.trim().toLowerCase()));
});
const insightSeries = data => {
  const created=new Map((data.created_per_week||[]).map(row=>[row.week_start,row.count]));
  const closed=new Map((data.closed_per_week||[]).map(row=>[row.week_start,row.count]));
  return [...new Set([...created.keys(),...closed.keys()])].sort().map(week=>({week,created:created.get(week)??null,closed:closed.get(week)??null}));
};
const graphPartition = (issues,relations,showClosed) => {
  const visible=showClosed?issues:issues.filter(row=>['backlog','todo','active'].includes(row.status));
  const ids=new Set(visible.map(row=>row.id));
  const edges=relations.filter(row=>ids.has(row.source_id)&&ids.has(row.target_id));
  const linked=new Set(edges.flatMap(row=>[row.source_id,row.target_id]));
  return {linked:visible.filter(row=>linked.has(row.id)),unlinked:visible.filter(row=>!linked.has(row.id)),relations:edges};
};
const api={timestamp,actorName,activityHref,activityVerb,filterActivity,insightSeries,graphPartition,layoutGraph,layoutGrid,diffLines,foldContext,isUnchanged,smoothPath,smoothAreaPath,niceTicks};
globalThis.LificTopcoatAnalyticsModel=api;if(typeof module!=='undefined')module.exports=api;
})();

<script lang="ts">
  /*
   * LIF-363 — Dependency graph: a project's issues as nodes, `blocks`
   * relations as edges, laid out left-to-right so blockers sit left of the
   * work they hold up. Requested by jbakesthefirst in Discord.
   *
   * Rendering approach: HTML node cards absolutely positioned over an SVG
   * edge layer, both inside ONE pan/zoom-transformed container. Not SVG
   * foreignObject (mobile Safari transform bugs), not mermaid (static SVG,
   * no click handlers). HTML cards get CSS truncation, the shared
   * StatusIcon/PriorityIcon vocabulary, and real <button> semantics for free.
   *
   * Interaction model: drag anywhere to pan (including starting on a node —
   * essential on touch), wheel scrolls, ctrl/cmd+wheel or pinch zooms,
   * arrow keys pan, +/- zoom, 0 refits. A click only counts as a click when
   * the gesture didn't travel, so panning over a node never navigates.
   */
  import {
    listProjects,
    listIssues,
    listProjectRelations,
    type Project,
    type Issue,
    type ProjectRelation,
  } from "../lib/api";
  import { layoutGraph } from "../lib/graph/layout";
  import StatusIcon from "../lib/StatusIcon.svelte";
  import PriorityIcon from "../lib/PriorityIcon.svelte";
  import Mascot from "../lib/Mascot.svelte";
  import ErrorState from "../lib/ErrorState.svelte";
  import Skeleton from "../lib/Skeleton.svelte";
  import { ChevronRight, Plus, Minus, Maximize, MoveRight } from "lucide-svelte";
  import { getContext } from "svelte";

  const topbarCtx = getContext<{
    set: (s: import("svelte").Snippet | undefined) => void;
  } | undefined>("lific:topbar");

  $effect(() => {
    topbarCtx?.set(topbarContent);
    return () => topbarCtx?.set(undefined);
  });

  let {
    navigate,
    projectIdentifier,
  }: {
    navigate: (path: string) => void;
    projectIdentifier: string;
  } = $props();

  // ── Geometry constants ────────────────────────────────────
  const NODE_W = 200;
  const NODE_H = 58;
  const GAP_X = 90;
  const GAP_Y = 18;
  const COMPONENT_GAP = 48;
  const FIT_PAD = 40;
  const MIN_SCALE = 0.2;
  const MAX_SCALE = 2;

  // ── Data ──────────────────────────────────────────────────
  let project = $state<Project | null>(null);
  let issues = $state<Issue[]>([]);
  let relations = $state<ProjectRelation[]>([]);
  let loading = $state(true);
  let error = $state("");
  let showClosed = $state(false);

  $effect(() => {
    const id = projectIdentifier;
    showClosed = false;
    fitted = false;
    loadProject(id);
  });

  async function loadProject(ident: string) {
    loading = true;
    error = "";
    const projRes = await listProjects();
    if (!projRes.ok) { error = projRes.error; loading = false; return; }
    const found = projRes.data.find((p) => p.identifier === ident);
    if (!found) { error = `Project ${ident} not found`; loading = false; return; }
    project = found;
    const [issueRes, relRes] = await Promise.all([
      listIssues({ project_id: found.id, limit: 1000 }),
      listProjectRelations(found.id),
    ]);
    if (!issueRes.ok) { error = issueRes.error; loading = false; return; }
    if (!relRes.ok) { error = relRes.error; loading = false; return; }
    issues = issueRes.data;
    relations = relRes.data;
    loading = false;
  }

  // ── Graph derivation ──────────────────────────────────────
  const OPEN = new Set(["backlog", "todo", "active"]);

  let visibleIssues = $derived(
    showClosed ? issues : issues.filter((i) => OPEN.has(i.status)),
  );
  let visibleIds = $derived(new Set(visibleIssues.map((i) => i.id)));
  let issueById = $derived(new Map(issues.map((i) => [i.id, i])));

  let blockEdges = $derived(
    relations.filter(
      (r) =>
        r.relation_type === "blocks" &&
        visibleIds.has(r.source_id) &&
        visibleIds.has(r.target_id),
    ),
  );

  // Singletons (no blocking edge either way) are excluded from the canvas —
  // the acceptance criteria call for hiding them so the graph reads as
  // structure, not a soup of disconnected dots. A count chip says how many
  // sit outside the chains.
  let connectedIds = $derived.by(() => {
    const s = new Set<number>();
    for (const e of blockEdges) {
      s.add(e.source_id);
      s.add(e.target_id);
    }
    return s;
  });
  let graphIssues = $derived(visibleIssues.filter((i) => connectedIds.has(i.id)));
  let singletonCount = $derived(visibleIssues.length - graphIssues.length);

  let layout = $derived(
    layoutGraph(
      graphIssues.map((i) => i.id),
      blockEdges.map((e) => ({ source: e.source_id, target: e.target_id })),
      {
        nodeWidth: NODE_W,
        nodeHeight: NODE_H,
        gapX: GAP_X,
        gapY: GAP_Y,
        componentGap: COMPONENT_GAP,
      },
    ),
  );

  function edgePath(e: ProjectRelation): string {
    const s = layout.positions.get(e.source_id);
    const t = layout.positions.get(e.target_id);
    if (!s || !t) return "";
    const x1 = s.x + NODE_W;
    const y1 = s.y + NODE_H / 2;
    const x2 = t.x;
    const y2 = t.y + NODE_H / 2;
    const dx = Math.max(40, Math.abs(x2 - x1) / 2);
    return `M ${x1} ${y1} C ${x1 + dx} ${y1}, ${x2 - dx} ${y2}, ${x2} ${y2}`;
  }

  // ── Pan / zoom ────────────────────────────────────────────
  let viewportEl = $state<HTMLElement | null>(null);
  let viewportW = $state(0);
  let viewportH = $state(0);
  let tx = $state(0);
  let ty = $state(0);
  let scale = $state(1);
  let fitted = $state(false);

  function clampScale(s: number): number {
    return Math.min(MAX_SCALE, Math.max(MIN_SCALE, s));
  }

  function fit() {
    if (layout.width <= 0 || viewportW <= 0) return;
    scale = clampScale(
      Math.min(
        (viewportW - FIT_PAD * 2) / layout.width,
        (viewportH - FIT_PAD * 2) / layout.height,
        1,
      ),
    );
    tx = (viewportW - layout.width * scale) / 2;
    ty = (viewportH - layout.height * scale) / 2;
  }

  // Auto-fit once the layout and the viewport are both known; refits after
  // a project switch or a show-closed toggle (both reset `fitted`).
  $effect(() => {
    void layout;
    if (!fitted && viewportW > 0 && layout.width > 0) {
      fit();
      fitted = true;
    }
  });

  function zoomAt(cx: number, cy: number, factor: number) {
    const next = clampScale(scale * factor);
    // Keep the graph point under (cx, cy) stationary.
    tx = cx - ((cx - tx) / scale) * next;
    ty = cy - ((cy - ty) / scale) * next;
    scale = next;
  }

  // Svelte 5 registers `onwheel` passively, so preventDefault (needed to keep
  // the page from scrolling/zooming under the canvas) requires a manual
  // non-passive listener.
  $effect(() => {
    const el = viewportEl;
    if (!el) return;
    const onWheel = (e: WheelEvent) => {
      e.preventDefault();
      const rect = el.getBoundingClientRect();
      if (e.ctrlKey || e.metaKey) {
        zoomAt(e.clientX - rect.left, e.clientY - rect.top, Math.exp(-e.deltaY * 0.01));
      } else {
        tx -= e.deltaX;
        ty -= e.deltaY;
      }
    };
    el.addEventListener("wheel", onWheel, { passive: false });
    return () => el.removeEventListener("wheel", onWheel);
  });

  // One gesture model for mouse-drag pan, touch pan, and pinch zoom. A
  // gesture accumulates travel; node clicks are suppressed when it moved.
  const pointers = new Map<number, { x: number; y: number }>();
  let panning = $state(false);
  let gestureTravel = 0;
  let pinch: { dist: number; scale: number; gx: number; gy: number } | null = null;

  function viewportPoint(e: PointerEvent): { x: number; y: number } {
    const rect = viewportEl!.getBoundingClientRect();
    return { x: e.clientX - rect.left, y: e.clientY - rect.top };
  }

  function onPointerDown(e: PointerEvent) {
    if (e.button !== 0 && e.pointerType === "mouse") return;
    pointers.set(e.pointerId, viewportPoint(e));
    gestureTravel = 0;
    panning = true;
    if (pointers.size === 2) {
      const [a, b] = [...pointers.values()];
      const mid = { x: (a.x + b.x) / 2, y: (a.y + b.y) / 2 };
      pinch = {
        dist: Math.hypot(a.x - b.x, a.y - b.y),
        scale,
        gx: (mid.x - tx) / scale,
        gy: (mid.y - ty) / scale,
      };
    }
  }

  function onPointerMove(e: PointerEvent) {
    const prev = pointers.get(e.pointerId);
    if (!prev) return;
    const p = viewportPoint(e);
    pointers.set(e.pointerId, p);
    gestureTravel += Math.hypot(p.x - prev.x, p.y - prev.y);

    if (pinch && pointers.size === 2) {
      const [a, b] = [...pointers.values()];
      const mid = { x: (a.x + b.x) / 2, y: (a.y + b.y) / 2 };
      const dist = Math.hypot(a.x - b.x, a.y - b.y);
      const next = clampScale(pinch.scale * (dist / pinch.dist));
      scale = next;
      tx = mid.x - pinch.gx * next;
      ty = mid.y - pinch.gy * next;
    } else if (pointers.size === 1) {
      tx += p.x - prev.x;
      ty += p.y - prev.y;
    }
  }

  function onPointerEnd(e: PointerEvent) {
    pointers.delete(e.pointerId);
    if (pointers.size < 2) pinch = null;
    if (pointers.size === 0) panning = false;
  }

  function nodeClick(issue: Issue) {
    // A pan that started on a node ends with a click event; ignore it.
    if (gestureTravel > 6) return;
    navigate(`/${projectIdentifier}/issues/${issue.identifier}`);
  }

  function onKeydown(e: KeyboardEvent) {
    const PAN = 60;
    if (e.key === "ArrowLeft") { tx += PAN; e.preventDefault(); }
    else if (e.key === "ArrowRight") { tx -= PAN; e.preventDefault(); }
    else if (e.key === "ArrowUp") { ty += PAN; e.preventDefault(); }
    else if (e.key === "ArrowDown") { ty -= PAN; e.preventDefault(); }
    else if (e.key === "+" || e.key === "=") { zoomAt(viewportW / 2, viewportH / 2, 1.2); e.preventDefault(); }
    else if (e.key === "-") { zoomAt(viewportW / 2, viewportH / 2, 1 / 1.2); e.preventDefault(); }
    else if (e.key === "0") { fit(); e.preventDefault(); }
  }

  function toggleClosed() {
    showClosed = !showClosed;
    fitted = false; // effect refits with the new node set
  }

  // ── Hover highlighting ────────────────────────────────────
  let hoveredId = $state<number | null>(null);
  function edgeHot(e: ProjectRelation): boolean {
    return hoveredId !== null && (e.source_id === hoveredId || e.target_id === hoveredId);
  }

  let isClosed = (i: Issue) => i.status === "done" || i.status === "cancelled";
  let hasAnyIssues = $derived(issues.length > 0);
  let hasGraph = $derived(blockEdges.length > 0);
</script>

{#snippet topbarContent()}
  <div class="flex items-center gap-3 px-6 py-2 w-full">
    <div class="flex items-center gap-1.5 shrink-0">
      <button
        class="text-body-sm font-mono font-medium text-[var(--text-muted)]
               hover:text-[var(--text)] transition-colors"
        onclick={() => navigate(`/${projectIdentifier}/overview`)}
      >
        {projectIdentifier}
      </button>
      <ChevronRight size={12} class="text-[var(--text-faint)]" />
      <span class="text-body-sm font-medium text-[var(--text)]">Graph</span>
    </div>
  </div>
{/snippet}

<div class="h-full flex flex-col">
  {#if loading}
    <div class="flex-1 p-6">
      <Skeleton variant="block" class="h-full w-full rounded-xl" />
    </div>
  {:else if error}
    <ErrorState title="Couldn't load the graph" message={error}>
      <button
        class="text-body-sm font-medium text-[var(--btn-success-text)] bg-[var(--btn-success)] px-3 py-1.5 rounded-md hover:bg-[var(--btn-success-hover)] transition-colors"
        onclick={() => loadProject(projectIdentifier)}
      >
        Try again
      </button>
    </ErrorState>
  {:else if !hasAnyIssues}
    <div class="flex flex-col items-center py-20 gap-4 px-6 max-w-[480px] mx-auto text-center">
      <Mascot src="/LizzySleep2.png" nativeW={1000} nativeH={420} scale={0.25} />
      <div class="flex flex-col items-center gap-1.5">
        <p class="text-heading font-medium text-[var(--text)]">Nothing to graph yet</p>
        <p class="text-body-sm text-[var(--text-muted)] leading-relaxed">
          The graph draws this project's blocking structure. Create some issues
          first, then link them with block relations.
        </p>
      </div>
    </div>
  {:else if !hasGraph}
    <div class="flex flex-col items-center py-20 gap-4 px-6 max-w-[520px] mx-auto text-center">
      <Mascot src="/LizzySleep2.png" nativeW={1000} nativeH={420} scale={0.25} />
      <div class="flex flex-col items-center gap-1.5">
        <p class="text-heading font-medium text-[var(--text)]">
          {showClosed ? "No dependencies yet" : "No open dependencies"}
        </p>
        <p class="text-body-sm text-[var(--text-muted)] leading-relaxed">
          {#if showClosed}
            No issues block each other yet. Add a block relation from an
            issue's detail page (or via <code class="font-mono text-caption">link_issues</code> over MCP)
            and the chains will draw themselves here.
          {:else}
            Every blocking chain is closed out. Toggle closed issues to see
            resolved history, or link open issues to map what's in the way.
          {/if}
        </p>
      </div>
      <div class="flex items-center gap-2">
        {#if !showClosed}
          <button
            class="text-body-sm text-[var(--text-muted)] border border-[var(--border)]
                   px-3 py-1.5 rounded-md hover:bg-[var(--bg-subtle)] transition-colors"
            onclick={toggleClosed}
          >
            Show closed issues
          </button>
        {/if}
        <button
          class="text-body-sm font-medium text-[var(--btn-success-text)] bg-[var(--btn-success)]
                 px-3 py-1.5 rounded-md hover:bg-[var(--btn-success-hover)] transition-colors"
          onclick={() => navigate(`/${projectIdentifier}/issues`)}
        >
          Go to issues
        </button>
      </div>
    </div>
  {:else}
    <!-- ── The canvas ─────────────────────────────────────── -->
    <!-- The canvas takes focus deliberately: arrow keys pan, +/- zoom, 0
         refits (announced via aria-label). The a11y rule can't see that. -->
    <!-- svelte-ignore a11y_no_noninteractive_element_interactions, a11y_no_noninteractive_tabindex -->
    <div
      bind:this={viewportEl}
      bind:clientWidth={viewportW}
      bind:clientHeight={viewportH}
      class="relative flex-1 min-h-0 overflow-hidden select-none
             {panning ? 'cursor-grabbing' : 'cursor-grab'}"
      style="touch-action: none;
             background-image: radial-gradient(color-mix(in srgb, var(--border) 60%, transparent) 1px, transparent 1px);
             background-size: {24 * scale}px {24 * scale}px;
             background-position: {tx}px {ty}px;"
      role="application"
      aria-label="Dependency graph. Drag to pan, pinch or ctrl+scroll to zoom, arrow keys to pan, 0 to re-center."
      tabindex="0"
      onpointerdown={onPointerDown}
      onpointermove={onPointerMove}
      onpointerup={onPointerEnd}
      onpointercancel={onPointerEnd}
      onkeydown={onKeydown}
    >
      <div
        class="absolute top-0 left-0 will-change-transform"
        style="transform: translate({tx}px, {ty}px) scale({scale}); transform-origin: 0 0;"
      >
        <svg
          class="absolute top-0 left-0 overflow-visible pointer-events-none"
          width={Math.max(1, layout.width)}
          height={Math.max(1, layout.height)}
          aria-hidden="true"
        >
          <defs>
            <marker
              id="dep-arrow"
              viewBox="0 0 8 8"
              refX="7"
              refY="4"
              markerWidth="7"
              markerHeight="7"
              orient="auto-start-reverse"
            >
              <path d="M 0 0 L 8 4 L 0 8 z" fill="var(--text-faint)" />
            </marker>
            <marker
              id="dep-arrow-hot"
              viewBox="0 0 8 8"
              refX="7"
              refY="4"
              markerWidth="7"
              markerHeight="7"
              orient="auto-start-reverse"
            >
              <path d="M 0 0 L 8 4 L 0 8 z" fill="var(--accent)" />
            </marker>
          </defs>
          {#each blockEdges as e (e.source_id + "-" + e.target_id)}
            <path
              d={edgePath(e)}
              fill="none"
              stroke={edgeHot(e) ? "var(--accent)" : "var(--text-faint)"}
              stroke-width={edgeHot(e) ? 2 : 1.5}
              stroke-opacity={edgeHot(e) ? 1 : 0.55}
              marker-end="url(#{edgeHot(e) ? 'dep-arrow-hot' : 'dep-arrow'})"
            />
          {/each}
        </svg>

        {#each graphIssues as issue (issue.id)}
          {@const pos = layout.positions.get(issue.id)}
          {#if pos}
            <button
              class="absolute text-left rounded-lg border bg-[var(--surface)]
                     shadow-[0_1px_2px_rgba(0,0,0,0.06)] px-2.5 py-1.5
                     transition-colors overflow-hidden
                     {hoveredId === issue.id
                ? 'border-[var(--accent)]'
                : 'border-[var(--border)] hover:border-[var(--accent)]'}
                     {isClosed(issue) ? 'opacity-50' : ''}"
              style="left: {pos.x}px; top: {pos.y}px; width: {NODE_W}px; height: {NODE_H}px;"
              onmouseenter={() => (hoveredId = issue.id)}
              onmouseleave={() => (hoveredId = null)}
              onclick={() => nodeClick(issue)}
            >
              <span class="flex items-center gap-1.5">
                <StatusIcon status={issue.status} size={12} />
                <span class="font-mono text-micro text-[var(--text-faint)]">
                  {issue.identifier}
                </span>
                <span class="ml-auto">
                  <PriorityIcon priority={issue.priority} size={12} />
                </span>
              </span>
              <span class="block text-caption text-[var(--text)] truncate leading-snug mt-0.5">
                {issue.title}
              </span>
            </button>
          {/if}
        {/each}
      </div>

      <!-- ── Floating controls ──────────────────────────────── -->
      <div
        class="absolute top-3 left-3 flex flex-wrap items-center gap-2 max-w-[calc(100%-1.5rem)]"
      >
        <button
          class="flex items-center gap-1.5 h-8 px-2.5 rounded-lg text-caption font-medium
                 border transition-colors
                 {showClosed
            ? 'bg-[var(--accent-subtle)] border-[var(--accent)] text-[var(--text)]'
            : 'bg-[var(--surface)] border-[var(--border)] text-[var(--text-muted)] hover:text-[var(--text)]'}"
          aria-pressed={showClosed}
          onclick={toggleClosed}
        >
          <StatusIcon status="done" size={12} />
          Closed issues
        </button>
        <span
          class="flex items-center gap-1.5 h-8 px-2.5 rounded-lg text-caption
                 bg-[var(--surface)] border border-[var(--border)] text-[var(--text-faint)]"
        >
          blocker <MoveRight size={12} /> blocked
        </span>
        {#if singletonCount > 0}
          <span
            class="flex items-center h-8 px-2.5 rounded-lg text-caption
                   bg-[var(--surface)] border border-[var(--border)] text-[var(--text-faint)]"
          >
            {singletonCount} issue{singletonCount === 1 ? "" : "s"} outside the chains
          </span>
        {/if}
      </div>

      <div class="absolute bottom-3 right-3 flex items-center gap-1">
        <div
          class="flex items-center rounded-lg bg-[var(--surface)] border border-[var(--border)]
                 shadow-[0_1px_2px_rgba(0,0,0,0.06)] overflow-hidden"
        >
          <button
            class="size-9 grid place-items-center text-[var(--text-muted)]
                   hover:text-[var(--text)] hover:bg-[var(--bg-subtle)] transition-colors"
            aria-label="Zoom out"
            onclick={() => zoomAt(viewportW / 2, viewportH / 2, 1 / 1.2)}
          >
            <Minus size={15} />
          </button>
          <span
            class="w-11 text-center text-micro tabular-nums text-[var(--text-faint)] select-none"
          >
            {Math.round(scale * 100)}%
          </span>
          <button
            class="size-9 grid place-items-center text-[var(--text-muted)]
                   hover:text-[var(--text)] hover:bg-[var(--bg-subtle)] transition-colors"
            aria-label="Zoom in"
            onclick={() => zoomAt(viewportW / 2, viewportH / 2, 1.2)}
          >
            <Plus size={15} />
          </button>
        </div>
        <button
          class="size-9 grid place-items-center rounded-lg bg-[var(--surface)]
                 border border-[var(--border)] shadow-[0_1px_2px_rgba(0,0,0,0.06)]
                 text-[var(--text-muted)] hover:text-[var(--text)]
                 hover:bg-[var(--bg-subtle)] transition-colors"
          aria-label="Fit graph to view"
          onclick={fit}
        >
          <Maximize size={15} />
        </button>
      </div>
    </div>
  {/if}
</div>

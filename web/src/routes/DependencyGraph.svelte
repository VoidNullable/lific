<script lang="ts">
  /*
   * LIF-363 — Dependency graph, now on Svelte Flow (@xyflow/svelte).
   *
   * Two canvases behind one toggle:
   *   Linked   — issues with at least one visible relation, laid out as a
   *              layered DAG (blocks edges drive the layering, other
   *              relation types only cluster). The default view.
   *   Unlinked — issues with no visible relations, packed in a grid so they
   *              can be wired up: drag from a card's right dot onto another
   *              card and a menu asks what kind of relation to create.
   *
   * Positions are never persisted. Both canvases relayout from scratch on
   * every load (and after every link/unlink) via {#key} remounts — the
   * layout algorithm is the source of truth, not saved coordinates.
   *
   * Editing: drag-to-connect opens the relation-type menu; clicking an edge
   * opens a manage menu (reverse / remove). Both are gated on the caller's
   * project role (viewer = read-only, same as the rest of the UI; the
   * server enforces regardless).
   */
  import { type Node, type Edge, MarkerType, Position, type Connection } from "@xyflow/svelte";
  import "@xyflow/svelte/dist/style.css";
  import {
    listProjects,
    listIssues,
    listProjectRelations,
    linkIssues,
    unlinkIssues,
    type Project,
    type Issue,
    type ProjectRelation,
    type RelationType,
  } from "../lib/api";
  import { layoutGraph, layoutGrid } from "../lib/graph/layout";
  import GraphCanvas from "../lib/graph/GraphCanvas.svelte";
  import Mascot from "../lib/Mascot.svelte";
  import ErrorState from "../lib/ErrorState.svelte";
  import Skeleton from "../lib/Skeleton.svelte";
  import StatusIcon from "../lib/StatusIcon.svelte";
  import { toast } from "../lib/toast/toast.svelte";
  import { projectRole, loadProjectRole } from "../lib/projectRole.svelte";
  import { ChevronRight, MoveRight, ArrowLeftRight, Unlink, X } from "lucide-svelte";
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

  // ── Geometry ──────────────────────────────────────────────
  const NODE_W = 200;
  const NODE_H = 58;
  const DAG_OPTS = { nodeWidth: NODE_W, nodeHeight: NODE_H, gapX: 90, gapY: 18, componentGap: 48 };
  const GRID_OPTS = { nodeWidth: NODE_W, nodeHeight: NODE_H, gapX: 24, gapY: 16 };

  // ── Data ──────────────────────────────────────────────────
  let project = $state<Project | null>(null);
  let issues = $state<Issue[]>([]);
  let relations = $state<ProjectRelation[]>([]);
  let loading = $state(true);
  let error = $state("");
  let showClosed = $state(false);
  let view = $state<"linked" | "unlinked">("linked");
  /** Bumped after every successful link/unlink; keys the canvas remount so
   *  both views relayout with fresh data (positions are never persisted). */
  let revision = $state(0);

  $effect(() => {
    const id = projectIdentifier;
    showClosed = false;
    view = "linked";
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
    loadProjectRole(found.id); // role-gates editing affordances (LIF-234)
    const ok = await reloadData();
    loading = false;
    if (ok) revision++;
  }

  async function reloadData(): Promise<boolean> {
    if (!project) return false;
    const [issueRes, relRes] = await Promise.all([
      listIssues({ project_id: project.id, limit: 1000 }),
      listProjectRelations(project.id),
    ]);
    if (!issueRes.ok) { error = issueRes.error; return false; }
    if (!relRes.ok) { error = relRes.error; return false; }
    issues = issueRes.data;
    relations = relRes.data;
    return true;
  }

  // ── Partition: linked vs unlinked ─────────────────────────
  const OPEN = new Set(["backlog", "todo", "active"]);

  let visibleIssues = $derived(
    showClosed ? issues : issues.filter((i) => OPEN.has(i.status)),
  );
  let visibleIds = $derived(new Set(visibleIssues.map((i) => i.id)));
  let issueById = $derived(new Map(issues.map((i) => [i.id, i])));

  /** Relations whose endpoints are both on-screen under the current filter. */
  let visibleRelations = $derived(
    relations.filter(
      (r) => visibleIds.has(r.source_id) && visibleIds.has(r.target_id),
    ),
  );

  let linkedIds = $derived.by(() => {
    const s = new Set<number>();
    for (const r of visibleRelations) {
      s.add(r.source_id);
      s.add(r.target_id);
    }
    return s;
  });
  let linkedIssues = $derived(visibleIssues.filter((i) => linkedIds.has(i.id)));
  let unlinkedIssues = $derived(visibleIssues.filter((i) => !linkedIds.has(i.id)));

  // ── Layouts → flow nodes/edges ────────────────────────────
  let linkedLayout = $derived(
    layoutGraph(
      linkedIssues.map((i) => i.id),
      visibleRelations
        .filter((r) => r.relation_type === "blocks")
        .map((r) => ({ source: r.source_id, target: r.target_id })),
      DAG_OPTS,
      visibleRelations.map((r) => ({ source: r.source_id, target: r.target_id })),
    ),
  );
  let unlinkedLayout = $derived(
    layoutGrid(unlinkedIssues.map((i) => i.id), GRID_OPTS),
  );

  let flowNodes = $derived.by<Node[]>(() => {
    const [list, layout] =
      view === "linked"
        ? [linkedIssues, linkedLayout]
        : [unlinkedIssues, unlinkedLayout];
    return list.map((issue) => ({
      id: String(issue.id),
      type: "issue",
      position: layout.positions.get(issue.id) ?? { x: 0, y: 0 },
      data: { issue },
      deletable: false,
      // Cards are fixed-size with fixed handle spots, so declare BOTH
      // statically (xyflow's SSR path). Without this, nodes hide behind
      // `visibility: hidden` and edges refuse to draw until a
      // ResizeObserver measurement pass lands — declared dimensions and
      // handle bounds make the whole graph render deterministically with
      // no dependency on that pass.
      width: NODE_W,
      height: NODE_H,
      handles: [
        { type: "target" as const, position: Position.Left, x: 0, y: NODE_H / 2 },
        { type: "source" as const, position: Position.Right, x: NODE_W, y: NODE_H / 2 },
      ],
    }));
  });

  function edgeStyle(t: string): { style: string; marker: boolean } {
    switch (t) {
      case "blocks":
        return { style: "stroke: var(--text-faint); stroke-opacity: 0.55; stroke-width: 1.5;", marker: true };
      case "duplicate":
        return { style: "stroke: var(--text-faint); stroke-opacity: 0.4; stroke-width: 1.5; stroke-dasharray: 2 3;", marker: true };
      default: // relates_to — undirected, drawn quietest
        return { style: "stroke: var(--text-faint); stroke-opacity: 0.4; stroke-width: 1.5; stroke-dasharray: 5 4;", marker: false };
    }
  }

  let relationByEdgeId = $derived(
    new Map(
      visibleRelations.map((r) => [
        `${r.source_id}:${r.target_id}:${r.relation_type}`,
        r,
      ]),
    ),
  );

  let flowEdges = $derived.by<Edge[]>(() => {
    if (view !== "linked") return [];
    return visibleRelations.map((r) => {
      const { style, marker } = edgeStyle(r.relation_type);
      return {
        id: `${r.source_id}:${r.target_id}:${r.relation_type}`,
        source: String(r.source_id),
        target: String(r.target_id),
        style,
        deletable: false,
        ...(marker
          ? { markerEnd: { type: MarkerType.ArrowClosed, color: "var(--text-faint)", width: 16, height: 16 } }
          : {}),
      };
    });
  });

  let editable = $derived(projectRole.canEdit);

  // ── Relation menus ────────────────────────────────────────
  type Menu =
    | { kind: "create"; source: Issue; target: Issue; x: number; y: number }
    | { kind: "edge"; relation: ProjectRelation; x: number; y: number };
  let menu = $state<Menu | null>(null);
  let busy = $state(false);

  /** Clamp a client-coords anchor so the menu never renders off-screen. */
  function anchored(x: number, y: number): { x: number; y: number } {
    return {
      x: Math.max(8, Math.min(x, window.innerWidth - 288)),
      y: Math.max(8, Math.min(y, window.innerHeight - 260)),
    };
  }

  // onconnect delivers the node pair but no pointer position; the position
  // arrives one tick later in onconnectend. Stash the pair, then place the
  // menu where the finger/cursor actually let go.
  let pendingPair: { source: Issue; target: Issue } | null = null;

  function onconnect(conn: Connection) {
    if (!editable || conn.source === conn.target) return;
    const source = issueById.get(Number(conn.source));
    const target = issueById.get(Number(conn.target));
    if (!source || !target) return;
    pendingPair = { source, target };
  }

  function onconnectend(event: MouseEvent | TouchEvent) {
    if (!pendingPair) return;
    const p = "changedTouches" in event ? event.changedTouches[0] : event;
    menu = { kind: "create", ...pendingPair, ...anchored(p.clientX, p.clientY) };
    pendingPair = null;
  }

  function onedgeclick({ edge, event }: { edge: Edge; event: MouseEvent | TouchEvent }) {
    if (!editable) return;
    const relation = relationByEdgeId.get(edge.id);
    if (!relation) return;
    const p = "changedTouches" in event ? event.changedTouches[0] : event;
    menu = { kind: "edge", relation, ...anchored(p.clientX, p.clientY) };
  }

  async function createRelation(source: Issue, target: Issue, type: RelationType) {
    busy = true;
    const res = await linkIssues(source.identifier, target.identifier, type);
    busy = false;
    menu = null;
    if (!res.ok) {
      toast(res.error, { kind: "error" });
      return;
    }
    const wasUnlinked = view === "unlinked";
    await reloadData();
    revision++;
    toast(
      wasUnlinked
        ? `Linked — ${source.identifier} and ${target.identifier} moved to the Linked view.`
        : `Linked ${source.identifier} and ${target.identifier}.`,
      { kind: "success" },
    );
  }

  async function removeRelation(r: ProjectRelation) {
    busy = true;
    const res = await unlinkIssues(r.source_identifier, r.target_identifier);
    busy = false;
    menu = null;
    if (!res.ok) {
      toast(res.error, { kind: "error" });
      return;
    }
    await reloadData();
    revision++;
    toast(`Removed the link between ${r.source_identifier} and ${r.target_identifier}.`, { kind: "success" });
  }

  async function reverseRelation(r: ProjectRelation) {
    busy = true;
    const un = await unlinkIssues(r.source_identifier, r.target_identifier);
    if (!un.ok) {
      busy = false;
      menu = null;
      toast(un.error, { kind: "error" });
      return;
    }
    const re = await linkIssues(
      r.target_identifier,
      r.source_identifier,
      r.relation_type as RelationType,
    );
    busy = false;
    menu = null;
    if (!re.ok) {
      toast(re.error, { kind: "error" });
    }
    await reloadData();
    revision++;
  }

  function relationVerb(t: string): string {
    return t === "blocks" ? "blocks" : t === "duplicate" ? "duplicates" : "relates to";
  }

  function onKeydown(e: KeyboardEvent) {
    if (e.key === "Escape" && menu) {
      menu = null;
      e.stopPropagation();
    }
  }

  let hasAnyIssues = $derived(issues.length > 0);
</script>

<svelte:window onkeydown={onKeydown} />

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
          first, then link them up right here.
        </p>
      </div>
    </div>
  {:else}
    <div class="relative flex-1 min-h-0 dep-graph">
      {#key `${view}-${showClosed}-${revision}`}
        {#if flowNodes.length === 0}
          <!-- Empty state for the current canvas; the switcher stays usable. -->
          <div class="h-full flex flex-col items-center justify-center gap-4 px-6 text-center">
            <Mascot src="/LizzySleep2.png" nativeW={1000} nativeH={420} scale={0.22} />
            <div class="flex flex-col items-center gap-1.5 max-w-[440px]">
              {#if view === "linked"}
                <p class="text-heading font-medium text-[var(--text)]">
                  {showClosed ? "No links yet" : "No open links"}
                </p>
                <p class="text-body-sm text-[var(--text-muted)] leading-relaxed">
                  {#if unlinkedIssues.length > 0}
                    Switch to Unlinked and drag between cards to start
                    building chains — drag from a card's right dot onto
                    another card.
                  {:else if !showClosed}
                    Every linked chain is closed out. Toggle closed issues to
                    see the history.
                  {:else}
                    No issues are linked to each other yet.
                  {/if}
                </p>
              {:else}
                <p class="text-heading font-medium text-[var(--text)]">Everything is linked</p>
                <p class="text-body-sm text-[var(--text-muted)] leading-relaxed">
                  No loose issues under the current filter — every visible
                  issue is part of a chain.
                </p>
              {/if}
            </div>
          </div>
        {:else}
          <GraphCanvas
            initialNodes={flowNodes}
            initialEdges={flowEdges}
            {editable}
            {onconnect}
            {onconnectend}
            {onedgeclick}
            onnodeopen={(node) => {
              const issue = (node.data as { issue: Issue }).issue;
              navigate(`/${projectIdentifier}/issues/${issue.identifier}`);
            }}
            onpaneclick={() => (menu = null)}
          />
        {/if}
      {/key}

      <!-- ── View switcher + filters (floating, works on all canvases) ── -->
      <div class="absolute top-3 left-3 z-10 flex flex-wrap items-center gap-2 max-w-[calc(100%-1.5rem)]">
        <div
          class="inline-flex p-0.5 rounded-lg bg-[var(--surface)] border border-[var(--border)]
                 shadow-[0_1px_2px_rgba(0,0,0,0.06)]"
        >
          <button
            class="px-2.5 py-1 rounded-md text-caption font-medium transition
                   {view === 'linked'
              ? 'bg-[var(--bg-subtle)] text-[var(--text)]'
              : 'text-[var(--text-muted)] hover:text-[var(--text)]'}"
            onclick={() => (view = "linked")}
          >
            Linked
            <span class="text-[var(--text-faint)] tabular-nums ml-0.5">{linkedIssues.length}</span>
          </button>
          <button
            class="px-2.5 py-1 rounded-md text-caption font-medium transition
                   {view === 'unlinked'
              ? 'bg-[var(--bg-subtle)] text-[var(--text)]'
              : 'text-[var(--text-muted)] hover:text-[var(--text)]'}"
            onclick={() => (view = "unlinked")}
          >
            Unlinked
            <span class="text-[var(--text-faint)] tabular-nums ml-0.5">{unlinkedIssues.length}</span>
          </button>
        </div>

        <button
          class="flex items-center gap-1.5 h-8 px-2.5 rounded-lg text-caption font-medium
                 border transition-colors
                 {showClosed
            ? 'bg-[var(--accent-subtle)] border-[var(--accent)] text-[var(--text)]'
            : 'bg-[var(--surface)] border-[var(--border)] text-[var(--text-muted)] hover:text-[var(--text)]'}"
          aria-pressed={showClosed}
          onclick={() => (showClosed = !showClosed)}
        >
          <StatusIcon status="done" size={12} />
          Closed issues
        </button>

        {#if view === "linked"}
          <span
            class="hidden sm:flex items-center gap-1.5 h-8 px-2.5 rounded-lg text-caption
                   bg-[var(--surface)] border border-[var(--border)] text-[var(--text-faint)]"
          >
            blocker <MoveRight size={12} /> blocked
          </span>
        {:else if editable}
          <span
            class="hidden sm:flex items-center h-8 px-2.5 rounded-lg text-caption
                   bg-[var(--surface)] border border-[var(--border)] text-[var(--text-faint)]"
          >
            Drag between cards to link them
          </span>
        {/if}
      </div>

      <!-- ── Relation menu ─────────────────────────────────── -->
      {#if menu}
        <div
          class="fixed z-50 w-70 rounded-lg bg-[var(--surface)] border border-[var(--border)]
                 shadow-[0_8px_24px_rgba(0,0,0,0.16)] py-1.5"
          style="left: {menu.x}px; top: {menu.y}px;"
          role="menu"
        >
          {#if menu.kind === "create"}
            {@const m = menu}
            <div class="flex items-center gap-2 px-3 pb-1.5 border-b border-[var(--border)]">
              <span class="text-caption font-medium text-[var(--text-muted)]">
                Link {m.source.identifier} and {m.target.identifier}
              </span>
              <button
                class="ml-auto size-6 grid place-items-center rounded text-[var(--text-faint)]
                       hover:text-[var(--text)] hover:bg-[var(--bg-subtle)] transition-colors"
                aria-label="Cancel"
                onclick={() => (menu = null)}
              >
                <X size={13} />
              </button>
            </div>
            <div class="pt-1">
              <button
                class="w-full flex items-center gap-2 px-3 py-1.5 text-left text-body-sm text-[var(--text)]
                       hover:bg-[var(--bg-subtle)] transition-colors disabled:opacity-50"
                disabled={busy}
                onclick={() => createRelation(m.source, m.target, "blocks")}
              >
                <span class="font-mono text-micro text-[var(--text-faint)]">{m.source.identifier}</span>
                <MoveRight size={12} class="text-[var(--text-faint)]" />
                blocks
                <span class="font-mono text-micro text-[var(--text-faint)]">{m.target.identifier}</span>
              </button>
              <button
                class="w-full flex items-center gap-2 px-3 py-1.5 text-left text-body-sm text-[var(--text)]
                       hover:bg-[var(--bg-subtle)] transition-colors disabled:opacity-50"
                disabled={busy}
                onclick={() => createRelation(m.target, m.source, "blocks")}
              >
                <span class="font-mono text-micro text-[var(--text-faint)]">{m.target.identifier}</span>
                <MoveRight size={12} class="text-[var(--text-faint)]" />
                blocks
                <span class="font-mono text-micro text-[var(--text-faint)]">{m.source.identifier}</span>
              </button>
              <button
                class="w-full flex items-center gap-2 px-3 py-1.5 text-left text-body-sm text-[var(--text)]
                       hover:bg-[var(--bg-subtle)] transition-colors disabled:opacity-50"
                disabled={busy}
                onclick={() => createRelation(m.source, m.target, "relates_to")}
              >
                <ArrowLeftRight size={12} class="text-[var(--text-faint)]" />
                Relates to <span class="text-micro text-[var(--text-faint)]">(no direction)</span>
              </button>
              <button
                class="w-full flex items-center gap-2 px-3 py-1.5 text-left text-body-sm text-[var(--text)]
                       hover:bg-[var(--bg-subtle)] transition-colors disabled:opacity-50"
                disabled={busy}
                onclick={() => createRelation(m.source, m.target, "duplicate")}
              >
                <span class="font-mono text-micro text-[var(--text-faint)]">{m.source.identifier}</span>
                duplicates
                <span class="font-mono text-micro text-[var(--text-faint)]">{m.target.identifier}</span>
              </button>
            </div>
          {:else}
            {@const r = menu.relation}
            <div class="flex items-center gap-2 px-3 pb-1.5 border-b border-[var(--border)]">
              <span class="text-caption font-medium text-[var(--text-muted)]">
                {r.source_identifier} {relationVerb(r.relation_type)} {r.target_identifier}
              </span>
              <button
                class="ml-auto size-6 grid place-items-center rounded text-[var(--text-faint)]
                       hover:text-[var(--text)] hover:bg-[var(--bg-subtle)] transition-colors"
                aria-label="Close"
                onclick={() => (menu = null)}
              >
                <X size={13} />
              </button>
            </div>
            <div class="pt-1">
              {#if r.relation_type !== "relates_to"}
                <button
                  class="w-full flex items-center gap-2 px-3 py-1.5 text-left text-body-sm text-[var(--text)]
                         hover:bg-[var(--bg-subtle)] transition-colors disabled:opacity-50"
                  disabled={busy}
                  onclick={() => reverseRelation(r)}
                >
                  <ArrowLeftRight size={13} class="text-[var(--text-faint)]" />
                  Reverse direction
                </button>
              {/if}
              <button
                class="w-full flex items-center gap-2 px-3 py-1.5 text-left text-body-sm text-[var(--error)]
                       hover:bg-[var(--bg-subtle)] transition-colors disabled:opacity-50"
                disabled={busy}
                onclick={() => removeRelation(r)}
              >
                <Unlink size={13} />
                Remove link
              </button>
            </div>
          {/if}
        </div>
      {/if}
    </div>
  {/if}
</div>

<style>
  /* Svelte Flow theming: its defaults assume white; point the pieces we
     actually use at the app's CSS vars so dark mode just works. */
  .dep-graph :global(.svelte-flow) {
    background: var(--bg);
  }
  .dep-graph :global(.svelte-flow__attribution) {
    background: transparent;
    color: var(--text-faint);
  }
  .dep-graph :global(.svelte-flow__handle) {
    /* Handles are styled per-node (IssueNode); keep hit area finger-sized
       without inflating the visible dot. */
    min-width: 10px;
    min-height: 10px;
  }
  .dep-graph :global(.svelte-flow__edge) {
    cursor: pointer;
  }
  .dep-graph :global(.svelte-flow__edge.selected .svelte-flow__edge-path) {
    stroke: var(--accent);
    stroke-opacity: 1;
  }
</style>

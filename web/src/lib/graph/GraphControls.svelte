<script lang="ts">
  // LIF-363 — themed zoom controls for the graph canvases. xyflow ships a
  // <Controls> plugin, but it carries its own visual language; this panel
  // reuses the app's button styling instead. Must render as a child of
  // <SvelteFlow> so useSvelteFlow() finds the store context.
  import { Panel, useSvelteFlow, useUpdateNodeInternals, useNodes, type Node } from "@xyflow/svelte";
  import { Plus, Minus, Maximize } from "lucide-svelte";

  const { zoomIn, zoomOut, fitView } = useSvelteFlow();
  const FIT = { padding: 0.15, duration: 200 };

  // Belt-and-suspenders re-measure after mount: edge anchor points need each
  // node's handle bounds, which normally arrive via a ResizeObserver pass.
  // Force one explicit pass for every node once the canvas is up so edges
  // never stay missing if that initial pass was dropped.
  const updateNodeInternals = useUpdateNodeInternals();
  const nodes = useNodes();
  $effect(() => {
    const ids = nodes.current.map((n: Node) => n.id);
    if (ids.length === 0) return;
    const t = setTimeout(() => updateNodeInternals(ids), 100);
    return () => clearTimeout(t);
  });
</script>

<Panel position="bottom-right">
  <div class="flex items-center gap-1">
    <div
      class="flex items-center rounded-lg bg-[var(--surface)] border border-[var(--border)]
             shadow-[0_1px_2px_rgba(0,0,0,0.06)] overflow-hidden"
    >
      <button
        class="size-9 grid place-items-center text-[var(--text-muted)]
               hover:text-[var(--text)] hover:bg-[var(--bg-subtle)] transition-colors"
        aria-label="Zoom out"
        onclick={() => zoomOut({ duration: 150 })}
      >
        <Minus size={15} />
      </button>
      <button
        class="size-9 grid place-items-center text-[var(--text-muted)]
               hover:text-[var(--text)] hover:bg-[var(--bg-subtle)] transition-colors"
        aria-label="Zoom in"
        onclick={() => zoomIn({ duration: 150 })}
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
      onclick={() => fitView(FIT)}
    >
      <Maximize size={15} />
    </button>
  </div>
</Panel>

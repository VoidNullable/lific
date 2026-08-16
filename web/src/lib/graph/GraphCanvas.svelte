<script lang="ts">
  /*
   * LIF-363 — one Svelte Flow canvas instance. Exists as its own component
   * so the route can remount it via {#key} with the node/edge arrays ready
   * SYNCHRONOUSLY at init: SvelteFlow's nodes/edges are $bindable state it
   * measures and fits during initialization, and populating them after
   * mount (route-level $effect) left every node stuck in the pre-measure
   * `visibility: hidden` state. Fresh mount, fresh arrays, fresh auto-fit —
   * which is also exactly the "positions are never persisted" behavior the
   * view wants.
   */
  import { SvelteFlow, Background, type Node, type Edge, type Connection } from "@xyflow/svelte";
  import IssueNode from "./IssueNode.svelte";
  import GraphControls from "./GraphControls.svelte";

  const nodeTypes = { issue: IssueNode };

  let {
    initialNodes,
    initialEdges,
    editable,
    onconnect,
    onconnectend,
    onedgeclick,
    onnodeopen,
    onpaneclick,
    onnodepointerenter,
    onnodepointerleave,
  }: {
    initialNodes: Node[];
    initialEdges: Edge[];
    editable: boolean;
    onconnect: (conn: Connection) => void;
    onconnectend: (event: MouseEvent | TouchEvent) => void;
    onedgeclick: (args: { edge: Edge; event: MouseEvent | TouchEvent }) => void;
    onnodeopen: (node: Node) => void;
    onpaneclick: () => void;
    onnodepointerenter: (args: { node: Node; event: PointerEvent }) => void;
    onnodepointerleave: (args: { node: Node; event: PointerEvent }) => void;
  } = $props();

  // Capturing only the initial prop value is the point (see doc comment):
  // data changes arrive as a fresh mount via the route's {#key}, never as a
  // prop update to a live canvas.
  // svelte-ignore state_referenced_locally
  let nodes = $state.raw<Node[]>(initialNodes);
  // svelte-ignore state_referenced_locally
  let edges = $state.raw<Edge[]>(initialEdges);
</script>

<SvelteFlow
  bind:nodes
  bind:edges
  {nodeTypes}
  fitView
  fitViewOptions={{ padding: 0.15 }}
  minZoom={0.1}
  maxZoom={2}
  deleteKey={null}
  nodesConnectable={editable}
  connectionRadius={36}
  connectionLineStyle="stroke: var(--accent); stroke-width: 2;"
  onnodeclick={({ node }) => onnodeopen(node)}
  {onconnect}
  {onconnectend}
  {onedgeclick}
  {onnodepointerenter}
  {onnodepointerleave}
  onpaneclick={() => onpaneclick()}
>
  <Background gap={24} patternColor="var(--border)" bgColor="var(--bg)" />
  <GraphControls />
</SvelteFlow>

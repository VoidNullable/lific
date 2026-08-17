<script lang="ts">
  // LIF-418: inline viewer for structured data attachments.
  //
  // Two modes behind one card, because they answer the same question ("what is
  // in this file") and share the whole lazy-fetch + frame apparatus:
  //   - table: CSV/TSV, first 200 rows, click a header to sort client-side.
  //   - json:  a collapsible tree (see JsonNode) that renders children only
  //            when a node is open.
  //
  // Sorting is view-only and never round-trips to the server; the file is
  // whatever it was and the download button still hands over the original.

  import { Table, Braces, ArrowUp, ArrowDown } from "lucide-svelte";
  import ViewerFrame from "./ViewerFrame.svelte";
  import JsonNode from "./JsonNode.svelte";
  import { whenVisible } from "./visible";
  import { fetchAttachmentText } from "../../api";
  import { detectDelimiter, parseDelimited, sortRows, type ParsedTable } from "./csv";
  import { MAX_INLINE_BYTES } from "./kind";

  let {
    id,
    filename,
    sizeBytes = null,
    mode,
  }: {
    id: number;
    filename: string;
    sizeBytes?: number | null;
    mode: "table" | "json";
  } = $props();

  const MAX_ROWS = 200;

  let text = $state<string | null>(null);
  let loading = $state(false);
  let loadError = $state<string | null>(null);
  let expanded = $state(false);
  let sortColumn = $state<number | null>(null);
  let sortDirection = $state<"asc" | "desc">("asc");

  async function load() {
    if (text !== null || loading) return;
    loading = true;
    const res = await fetchAttachmentText(id);
    loading = false;
    if (!res.ok) {
      loadError = res.error;
      return;
    }
    if (res.text.length > MAX_INLINE_BYTES) {
      loadError = "This file is too large to preview inline.";
      return;
    }
    text = res.text;
  }

  const table = $derived.by<ParsedTable | null>(() => {
    if (mode !== "table" || text === null) return null;
    return parseDelimited(text, {
      delimiter: detectDelimiter(filename, text),
      maxRows: MAX_ROWS,
    });
  });

  const sortedRows = $derived.by<string[][]>(() => {
    if (!table) return [];
    if (sortColumn === null) return table.rows;
    return sortRows(table.rows, sortColumn, sortDirection);
  });

  /** JSON parse result: either the value, or the parser's complaint. A
   *  malformed .json is a normal thing to attach (that is often why it was
   *  attached), so this reports rather than throws. */
  const json = $derived.by<{ ok: true; value: unknown } | { ok: false; error: string } | null>(
    () => {
      if (mode !== "json" || text === null) return null;
      try {
        return { ok: true, value: JSON.parse(text) };
      } catch (e) {
        return { ok: false, error: e instanceof Error ? e.message : "Invalid JSON" };
      }
    },
  );

  const meta = $derived.by(() => {
    if (mode === "table" && table) {
      const rows = table.totalRows;
      return `${table.columnCount} ${table.columnCount === 1 ? "column" : "columns"}, ${rows} ${rows === 1 ? "row" : "rows"}`;
    }
    if (mode === "json" && json) return json.ok ? "JSON" : "invalid JSON";
    return null;
  });

  function toggleSort(column: number) {
    if (sortColumn === column) {
      sortDirection = sortDirection === "asc" ? "desc" : "asc";
    } else {
      sortColumn = column;
      sortDirection = "asc";
    }
  }
</script>

<div use:whenVisible={{ onVisible: () => void load(), enabled: text === null }}>
  <ViewerFrame {id} {filename} {sizeBytes} {meta} tone="flush">
    {#snippet icon()}
      {#if mode === "table"}<Table size={14} />{:else}<Braces size={14} />{/if}
    {/snippet}

    {#snippet actions()}
      <button
        type="button"
        class="vf__btn vf__btn--label"
        onclick={() => {
          expanded = !expanded;
          if (expanded) void load();
        }}
      >
        {expanded ? "Collapse" : "Expand"}
      </button>
    {/snippet}

    {#if loadError}
      <p class="dt__note">{loadError}</p>
    {:else if text === null}
      <p class="dt__note">{loading ? "Loading preview..." : "Preview not loaded yet."}</p>
    {:else if !expanded}
      <p class="dt__note">
        {#if mode === "table" && table}
          {table.columnCount}
          {table.columnCount === 1 ? "column" : "columns"} and {table.totalRows}
          {table.totalRows === 1 ? "row" : "rows"}. Expand to browse.
        {:else if mode === "json" && json && !json.ok}
          This file is not valid JSON: {json.error}
        {:else}
          Expand to browse the contents.
        {/if}
      </p>
    {:else if mode === "table" && table}
      <div class="dt__scroll">
        <table class="dt__table">
          <thead>
            <tr>
              {#each table.headers as header, i (i)}
                <th scope="col">
                  <button type="button" class="dt__th" onclick={() => toggleSort(i)}>
                    <span>{header || `Column ${i + 1}`}</span>
                    {#if sortColumn === i}
                      {#if sortDirection === "asc"}<ArrowUp size={11} />{:else}<ArrowDown
                          size={11}
                        />{/if}
                    {/if}
                  </button>
                </th>
              {/each}
            </tr>
          </thead>
          <tbody>
            {#each sortedRows as row, i (i)}
              <tr>
                {#each table.headers as _header, c (c)}
                  <td>{row[c] ?? ""}</td>
                {/each}
              </tr>
            {/each}
          </tbody>
        </table>
      </div>
      {#if table.truncated}
        <p class="dt__foot">
          Showing the first {sortedRows.length} of {table.totalRows} rows. Download the file for the
          rest.
        </p>
      {/if}
    {:else if mode === "json" && json}
      {#if json.ok}
        <div class="dt__scroll dt__scroll--tree">
          <JsonNode value={json.value} open={true} />
        </div>
      {:else}
        <p class="dt__note">This file is not valid JSON: {json.error}</p>
      {/if}
    {/if}
  </ViewerFrame>
</div>

<style>
  .dt__note {
    margin: 0;
    padding: 0.5rem 0.625rem;
    color: var(--text-faint);
    font-size: var(--text-body-sm);
  }
  .dt__scroll {
    max-height: 32rem;
    overflow: auto;
    background: var(--bg);
  }
  .dt__scroll--tree {
    padding: 0.375rem 0;
  }
  .dt__table {
    border-collapse: collapse;
    width: max-content;
    min-width: 100%;
    font-size: var(--text-caption);
  }
  .dt__table th {
    position: sticky;
    top: 0;
    z-index: 1;
    padding: 0;
    background: var(--bg-subtle);
    border-bottom: 1px solid var(--border);
    text-align: left;
    white-space: nowrap;
  }
  .dt__th {
    display: flex;
    align-items: center;
    gap: 0.25rem;
    width: 100%;
    padding: 0.3125rem 0.625rem;
    border: 0;
    background: transparent;
    color: var(--text-muted);
    font: inherit;
    font-weight: 600;
    cursor: pointer;
  }
  .dt__th:hover {
    color: var(--text);
  }
  .dt__table td {
    padding: 0.25rem 0.625rem;
    border-bottom: 1px solid var(--border);
    color: var(--text);
    font-family: var(--font-mono);
    white-space: pre;
    max-width: 24rem;
    overflow: hidden;
    text-overflow: ellipsis;
  }
  .dt__table tbody tr:hover td {
    background: var(--bg-subtle);
  }
  .dt__foot {
    margin: 0;
    padding: 0.375rem 0.625rem;
    border-top: 1px solid var(--border);
    background: var(--bg-subtle);
    color: var(--text-faint);
    font-size: var(--text-micro);
  }
</style>

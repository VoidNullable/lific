<script lang="ts">
  // LIF-418: schema peek for an attached SQLite database.
  //
  // Table names and row counts only. Reading rows out of someone's attached
  // database is a different, much larger feature (and a much larger question
  // about who is allowed to run what query), so this stops at "what is in
  // here". Falls back to the download chip whenever /preview cannot answer.

  import { Database } from "lucide-svelte";
  import ViewerFrame from "./ViewerFrame.svelte";
  import FileChip from "./FileChip.svelte";
  import { whenVisible } from "./visible";
  import { getAttachmentPreview, type SqlitePreviewTable } from "../../api";

  let {
    id,
    filename,
    sizeBytes = null,
  }: { id: number; filename: string; sizeBytes?: number | null } = $props();

  let tables = $state<SqlitePreviewTable[] | null>(null);
  let failed = $state(false);
  let requested = false;

  async function load() {
    if (requested) return;
    requested = true;
    const res = await getAttachmentPreview(id);
    if (!res.ok || res.data.kind !== "sqlite") {
      failed = true;
      return;
    }
    tables = res.data.tables;
  }
</script>

{#if failed}
  <FileChip {id} {filename} {sizeBytes} />
{:else}
  <div use:whenVisible={{ onVisible: () => void load(), enabled: tables === null }}>
    {#if tables === null}
      <FileChip {id} {filename} {sizeBytes} note="Reading schema..." />
    {:else}
      <ViewerFrame
        {id}
        {filename}
        {sizeBytes}
        meta={`${tables.length} ${tables.length === 1 ? "table" : "tables"}`}
        tone="flush"
      >
        {#snippet icon()}<Database size={14} />{/snippet}

        {#if tables.length === 0}
          <p class="sv__note">This database has no tables.</p>
        {:else}
          <div class="sv__scroll">
            <table class="sv__table">
              <thead>
                <tr>
                  <th scope="col">Table</th>
                  <th scope="col" class="sv__num-col">Rows</th>
                </tr>
              </thead>
              <tbody>
                {#each tables as t (t.name)}
                  <tr>
                    <td class="sv__name">{t.name}</td>
                    <td class="sv__num-col">{t.rows.toLocaleString()}</td>
                  </tr>
                {/each}
              </tbody>
            </table>
          </div>
        {/if}
      </ViewerFrame>
    {/if}
  </div>
{/if}

<style>
  .sv__note {
    margin: 0;
    padding: 0.5rem 0.625rem;
    color: var(--text-faint);
    font-size: var(--text-body-sm);
  }
  .sv__scroll {
    max-height: 24rem;
    overflow: auto;
    background: var(--bg);
  }
  .sv__table {
    border-collapse: collapse;
    width: 100%;
    font-size: var(--text-caption);
  }
  .sv__table th {
    position: sticky;
    top: 0;
    z-index: 1;
    padding: 0.3125rem 0.625rem;
    background: var(--bg-subtle);
    border-bottom: 1px solid var(--border);
    color: var(--text-muted);
    font-weight: 600;
    text-align: left;
  }
  .sv__table td {
    padding: 0.25rem 0.625rem;
    border-bottom: 1px solid var(--border);
    color: var(--text);
    font-family: var(--font-mono);
  }
  .sv__name {
    word-break: break-all;
  }
  .sv__num-col {
    text-align: right;
    white-space: nowrap;
    font-variant-numeric: tabular-nums;
  }
</style>

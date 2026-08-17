<script lang="ts">
  // LIF-418: inline viewer for .patch / .diff attachments.
  //
  // Same card chrome and same lazy-fetch discipline as TextViewer, with the
  // gutter carrying old/new line numbers instead of one running count and the
  // rows colored by their marker. A patch dropped on an issue is usually there
  // to be read at a glance ("how big is this, what does it touch"), so the
  // summary line and the per-file headers are the point; the raw text stays
  // one copy button away for anyone who wants to `git apply` it.

  import { GitCompare, Copy } from "lucide-svelte";
  import ViewerFrame from "./ViewerFrame.svelte";
  import { whenVisible } from "./visible";
  import { fetchAttachmentText } from "../../api";
  import { copyToClipboard } from "../../clipboard";
  import { parseUnifiedDiff, summarizeDiff, type DiffFile } from "./diff";
  import { MAX_INLINE_BYTES } from "./kind";

  let {
    id,
    filename,
    sizeBytes = null,
  }: { id: number; filename: string; sizeBytes?: number | null } = $props();

  /** Rows rendered before the viewer asks whether you really meant it. A
   *  release-sized patch can be 40k lines and the DOM does not enjoy that. */
  const ROW_BUDGET = 2000;

  let text = $state<string | null>(null);
  let loading = $state(false);
  let loadError = $state<string | null>(null);
  let expanded = $state(false);
  let showAll = $state(false);

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
      loadError = "This patch is too large to preview inline.";
      return;
    }
    text = res.text;
  }

  const parsed = $derived(text === null ? null : parseUnifiedDiff(text));
  const summary = $derived(parsed ? summarizeDiff(parsed) : "");
  const totalRows = $derived(
    parsed ? parsed.files.reduce((n, f) => n + f.lines.length, 0) : 0,
  );
  const overBudget = $derived(!showAll && totalRows > ROW_BUDGET);

  /** Files trimmed to the row budget, so a giant patch still shows its first
   *  screens instead of nothing. */
  const files = $derived.by<DiffFile[]>(() => {
    if (!parsed) return [];
    if (!overBudget) return parsed.files;
    const out: DiffFile[] = [];
    let budget = ROW_BUDGET;
    for (const file of parsed.files) {
      if (budget <= 0) break;
      out.push(file.lines.length <= budget ? file : { ...file, lines: file.lines.slice(0, budget) });
      budget -= file.lines.length;
    }
    return out;
  });

  function fileStat(file: DiffFile): string {
    if (file.binary) return "binary";
    return `+${file.additions} -${file.deletions}`;
  }
</script>

<div use:whenVisible={{ onVisible: () => void load(), enabled: text === null }}>
  <ViewerFrame {id} {filename} {sizeBytes} meta={parsed ? summary : null} tone="flush">
    {#snippet icon()}<GitCompare size={14} />{/snippet}

    {#snippet actions()}
      {#if text !== null}
        <button
          type="button"
          class="vf__btn"
          title="Copy raw diff"
          aria-label="Copy raw diff"
          onclick={() => void copyToClipboard(text ?? "", { label: filename })}
        >
          <Copy size={13} />
        </button>
      {/if}
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
      <p class="dv__note">{loadError}</p>
    {:else if text === null}
      <p class="dv__note">{loading ? "Loading patch..." : "Preview not loaded yet."}</p>
    {:else if !expanded}
      <div class="dv__files">
        {#each parsed?.files ?? [] as file, i (i)}
          <div class="dv__file-line">
            <span class="dv__file-name">{file.display}</span>
            <span class="dv__file-stat">{fileStat(file)}</span>
          </div>
        {/each}
      </div>
    {:else}
      <div class="dv__scroll">
        {#each files as file, i (i)}
          <div class="dv__file">
            <header class="dv__file-head">
              <span class="dv__file-name">{file.display}</span>
              <span class="dv__file-stat">{fileStat(file)}</span>
            </header>
            <div class="dv__code">
              {#each file.lines as line, j (j)}
                <div class="dv__row dv__row--{line.kind}">
                  <span class="dv__num">{line.oldNo ?? ""}</span>
                  <span class="dv__num">{line.newNo ?? ""}</span>
                  <span class="dv__marker"
                    >{line.kind === "add" ? "+" : line.kind === "del" ? "-" : " "}</span
                  >
                  <span class="dv__text">{line.text}</span>
                </div>
              {/each}
            </div>
          </div>
        {/each}
        {#if overBudget}
          <div class="dv__gap">
            <span>Showing the first {ROW_BUDGET} of {totalRows} lines.</span>
            <button type="button" class="dv__more" onclick={() => (showAll = true)}>
              Show the whole patch
            </button>
          </div>
        {/if}
      </div>
    {/if}
  </ViewerFrame>
</div>

<style>
  .dv__note {
    margin: 0;
    padding: 0.5rem 0.625rem;
    color: var(--text-faint);
    font-size: var(--text-body-sm);
  }
  .dv__files {
    padding: 0.25rem 0;
  }
  .dv__file-line,
  .dv__file-head {
    display: flex;
    align-items: center;
    gap: 0.75rem;
    padding: 0.25rem 0.625rem;
    font-family: var(--font-mono);
    font-size: var(--text-caption);
  }
  .dv__file-head {
    position: sticky;
    top: 0;
    z-index: 1;
    border-bottom: 1px solid var(--border);
    background: var(--bg-subtle);
  }
  .dv__file + .dv__file .dv__file-head {
    border-top: 1px solid var(--border);
  }
  .dv__file-name {
    color: var(--text);
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
    min-width: 0;
  }
  .dv__file-stat {
    margin-left: auto;
    color: var(--text-faint);
    font-size: var(--text-micro);
    font-variant-numeric: tabular-nums;
    flex-shrink: 0;
  }

  .dv__scroll {
    max-height: 32rem;
    overflow: auto;
    background: var(--bg);
  }
  .dv__code {
    font-family: var(--font-mono);
    font-size: var(--text-caption);
    line-height: 1.5;
  }
  .dv__row {
    display: flex;
    align-items: flex-start;
    white-space: pre;
  }
  .dv__num {
    flex: 0 0 auto;
    width: 3rem;
    padding-right: 0.5rem;
    color: var(--text-faint);
    text-align: right;
    font-variant-numeric: tabular-nums;
    user-select: none;
  }
  .dv__marker {
    flex: 0 0 auto;
    width: 1rem;
    text-align: center;
    color: var(--text-faint);
    border-left: 1px solid var(--border);
    user-select: none;
  }
  .dv__text {
    flex: 1 1 auto;
    padding: 0 0.625rem 0 0.25rem;
    color: var(--text);
    min-width: 0;
  }
  .dv__row--add {
    background: color-mix(in srgb, var(--success) 14%, transparent);
  }
  .dv__row--del {
    background: color-mix(in srgb, var(--error) 14%, transparent);
  }
  .dv__row--hunk {
    background: var(--bg-subtle);
    color: var(--text-muted);
  }
  .dv__row--hunk .dv__text,
  .dv__row--meta .dv__text {
    color: var(--text-faint);
  }
  .dv__row--meta {
    opacity: 0.85;
  }

  .dv__gap {
    display: flex;
    align-items: center;
    gap: 0.75rem;
    padding: 0.375rem 0.625rem;
    border-top: 1px dashed var(--border);
    background: var(--bg-subtle);
    color: var(--text-faint);
    font-size: var(--text-micro);
  }
  .dv__more {
    border: 0;
    background: transparent;
    color: var(--accent);
    font-size: var(--text-micro);
    cursor: pointer;
    padding: 0;
  }
</style>

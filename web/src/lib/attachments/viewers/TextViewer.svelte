<script lang="ts">
  // LIF-418: inline viewer for text attachments.
  //
  // Replaces the bare download chip for anything text-shaped. Collapsed it is
  // a card with the first 15 lines, which is enough to recognize a log without
  // leaving the issue; expanded it is a proper reader with line numbers,
  // find-in-file, ANSI colors, and line deep links.
  //
  // Three things keep it cheap on a page full of attachments:
  //   - bytes are fetched only once the card scrolls into view,
  //   - files over 5k lines render head + tail until asked for the rest,
  //   - files over MAX_INLINE_BYTES are never fetched at all (the dispatcher
  //     hands those to the plain chip).

  import { tick } from "svelte";
  import {
    FileText,
    Search,
    ChevronUp,
    ChevronDown,
    Link as LinkIcon,
    Copy,
    X,
  } from "lucide-svelte";
  import ViewerFrame from "./ViewerFrame.svelte";
  import { whenVisible } from "./visible";
  import { fetchAttachmentText } from "../../api";
  import { copyToClipboard } from "../../clipboard";
  import { ansiToSpans, ansiStyleToCss, hasAnsi, stripAnsi, type AnsiSpan } from "./ansi";
  import { MAX_INLINE_BYTES } from "./kind";
  import {
    formatLineAnchor,
    fullLineLink,
    hashWithLineTarget,
    lineTargetFromHash,
  } from "./deepLink";

  let {
    id,
    filename,
    sizeBytes = null,
    /** Rendered above the code, e.g. the diff summary line. */
    summary = null,
  }: {
    id: number;
    filename: string;
    sizeBytes?: number | null;
    summary?: string | null;
  } = $props();

  const PREVIEW_LINES = 15;
  const HEAD_TAIL_THRESHOLD = 5000;
  const HEAD_TAIL_COUNT = 200;

  let text = $state<string | null>(null);
  let loading = $state(false);
  let loadError = $state<string | null>(null);
  let expanded = $state(false);
  let showAll = $state(false);

  // Find-in-file.
  let findOpen = $state(false);
  let query = $state("");
  let currentMatch = $state(0);
  let findInput = $state<HTMLInputElement | null>(null);

  // Line selection / deep link.
  let selStart = $state<number | null>(null);
  let selEnd = $state<number | null>(null);

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

  const colored = $derived(text !== null && hasAnsi(text));

  /** Per-line ANSI spans, or null when the file has no escapes at all (the
   *  common case, and worth not allocating for). */
  const spanLines = $derived.by<AnsiSpan[][] | null>(() =>
    text !== null && colored ? ansiToSpans(trimTrailingNewline(text)) : null,
  );

  const lines = $derived.by<string[]>(() => {
    if (text === null) return [];
    const body = trimTrailingNewline(text);
    const raw = body.split("\n");
    return colored ? raw.map(stripAnsi) : raw;
  });

  function trimTrailingNewline(value: string): string {
    return value.endsWith("\n") ? value.slice(0, -1) : value;
  }

  const total = $derived(lines.length);
  const previewLines = $derived(lines.slice(0, PREVIEW_LINES));
  const truncating = $derived(!showAll && total > HEAD_TAIL_THRESHOLD);
  const headTo = $derived(truncating ? HEAD_TAIL_COUNT : total);
  const tailFrom = $derived(truncating ? total - HEAD_TAIL_COUNT + 1 : total + 1);

  interface Match {
    line: number;
    start: number;
    end: number;
    index: number;
  }

  const matches = $derived.by<Match[]>(() => {
    const needle = query.trim();
    if (!needle || total === 0) return [];
    const lower = needle.toLowerCase();
    const found: Match[] = [];
    for (let i = 0; i < lines.length; i++) {
      const haystack = lines[i].toLowerCase();
      let from = 0;
      for (;;) {
        const at = haystack.indexOf(lower, from);
        if (at < 0) break;
        found.push({ line: i + 1, start: at, end: at + lower.length, index: found.length });
        from = at + lower.length;
        // A 200k-match search is a runaway, not a search.
        if (found.length >= 5000) return found;
      }
    }
    return found;
  });

  const matchesByLine = $derived.by(() => {
    const map = new Map<number, Match[]>();
    for (const m of matches) {
      const list = map.get(m.line);
      if (list) list.push(m);
      else map.set(m.line, [m]);
    }
    return map;
  });

  interface Segment {
    text: string;
    css: string;
    match: number | null;
  }

  function segmentsFor(lineNo: number): Segment[] {
    const idx = lineNo - 1;
    const spans: AnsiSpan[] = spanLines
      ? (spanLines[idx] ?? [{ text: "", style: {} }])
      : [{ text: lines[idx] ?? "", style: {} }];
    const ranges = matchesByLine.get(lineNo);
    const out: Segment[] = [];
    let offset = 0;
    for (const span of spans) {
      const css = ansiStyleToCss(span.style);
      const spanStart = offset;
      const spanEnd = offset + span.text.length;
      offset = spanEnd;
      if (!ranges || ranges.length === 0) {
        out.push({ text: span.text, css, match: null });
        continue;
      }
      let pos = 0;
      for (const range of ranges) {
        if (range.end <= spanStart || range.start >= spanEnd) continue;
        const from = Math.max(range.start, spanStart) - spanStart;
        const to = Math.min(range.end, spanEnd) - spanStart;
        if (from > pos) out.push({ text: span.text.slice(pos, from), css, match: null });
        out.push({ text: span.text.slice(from, to), css, match: range.index });
        pos = to;
      }
      if (pos < span.text.length) {
        out.push({ text: span.text.slice(pos), css, match: null });
      }
    }
    return out;
  }

  function rangeOf(from: number, to: number): number[] {
    const out: number[] = [];
    for (let n = from; n <= to; n++) out.push(n);
    return out;
  }

  /** The rows to render, with a single "gap" marker standing in for the
   *  elided middle of a head/tail view. One list keeps the template to one
   *  loop instead of duplicating the row markup per section. */
  const rows = $derived.by<(number | "gap")[]>(() => {
    const out: (number | "gap")[] = rangeOf(1, Math.min(headTo, total));
    if (truncating) {
      out.push("gap");
      out.push(...rangeOf(tailFrom, total));
    }
    return out;
  });

  function inSelection(lineNo: number): boolean {
    if (selStart === null) return false;
    const lo = Math.min(selStart, selEnd ?? selStart);
    const hi = Math.max(selStart, selEnd ?? selStart);
    return lineNo >= lo && lineNo <= hi;
  }

  const selectionToken = $derived(
    selStart === null ? null : formatLineAnchor(id, selStart, selEnd ?? selStart),
  );

  function lineDomId(lineNo: number): string {
    return `att${id}-L${lineNo}`;
  }

  function selectLine(lineNo: number, extend: boolean) {
    if (extend && selStart !== null) selEnd = lineNo;
    else {
      selStart = lineNo;
      selEnd = lineNo;
    }
    writeHash();
  }

  /** Push the current selection into the URL without clobbering the hash
   *  route (see deepLink.ts for why the two shapes exist). */
  function writeHash() {
    if (typeof window === "undefined") return;
    const next = hashWithLineTarget(window.location.hash, selectionToken);
    const url = `${window.location.pathname}${window.location.search}${next}`;
    history.replaceState(null, "", url);
  }

  async function copyLink() {
    if (!selectionToken) return;
    await copyToClipboard(fullLineLink(selectionToken, window.location), {
      label: "link to lines",
    });
  }

  async function copyAll() {
    if (text === null) return;
    await copyToClipboard(colored ? stripAnsi(text) : text, {
      label: filename,
      silentSuccess: false,
    });
  }

  async function expand() {
    expanded = true;
    await load();
  }

  async function openFind() {
    findOpen = true;
    // Searching a head/tail view would silently skip matches in the middle.
    showAll = true;
    await tick();
    findInput?.focus();
  }

  function closeFind() {
    findOpen = false;
    query = "";
  }

  async function gotoMatch(delta: number) {
    if (matches.length === 0) return;
    currentMatch = (currentMatch + delta + matches.length) % matches.length;
    await scrollToLine(matches[currentMatch].line);
  }

  async function scrollToLine(lineNo: number) {
    await tick();
    const el = document.getElementById(lineDomId(lineNo));
    el?.scrollIntoView({ block: "center", behavior: "auto" });
  }

  // Reset the match cursor whenever the result set changes under it.
  $effect(() => {
    matches.length;
    currentMatch = 0;
  });

  // ── Deep links ────────────────────────────────────────────
  //
  // A URL carrying `att{id}-L…` for THIS attachment expands the card, forces
  // the full render (the target may sit in the elided middle), selects the
  // range, and scrolls to it. Runs at mount and on every later hash change so
  // pasting a second link into the address bar works too.

  async function applyHashTarget() {
    if (typeof window === "undefined") return;
    const target = lineTargetFromHash(window.location.hash);
    if (!target || target.attachmentId !== id) return;
    expanded = true;
    showAll = true;
    await load();
    selStart = target.start;
    selEnd = target.end;
    await scrollToLine(target.start);
  }

  $effect(() => {
    void applyHashTarget();
    const onHash = () => void applyHashTarget();
    window.addEventListener("hashchange", onHash);
    return () => window.removeEventListener("hashchange", onHash);
  });
</script>

<div
  use:whenVisible={{ onVisible: () => void load(), enabled: text === null }}
>
  <ViewerFrame
    {id}
    {filename}
    {sizeBytes}
    meta={text === null ? null : `${total} ${total === 1 ? "line" : "lines"}`}
    tone="flush"
  >
    {#snippet icon()}<FileText size={14} />{/snippet}

    {#snippet actions()}
      {#if expanded && text !== null}
        <button
          type="button"
          class="vf__btn"
          aria-pressed={findOpen}
          title="Find in file"
          onclick={() => (findOpen ? closeFind() : void openFind())}
        >
          <Search size={13} />
        </button>
        <button type="button" class="vf__btn" title="Copy file contents" onclick={copyAll}>
          <Copy size={13} />
        </button>
      {/if}
      <button
        type="button"
        class="vf__btn vf__btn--label"
        onclick={() => (expanded ? (expanded = false) : void expand())}
      >
        {expanded ? "Collapse" : "Expand"}
      </button>
    {/snippet}

    {#if loadError}
      <p class="tv__note">{loadError}</p>
    {:else if text === null}
      <p class="tv__note">{loading ? "Loading preview..." : "Preview not loaded yet."}</p>
    {:else if !expanded}
      <div class="tv__preview">
        <pre class="tv__pre">{previewLines.join("\n")}</pre>
        {#if total > PREVIEW_LINES}
          <button type="button" class="tv__more" onclick={() => void expand()}>
            Expand to see all {total} lines
          </button>
        {/if}
      </div>
    {:else}
      {#if summary}
        <p class="tv__summary">{summary}</p>
      {/if}
      <div class="tv__scroll">
        {#if findOpen}
          <div class="tv__find">
            <Search size={13} />
            <input
              bind:this={findInput}
              bind:value={query}
              class="tv__find-input"
              type="search"
              placeholder="Find in file"
              aria-label="Find in file"
              onkeydown={(e) => {
                if (e.key === "Enter") {
                  e.preventDefault();
                  void gotoMatch(e.shiftKey ? -1 : 1);
                } else if (e.key === "Escape") {
                  e.preventDefault();
                  closeFind();
                }
              }}
            />
            <span class="tv__find-count">
              {matches.length === 0
                ? query.trim()
                  ? "No results"
                  : ""
                : `${currentMatch + 1} of ${matches.length}`}
            </span>
            <button
              type="button"
              class="tv__find-btn"
              title="Previous match"
              aria-label="Previous match"
              onclick={() => void gotoMatch(-1)}
            >
              <ChevronUp size={13} />
            </button>
            <button
              type="button"
              class="tv__find-btn"
              title="Next match"
              aria-label="Next match"
              onclick={() => void gotoMatch(1)}
            >
              <ChevronDown size={13} />
            </button>
            {#if selectionToken}
              <button
                type="button"
                class="tv__find-btn"
                title="Copy link to selected lines"
                aria-label="Copy link to selected lines"
                onclick={copyLink}
              >
                <LinkIcon size={13} />
              </button>
            {/if}
            <button
              type="button"
              class="tv__find-btn"
              title="Close find"
              aria-label="Close find"
              onclick={closeFind}
            >
              <X size={13} />
            </button>
          </div>
        {/if}

        <div class="tv__code">
          {#each rows as row (row)}
            {#if row === "gap"}
              <div class="tv__gap">
                <span>
                  Showing the first {HEAD_TAIL_COUNT} and last {HEAD_TAIL_COUNT} lines of {total}.
                </span>
                <button type="button" class="tv__more" onclick={() => (showAll = true)}>
                  Expand all
                </button>
              </div>
            {:else}
              <div class="tv__row" class:tv__row--sel={inSelection(row)} id={lineDomId(row)}>
                <button
                  type="button"
                  class="tv__num"
                  title="Select line {row} (shift-click for a range)"
                  onclick={(e) => selectLine(row, e.shiftKey)}
                >{row}</button>
                <span class="tv__text"
                  >{#each segmentsFor(row) as seg, i (i)}{#if seg.match === null}<span
                        style={seg.css}>{seg.text}</span
                      >{:else}<mark
                        class="tv__hit"
                        class:tv__hit--current={seg.match === currentMatch}
                        style={seg.css}>{seg.text}</mark
                      >{/if}{/each}</span
                >
              </div>
            {/if}
          {/each}
        </div>
      </div>

      {#if selectionToken && !findOpen}
        <div class="tv__foot">
          <span class="tv__foot-label">
            {selEnd !== null && selEnd !== selStart
              ? `Lines ${Math.min(selStart ?? 0, selEnd)} to ${Math.max(selStart ?? 0, selEnd)} selected`
              : `Line ${selStart} selected`}
          </span>
          <button type="button" class="tv__foot-btn" onclick={copyLink}>
            <LinkIcon size={12} /> Copy link
          </button>
          <button
            type="button"
            class="tv__foot-btn"
            onclick={() => {
              selStart = null;
              selEnd = null;
              writeHash();
            }}
          >
            Clear
          </button>
        </div>
      {/if}
    {/if}
  </ViewerFrame>
</div>

<style>
  /* ANSI palette. Terminal colors were chosen against a black background, so
     the light theme needs darker variants to stay legible on a pale card. */
  :global(:root) {
    --ansi-black: #3f3f3f;
    --ansi-red: #b02c2c;
    --ansi-green: #2f7d31;
    --ansi-yellow: #8a6d1a;
    --ansi-blue: #2b5fb0;
    --ansi-magenta: #8b3fa8;
    --ansi-cyan: #1f7a80;
    --ansi-white: #8a8a8a;
    --ansi-bright-black: #6b6b6b;
    --ansi-bright-red: #d13b3b;
    --ansi-bright-green: #35973a;
    --ansi-bright-yellow: #a07f1e;
    --ansi-bright-blue: #3a74cc;
    --ansi-bright-magenta: #a34ec2;
    --ansi-bright-cyan: #23929a;
    --ansi-bright-white: #2b2b2b;
    --find-hit: #fde68a;
  }
  :global(.dark) {
    --ansi-black: #6b6b6b;
    --ansi-red: #e06c75;
    --ansi-green: #8bc46a;
    --ansi-yellow: #e0c165;
    --ansi-blue: #74a4f0;
    --ansi-magenta: #c98ce0;
    --ansi-cyan: #56c2c8;
    --ansi-white: #c8c8c8;
    --ansi-bright-black: #8a8a8a;
    --ansi-bright-red: #ff8a92;
    --ansi-bright-green: #a3dd82;
    --ansi-bright-yellow: #f2d67e;
    --ansi-bright-blue: #93bcff;
    --ansi-bright-magenta: #dda6f0;
    --ansi-bright-cyan: #79dde3;
    --ansi-bright-white: #f0f0f0;
    --find-hit: #6b5a1f;
  }

  .tv__note {
    margin: 0;
    padding: 0.5rem 0.625rem;
    color: var(--text-faint);
    font-size: var(--text-body-sm);
  }
  .tv__preview {
    padding: 0.375rem 0 0;
  }
  .tv__pre {
    margin: 0;
    padding: 0.25rem 0.625rem 0.5rem;
    max-height: 16rem;
    overflow: hidden;
    font-family: var(--font-mono);
    font-size: var(--text-caption);
    line-height: 1.5;
    color: var(--text-muted);
    white-space: pre;
    overflow-x: auto;
  }
  .tv__summary {
    margin: 0;
    padding: 0.375rem 0.625rem;
    border-bottom: 1px solid var(--border);
    color: var(--text-muted);
    font-size: var(--text-micro);
    font-variant-numeric: tabular-nums;
  }
  .tv__more {
    display: block;
    width: 100%;
    padding: 0.375rem 0.625rem;
    border: 0;
    border-top: 1px solid var(--border);
    background: transparent;
    color: var(--accent);
    font-size: var(--text-micro);
    text-align: left;
    cursor: pointer;
  }
  .tv__more:hover {
    background: var(--bg-subtle);
  }

  .tv__scroll {
    position: relative;
    max-height: 32rem;
    overflow: auto;
    background: var(--bg);
  }
  .tv__find {
    position: sticky;
    top: 0;
    z-index: 2;
    display: flex;
    align-items: center;
    gap: 0.25rem;
    padding: 0.3125rem 0.5rem;
    border-bottom: 1px solid var(--border);
    background: var(--surface);
    color: var(--text-muted);
  }
  .tv__find-input {
    flex: 1 1 auto;
    min-width: 4rem;
    padding: 0.1875rem 0.375rem;
    border: 1px solid var(--border);
    border-radius: 0.3125rem;
    background: var(--bg);
    color: var(--text);
    font-size: var(--text-body-sm);
  }
  .tv__find-input:focus {
    outline: none;
    border-color: var(--accent);
  }
  .tv__find-count {
    color: var(--text-faint);
    font-size: var(--text-micro);
    font-variant-numeric: tabular-nums;
    white-space: nowrap;
  }
  .tv__find-btn {
    display: inline-flex;
    align-items: center;
    padding: 0.1875rem;
    border: 0;
    border-radius: 0.25rem;
    background: transparent;
    color: var(--text-muted);
    cursor: pointer;
  }
  .tv__find-btn:hover {
    color: var(--text);
    background: var(--bg-subtle);
  }

  .tv__code {
    font-family: var(--font-mono);
    font-size: var(--text-caption);
    line-height: 1.5;
  }
  .tv__row {
    display: flex;
    align-items: flex-start;
    scroll-margin-block: 3rem;
  }
  .tv__row--sel {
    background: var(--accent-subtle);
  }
  .tv__num {
    flex: 0 0 auto;
    width: 3.5rem;
    padding: 0 0.5rem 0 0;
    border: 0;
    border-right: 1px solid var(--border);
    background: transparent;
    color: var(--text-faint);
    font: inherit;
    font-variant-numeric: tabular-nums;
    text-align: right;
    cursor: pointer;
    user-select: none;
  }
  .tv__num:hover {
    color: var(--accent);
    background: var(--bg-subtle);
  }
  .tv__text {
    flex: 1 1 auto;
    padding: 0 0.625rem;
    white-space: pre;
    color: var(--text);
    min-width: 0;
  }
  .tv__hit {
    background: var(--find-hit);
    color: var(--text);
    border-radius: 0.125rem;
  }
  .tv__hit--current {
    background: var(--accent);
    color: var(--accent-text);
  }

  .tv__gap {
    display: flex;
    align-items: center;
    gap: 0.75rem;
    padding: 0.375rem 0.625rem;
    border-block: 1px dashed var(--border);
    background: var(--bg-subtle);
    color: var(--text-faint);
    font-size: var(--text-micro);
  }
  .tv__gap .tv__more {
    width: auto;
    padding: 0;
    border: 0;
  }

  .tv__foot {
    display: flex;
    align-items: center;
    gap: 0.5rem;
    padding: 0.3125rem 0.625rem;
    border-top: 1px solid var(--border);
    background: var(--bg-subtle);
    font-size: var(--text-micro);
    color: var(--text-muted);
  }
  .tv__foot-btn {
    display: inline-flex;
    align-items: center;
    gap: 0.25rem;
    padding: 0.125rem 0.3125rem;
    border: 1px solid var(--border);
    border-radius: 0.25rem;
    background: var(--surface);
    color: var(--text-muted);
    font-size: var(--text-micro);
    cursor: pointer;
  }
  .tv__foot-btn:hover {
    color: var(--accent);
    border-color: var(--accent);
  }
</style>

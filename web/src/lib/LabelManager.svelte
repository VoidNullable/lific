<script lang="ts">
  // Full label management surface for the project overview (label management).
  // Create, rename, recolor, merge, and delete project labels in one place —
  // the canonical home for the vocabulary issues/pages attach. Inline create
  // in the issue picker (LabelEditor) covers the quick path; this covers the
  // deliberate one.
  //
  // Per-row action split keeps each control obvious:
  //   • color dot  → recolor inline via ColorPicker (instant)        (#2/#8)
  //   • name       → click to rename inline                          (#7)
  //   • usage count→ link to the filtered issue list                 (#1)
  //   • trash      → delete, with an optional "merge into…" path      (#4)
  import {
    listLabels,
    createLabel,
    updateLabel,
    deleteLabel,
    mergeLabel,
    listPages,
    type Label,
    type Issue,
    type Page,
  } from "./api";
  import { Tag, Trash2, Plus, ArrowRight, Search } from "lucide-svelte";
  import ColorPicker from "./ColorPicker.svelte";
  import { colorForName, DEFAULT_LABEL_COLOR } from "./labelColors";

  let {
    projectId,
    issues = [],
    onChange,
    onOpenLabel,
  }: {
    projectId: number;
    /** Project issues, used for per-label usage counts. */
    issues?: Issue[];
    /** Fired after any create/update/delete/merge so the parent can refresh. */
    onChange?: () => void;
    /** Navigate to the issue list filtered by this label (#1). */
    onOpenLabel?: (name: string) => void;
  } = $props();

  let labels = $state<Label[]>([]);
  let pages = $state<Page[]>([]);
  let loading = $state(true);
  let err = $state("");

  // ── Create form (#6: live preview + locked-once-picked color) ──
  let newName = $state("");
  let newColor = $state(DEFAULT_LABEL_COLOR);
  let colorTouched = $state(false);
  let creating = $state(false);
  // Until the user picks a color, track a stable hue derived from the name.
  let createColor = $derived(
    colorTouched ? newColor : colorForName(newName.trim() || "label"),
  );

  // ── Rename (inline) ──
  let editingId = $state<number | null>(null);
  let editName = $state("");

  // ── Delete / merge ──
  let confirmingId = $state<number | null>(null);
  let mergeInto = $state<number | null>(null);
  let busyId = $state<number | null>(null);

  // ── Sort + filter (#3) ──
  let sortBy = $state<"name" | "usage" | "newest">("name");
  let filter = $state("");

  // ── Starter presets (#5) ──
  const PRESETS: { name: string; color: string }[] = [
    { name: "bug", color: "#EF4444" },
    { name: "feature", color: "#16A34A" },
    { name: "docs", color: "#2563EB" },
    { name: "chore", color: "#6B7280" },
    { name: "blocked", color: "#D97706" },
    { name: "design", color: "#7C3AED" },
  ];

  $effect(() => {
    projectId;
    init();
  });

  async function init() {
    loading = true;
    err = "";
    const [lr, pr] = await Promise.all([listLabels(projectId), listPages(projectId)]);
    if (lr.ok) labels = lr.data;
    else err = lr.error;
    if (pr.ok) pages = pr.data;
    loading = false;
  }
  async function reloadPages() {
    const pr = await listPages(projectId);
    if (pr.ok) pages = pr.data;
  }

  // Usage across both issues and pages (labels attach to both — LIF-105).
  let usage = $derived.by(() => {
    const m = new Map<string, { issues: number; pages: number }>();
    const bump = (n: string, k: "issues" | "pages") => {
      const e = m.get(n) ?? { issues: 0, pages: 0 };
      e[k]++;
      m.set(n, e);
    };
    for (const i of issues) for (const n of i.labels) bump(n, "issues");
    for (const p of pages) for (const n of p.labels) bump(n, "pages");
    return m;
  });
  function usageTotal(name: string): number {
    const e = usage.get(name);
    return e ? e.issues + e.pages : 0;
  }
  function usageText(name: string): string {
    const e = usage.get(name);
    if (!e) return "";
    const parts: string[] = [];
    if (e.issues) parts.push(`${e.issues} issue${e.issues === 1 ? "" : "s"}`);
    if (e.pages) parts.push(`${e.pages} page${e.pages === 1 ? "" : "s"}`);
    return parts.join(" · ");
  }

  let showToolbar = $derived(labels.length > 1);
  let showFilter = $derived(labels.length > 8);

  let visible = $derived.by(() => {
    const q = filter.trim().toLowerCase();
    let arr = q ? labels.filter((l) => l.name.toLowerCase().includes(q)) : [...labels];
    if (sortBy === "name") arr.sort((a, b) => a.name.localeCompare(b.name));
    else if (sortBy === "usage")
      arr.sort((a, b) => usageTotal(b.name) - usageTotal(a.name) || a.name.localeCompare(b.name));
    else if (sortBy === "newest") arr.sort((a, b) => b.id - a.id);
    return arr;
  });

  function nameTaken(name: string, exceptId?: number): boolean {
    const lc = name.trim().toLowerCase();
    return labels.some((l) => l.name.toLowerCase() === lc && l.id !== exceptId);
  }
  // Live duplicate hint for the create form (#6).
  let createDup = $derived(newName.trim().length > 0 && nameTaken(newName));

  async function create(name = newName, color = createColor) {
    const n = name.trim();
    if (!n || creating || nameTaken(n)) return;
    creating = true;
    err = "";
    const r = await createLabel({ project_id: projectId, name: n, color });
    creating = false;
    if (r.ok) {
      labels = [...labels, r.data];
      if (name === newName) {
        newName = "";
        newColor = DEFAULT_LABEL_COLOR;
        colorTouched = false;
      }
      onChange?.();
    } else {
      err = r.error;
    }
  }

  async function addAllPresets() {
    for (const p of PRESETS) {
      if (!nameTaken(p.name)) await create(p.name, p.color);
    }
  }

  // Recolor instantly from the display-row dot (no edit mode needed).
  async function recolor(l: Label, color: string) {
    if (color === l.color) return;
    const prev = l.color;
    labels = labels.map((x) => (x.id === l.id ? { ...x, color } : x));
    const r = await updateLabel(l.id, { color });
    if (r.ok) onChange?.();
    else {
      err = r.error;
      labels = labels.map((x) => (x.id === l.id ? { ...x, color: prev } : x));
    }
  }

  function startRename(l: Label) {
    editingId = l.id;
    editName = l.name;
    confirmingId = null;
  }
  async function commitRename(l: Label) {
    const name = editName.trim();
    editingId = null;
    if (!name || name === l.name) return;
    if (nameTaken(name, l.id)) {
      err = `A label named "${name}" already exists.`;
      return;
    }
    err = "";
    const r = await updateLabel(l.id, { name });
    if (r.ok) {
      labels = labels.map((x) => (x.id === l.id ? r.data : x));
      onChange?.();
    } else {
      err = r.error;
    }
  }

  function startConfirm(l: Label) {
    confirmingId = l.id;
    mergeInto = null;
    editingId = null;
  }
  async function del(id: number) {
    busyId = id;
    err = "";
    const r = await deleteLabel(id);
    busyId = null;
    if (r.ok) {
      labels = labels.filter((l) => l.id !== id);
      confirmingId = null;
      await reloadPages();
      onChange?.();
    } else {
      err = r.error;
    }
  }
  async function doMerge(sourceId: number) {
    if (mergeInto == null) return;
    busyId = sourceId;
    err = "";
    const r = await mergeLabel(sourceId, mergeInto);
    busyId = null;
    if (r.ok) {
      labels = labels.filter((l) => l.id !== sourceId);
      confirmingId = null;
      mergeInto = null;
      await reloadPages();
      onChange?.();
    } else {
      err = r.error;
    }
  }
</script>

<section>
  <div class="flex items-center justify-between gap-3 mb-3 flex-wrap">
    <h2 class="text-body-sm font-semibold text-[var(--text)] flex items-center gap-1.5">
      <Tag size={14} class="text-[var(--text-muted)]" />
      Labels
      {#if labels.length > 0}
        <span class="text-caption font-normal text-[var(--text-faint)] tabular-nums">{labels.length}</span>
      {/if}
    </h2>

    {#if showToolbar}
      <div class="flex items-center gap-2">
        {#if showFilter}
          <div class="relative">
            <Search size={12} class="absolute left-2 top-1/2 -translate-y-1/2 text-[var(--text-faint)]" />
            <input
              bind:value={filter}
              placeholder="Filter…"
              class="w-[130px] pr-2 py-1 text-caption rounded-md border border-[var(--border)]
                     bg-[var(--surface)] text-[var(--text)] placeholder:text-[var(--text-faint)]
                     outline-none focus:border-[var(--accent)]"
              style="padding-left: 1.5rem"
            />
          </div>
        {/if}
        <div class="flex items-center gap-0.5 text-caption">
          <span class="text-[var(--text-faint)]">Sort</span>
          {#each [["name", "A–Z"], ["usage", "Most used"], ["newest", "Newest"]] as [val, label] (val)}
            <button
              class="px-1.5 py-0.5 rounded transition-colors
                     {sortBy === val
                ? 'bg-[var(--bg-subtle)] text-[var(--text)] font-medium'
                : 'text-[var(--text-muted)] hover:text-[var(--text)] hover:bg-[var(--bg-subtle)]'}"
              onclick={() => (sortBy = val as typeof sortBy)}
            >
              {label}
            </button>
          {/each}
        </div>
      </div>
    {/if}
  </div>

  <div class="rounded-xl bg-[var(--surface)] shadow-[0_1px_2px_rgba(0,0,0,0.06)] overflow-hidden">
    <!-- Create row with live preview (#6) -->
    <div class="flex items-center gap-3 px-4 py-3 border-b border-[var(--border)] flex-wrap">
      <ColorPicker value={createColor} onChange={(c) => { newColor = c; colorTouched = true; }} />
      <input
        bind:value={newName}
        placeholder="New label name…"
        maxlength="40"
        class="flex-1 min-w-[120px] text-body-sm bg-transparent text-[var(--text)]
               placeholder:text-[var(--text-faint)] outline-none"
        onkeydown={(e) => { if (e.key === 'Enter') create(); }}
      />
      <!-- Live chip preview -->
      {#if newName.trim()}
        <span
          class="inline-flex items-center gap-1.5 text-caption font-medium px-2 py-0.5 rounded-full border shrink-0 max-w-[160px]"
          style="color: {createColor}; border-color: {createColor}40; background: {createColor}10;"
        >
          <span class="size-2 rounded-full shrink-0" style="background: {createColor}"></span>
          <span class="truncate">{newName.trim()}</span>
        </span>
      {/if}
      <button
        class="flex items-center gap-1.5 text-body-sm font-medium text-[var(--btn-success-text)]
               bg-[var(--btn-success)] px-3 py-1.5 rounded-md hover:bg-[var(--btn-success-hover)]
               transition-colors disabled:opacity-40 disabled:cursor-not-allowed"
        disabled={creating || !newName.trim() || createDup}
        onclick={() => create()}
      >
        <Plus size={14} />
        {creating ? "Adding…" : "Add"}
      </button>
    </div>

    {#if createDup}
      <div class="px-4 py-1.5 text-caption text-[var(--warn)] bg-[color-mix(in_oklab,var(--warn)_10%,transparent)]">
        “{newName.trim()}” already exists.
      </div>
    {/if}
    {#if err}
      <div class="px-4 py-2 text-caption text-[var(--error)] bg-[var(--error-bg)]">{err}</div>
    {/if}

    <!-- List -->
    {#if loading}
      <div class="px-4 py-6 flex justify-center">
        <div class="size-5 rounded-full border-2 border-[var(--border)] border-t-[var(--accent)] animate-spin"></div>
      </div>
    {:else if labels.length === 0}
      <!-- Empty-state starter presets (#5) -->
      <div class="px-4 py-5 flex flex-col gap-3">
        <p class="text-body-sm text-[var(--text-muted)]">
          No labels yet. Start from a common set, or create your own above.
        </p>
        <div class="flex items-center gap-2 flex-wrap">
          {#each PRESETS as p (p.name)}
            <button
              class="inline-flex items-center gap-1.5 text-caption font-medium px-2 py-1 rounded-full border
                     hover:brightness-110 transition-all"
              style="color: {p.color}; border-color: {p.color}55; background: {p.color}12;"
              onclick={() => create(p.name, p.color)}
            >
              <Plus size={11} />
              {p.name}
            </button>
          {/each}
          <button
            class="text-caption font-medium text-[var(--accent)] hover:underline px-1"
            onclick={addAllPresets}
          >
            Add all
          </button>
        </div>
      </div>
    {:else if visible.length === 0}
      <div class="px-4 py-6 text-center text-body-sm text-[var(--text-faint)]">
        No labels match “{filter.trim()}”.
      </div>
    {:else}
      {#each visible as l, idx (l.id)}
        <div class="flex items-center gap-3 px-4 py-2.5 {idx > 0 ? 'border-t border-[var(--border)]' : ''}">
          {#if confirmingId === l.id}
            <!-- Delete / merge confirm (#4) -->
            <span class="size-3 rounded-full shrink-0" style="background: {l.color}"></span>
            <div class="flex-1 min-w-0">
              <p class="text-body-sm text-[var(--text)]">
                Delete <strong>{l.name}</strong>?
                {#if usageTotal(l.name) > 0}
                  <span class="text-[var(--text-muted)]">Detaches from {usageText(l.name)}.</span>
                {/if}
              </p>
              {#if labels.length > 1}
                <div class="flex items-center gap-1.5 mt-1.5">
                  <span class="text-caption text-[var(--text-muted)] shrink-0">or merge into</span>
                  <select
                    bind:value={mergeInto}
                    class="text-caption rounded border border-[var(--border)] bg-[var(--surface)]
                           text-[var(--text)] px-1.5 py-0.5 outline-none focus:border-[var(--accent)]"
                  >
                    <option value={null}>Choose label…</option>
                    {#each labels.filter((x) => x.id !== l.id) as t (t.id)}
                      <option value={t.id}>{t.name}</option>
                    {/each}
                  </select>
                  <button
                    class="text-caption font-medium text-[var(--accent-text)] bg-[var(--accent)] px-2 py-0.5 rounded
                           disabled:opacity-40 disabled:cursor-not-allowed"
                    disabled={mergeInto == null || busyId === l.id}
                    onclick={() => doMerge(l.id)}
                  >
                    Merge
                  </button>
                </div>
              {/if}
            </div>
            <button
              class="text-body-sm font-medium text-[var(--error-text)] bg-[var(--error)] px-2.5 py-1 rounded-md
                     hover:opacity-90 transition-opacity disabled:opacity-40 shrink-0"
              disabled={busyId === l.id}
              onclick={() => del(l.id)}
            >
              {busyId === l.id ? "…" : "Delete"}
            </button>
            <button
              class="text-body-sm text-[var(--text-muted)] px-2 py-1 rounded-md hover:bg-[var(--bg-subtle)] transition-colors shrink-0"
              onclick={() => { confirmingId = null; mergeInto = null; }}
            >
              Cancel
            </button>
          {:else}
            <!-- Display row: dot recolors, name renames, count links out -->
            <ColorPicker value={l.color} size={14} onChange={(c) => recolor(l, c)} />

            {#if editingId === l.id}
              <!-- svelte-ignore a11y_autofocus -->
              <input
                bind:value={editName}
                maxlength="40"
                autofocus
                class="flex-1 min-w-[120px] text-body-sm bg-transparent text-[var(--text)] outline-none
                       border-b border-[var(--accent)]"
                onblur={() => commitRename(l)}
                onkeydown={(e) => {
                  if (e.key === 'Enter') { e.preventDefault(); commitRename(l); }
                  if (e.key === 'Escape') { e.preventDefault(); editingId = null; }
                }}
              />
            {:else}
              <button
                class="group flex-1 min-w-0 flex items-center text-left rounded px-1 -mx-1 py-0.5
                       cursor-text hover:bg-[var(--bg-subtle)] transition-colors"
                title="Rename"
                onclick={() => startRename(l)}
              >
                <span class="text-body-sm font-medium text-[var(--text)] truncate">{l.name}</span>
              </button>
            {/if}

            {#if usageTotal(l.name) > 0}
              <button
                class="shrink-0 inline-flex items-center gap-1 text-caption text-[var(--text-faint)]
                       hover:text-[var(--accent)] transition-colors tabular-nums"
                title="View {l.name} issues"
                onclick={() => onOpenLabel?.(l.name)}
              >
                {usageText(l.name)}
                <ArrowRight size={11} />
              </button>
            {:else}
              <span class="shrink-0 text-caption text-[var(--text-faint)]">unused</span>
            {/if}

            <button
              class="size-7 grid place-items-center rounded-md text-[var(--text-muted)] shrink-0
                     hover:text-[var(--error)] hover:bg-[var(--error-bg)] transition-colors"
              onclick={() => startConfirm(l)}
              aria-label="Delete {l.name}"
            >
              <Trash2 size={14} />
            </button>
          {/if}
        </div>
      {/each}
    {/if}
  </div>
</section>

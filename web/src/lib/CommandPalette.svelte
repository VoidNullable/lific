<script lang="ts">
  // LIF-159 — command palette. cmd+k / ctrl+k from anywhere.
  //
  // One input that understands what you mean:
  //   "OMN156" / "omn 156" / "OMN-156"  → issue OMN-156, resolved directly
  //   "lif doc 3" / "LIF-DOC-3"         → that page
  //   "156"                              → issue #156 probed in EVERY project
  //   anything else                      → server FTS (issues+pages) merged
  //                                        with client fuzzy over projects,
  //                                        modules, and folders
  //
  // Mounted once in Layout so the session-cached catalog (projects ×
  // modules × folders) survives route changes. Selection navigates;
  // action commands are a follow-up.

  import {
    listProjects,
    listModules,
    listFolders,
    listPages,
    resolveIssue,
    search as searchApi,
    type Project,
    type Module,
    type Folder,
  } from "./api";
  import { fuzzyMatch } from "./fuzzy";
  import ProjectIcon from "./ProjectIcon.svelte";
  import {
    Search, CircleDot, FileText, Layers, FolderClosed, Box, CornerDownLeft,
    Zap, ChevronRight,
  } from "lucide-svelte";
  import { tick } from "svelte";
  import StatusIcon from "./StatusIcon.svelte";
  import PriorityIcon from "./PriorityIcon.svelte";
  import type { PaletteAction, PaletteActionChild } from "./palette";

  let {
    navigate,
    actions = [],
  }: {
    navigate: (path: string) => void;
    /** Context-aware actions registered by the current route (via
     *  Layout's "lific:palette" context → DocumentDetail). */
    actions?: PaletteAction[];
  } = $props();

  // ── Open/close + modes ───────────────────────────────
  //
  // root    — navigation search + action list
  // submenu — an action's children (statuses, labels, modules…)
  // prompt  — text input feeding an action (rename)

  type Mode =
    | { type: "root" }
    | { type: "submenu"; action: PaletteAction }
    | { type: "prompt"; action: PaletteAction };

  let open = $state(false);
  let mode = $state<Mode>({ type: "root" });
  let query = $state("");
  let inputEl = $state<HTMLInputElement | null>(null);
  let listEl = $state<HTMLDivElement | null>(null);
  let selectedIdx = $state(0);

  async function show() {
    open = true;
    mode = { type: "root" };
    query = "";
    selectedIdx = 0;
    await tick();
    inputEl?.focus();
    // First open: wait for the catalog before rendering the default
    // project-switcher list, or it flashes "No projects yet".
    await ensureCatalog();
    if (open && !query.trim()) void runSearch("");
  }

  function hide() {
    open = false;
    mode = { type: "root" };
  }

  // LIF-192: let the sidebar's "Jump to…" button summon the palette.
  export function openPalette() {
    void show();
  }

  /** Esc / backspace-on-empty: submenu/prompt step back; root closes. */
  function stepBack() {
    if (mode.type === "root") {
      hide();
      return;
    }
    mode = { type: "root" };
    query = "";
    selectedIdx = 0;
    void runSearch("");
    inputEl?.focus();
  }

  function enterAction(a: PaletteAction) {
    if (a.run) {
      hide();
      a.run();
      return;
    }
    if (a.children) {
      mode = { type: "submenu", action: a };
      query = "";
      selectedIdx = 0;
      inputEl?.focus();
      return;
    }
    if (a.prompt) {
      mode = { type: "prompt", action: a };
      query = a.prompt.initial ?? "";
      selectedIdx = 0;
      tick().then(() => inputEl?.select());
    }
  }

  function onWindowKeydown(e: KeyboardEvent) {
    // cmd/ctrl+K and cmd/ctrl+P both summon the palette (P overrides
    // the browser print dialog — jumping beats printing).
    if ((e.metaKey || e.ctrlKey) && ["k", "p"].includes(e.key.toLowerCase())) {
      e.preventDefault();
      if (open) hide();
      else void show();
      return;
    }
    if (!open) return;
    if (e.key === "Escape") {
      e.preventDefault();
      e.stopPropagation();
      stepBack();
    }
  }

  // ── Catalog: projects × (modules, folders), session-cached ──

  type Catalog = {
    projects: Project[];
    modules: Array<Module & { projectIdent: string }>;
    folders: Array<Folder & { projectIdent: string }>;
  };
  let catalog = $state<Catalog>({ projects: [], modules: [], folders: [] });
  let catalogAt = 0;
  const CATALOG_TTL = 60_000;

  async function ensureCatalog() {
    if (Date.now() - catalogAt < CATALOG_TTL) return;
    const projRes = await listProjects();
    if (!projRes.ok) return;
    const projects = projRes.data;

    const perProject = await Promise.all(
      projects.map(async (p) => {
        const [mods, flds] = await Promise.all([
          listModules(p.id),
          listFolders(p.id),
        ]);
        return {
          modules: (mods.ok ? mods.data : []).map((m) => ({
            ...m,
            projectIdent: p.identifier,
          })),
          folders: (flds.ok ? flds.data : []).map((f) => ({
            ...f,
            projectIdent: p.identifier,
          })),
        };
      }),
    );

    catalog = {
      projects,
      modules: perProject.flatMap((x) => x.modules),
      folders: perProject.flatMap((x) => x.folders),
    };
    catalogAt = Date.now();
  }

  // ── Results ──────────────────────────────────────────

  type PaletteResult = {
    kind: "issue" | "page" | "project" | "module" | "folder";
    title: string;
    identifier?: string;
    sub?: string;
    emoji?: string | null;
    route: string;
    score: number;
  };

  const GROUP_ORDER: PaletteResult["kind"][] = [
    "issue", "page", "project", "module", "folder",
  ];
  const GROUP_LABEL: Record<PaletteResult["kind"], string> = {
    issue: "Issues",
    page: "Pages",
    project: "Projects",
    module: "Modules",
    folder: "Folders",
  };
  const GROUP_CAP = 8;

  let results = $state<PaletteResult[]>([]);
  let searching = $state(false);
  let searchGen = 0;

  // Universal actions, available from every view. Context actions
  // (registered by the current route) list first since they're the more
  // likely intent on a detail page.
  const globalActions: PaletteAction[] = [
    {
      id: "new-project",
      title: "New project",
      run: () => navigate("/projects/new"),
    },
  ];

  let allActions = $derived([...actions, ...globalActions]);

  // Actions matched against the query (root mode only). Empty query
  // lists them all; they render above navigation results.
  let actionHits = $derived.by(() => {
    if (mode.type !== "root") return [] as PaletteAction[];
    const q = query.trim();
    if (!q) return allActions;
    return allActions
      .map((a) => ({ a, m: fuzzyMatch(q, a.title) }))
      .filter((x) => x.m !== null && x.m.score >= 0.3)
      .sort((x, y) => y.m!.score - x.m!.score)
      .map((x) => x.a);
  });

  // Submenu children filtered by the query.
  let childHits = $derived.by(() => {
    if (mode.type !== "submenu") return [] as PaletteActionChild[];
    const all = mode.action.children?.() ?? [];
    const q = query.trim();
    if (!q) return all;
    return all.filter((c) => (fuzzyMatch(q, c.title)?.score ?? 0) >= 0.3);
  });

  // Groups order by their strongest hit, not a fixed sequence — typing
  // a project name must surface Projects above a pile of FTS issue
  // matches (especially on detail pages, where Actions already sit on
  // top). GROUP_ORDER only breaks ties.
  let grouped = $derived.by(() => {
    // Nav results sit after the action list in the flat selection order.
    let flatIdx = mode.type === "root" ? actionHits.length : 0;
    const groups = GROUP_ORDER.map((kind, gi) => {
      const rs = results.filter((r) => r.kind === kind);
      return { kind, gi, rs, best: rs.reduce((m, r) => Math.max(m, r.score), 0) };
    }).filter((g) => g.rs.length > 0);
    groups.sort((a, b) => b.best - a.best || a.gi - b.gi);
    return groups.map((g) => ({
      label: GROUP_LABEL[g.kind],
      entries: g.rs.map((r) => ({ r, flatIdx: flatIdx++ })),
    }));
  });

  function projectByIdent(ident: string): Project | undefined {
    return catalog.projects.find(
      (p) => p.identifier.toLowerCase() === ident.toLowerCase(),
    );
  }

  /** Identifier fast-paths. Returns results for exact-shape queries. */
  async function identifierHits(q: string): Promise<PaletteResult[]> {
    const hits: PaletteResult[] = [];
    const compact = q.trim();

    // PROJ-DOC-n / "proj doc n" → page
    const pageMatch = compact.match(/^([a-z][a-z0-9_]*)[\s-]*doc[\s-]*(\d+)$/i);
    if (pageMatch) {
      const project = projectByIdent(pageMatch[1]);
      if (project) {
        const res = await listPages(project.id);
        if (res.ok) {
          const seq = parseInt(pageMatch[2]);
          const page = res.data.find((p) => p.sequence === seq);
          if (page) {
            hits.push({
              kind: "page",
              title: page.title,
              identifier: page.identifier,
              sub: project.name,
              route: `/${project.identifier}/pages/${page.id}`,
              score: 3,
            });
          }
        }
      }
      return hits;
    }

    // PROJ-n / "proj n" / "PROJn" → issue
    const issueMatch = compact.match(/^([a-z][a-z0-9_]*?)[\s-]*(\d+)$/i);
    if (issueMatch && projectByIdent(issueMatch[1])) {
      const project = projectByIdent(issueMatch[1])!;
      const ident = `${project.identifier}-${parseInt(issueMatch[2])}`;
      const res = await resolveIssue(ident);
      if (res.ok) {
        hits.push({
          kind: "issue",
          title: res.data.title,
          identifier: res.data.identifier,
          sub: `${project.name} · ${res.data.status}`,
          route: `/${project.identifier}/issues/${res.data.identifier}`,
          score: 3,
        });
      }
      return hits;
    }

    // Bare number → probe every project for issue #n
    const bare = compact.match(/^(\d+)$/);
    if (bare) {
      const n = parseInt(bare[1]);
      const probes = await Promise.all(
        catalog.projects.map(async (p) => {
          const res = await resolveIssue(`${p.identifier}-${n}`);
          return res.ok ? { project: p, issue: res.data } : null;
        }),
      );
      for (const hit of probes) {
        if (!hit) continue;
        hits.push({
          kind: "issue",
          title: hit.issue.title,
          identifier: hit.issue.identifier,
          sub: `${hit.project.name} · ${hit.issue.status}`,
          route: `/${hit.project.identifier}/issues/${hit.issue.identifier}`,
          score: 3,
        });
      }
    }

    return hits;
  }

  /** Client fuzzy over the cached catalog (projects/modules/folders). */
  function catalogHits(q: string): PaletteResult[] {
    const hits: PaletteResult[] = [];
    const ql = q.toLowerCase();
    for (const p of catalog.projects) {
      const m =
        fuzzyMatch(q, p.name) ??
        fuzzyMatch(q, p.identifier);
      if (m && m.score >= 0.3) {
        // Exact or prefix project matches outrank FTS text hits: typing
        // a project's name means "take me there", and Enter should land
        // on its issue list (the project's default view).
        let score = m.score;
        if (p.identifier.toLowerCase() === ql || p.name.toLowerCase() === ql) {
          score = 2.6;
        } else if (
          p.name.toLowerCase().startsWith(ql) ||
          p.identifier.toLowerCase().startsWith(ql)
        ) {
          score = Math.max(score, 2.2);
        }
        hits.push({
          kind: "project",
          title: p.name,
          identifier: p.identifier,
          emoji: p.emoji,
          route: `/${p.identifier}/overview`,
          score,
        });
      }
    }
    for (const mod of catalog.modules) {
      const m = fuzzyMatch(q, mod.name);
      if (m && m.score >= 0.3) {
        hits.push({
          kind: "module",
          title: mod.name,
          sub: mod.projectIdent,
          emoji: mod.emoji,
          route: `/${mod.projectIdent}/modules/${mod.id}`,
          score: m.score,
        });
      }
    }
    for (const f of catalog.folders) {
      const m = fuzzyMatch(q, f.name);
      if (m && m.score >= 0.3) {
        hits.push({
          kind: "folder",
          title: f.name,
          sub: f.projectIdent,
          route: `/${f.projectIdent}/pages`,
          score: m.score,
        });
      }
    }
    return hits;
  }

  async function runSearch(q: string) {
    const gen = ++searchGen;
    const trimmed = q.trim();

    // Empty query: quick project switcher.
    if (!trimmed) {
      results = catalog.projects.map((p) => ({
        kind: "project" as const,
        title: p.name,
        identifier: p.identifier,
        emoji: p.emoji,
        route: `/${p.identifier}/overview`,
        score: 1,
      }));
      selectedIdx = 0;
      return;
    }

    searching = true;
    const [idHits, ftsRes] = await Promise.all([
      identifierHits(trimmed),
      searchApi(trimmed),
    ]);
    if (gen !== searchGen) return; // superseded by a newer keystroke

    const merged: PaletteResult[] = [...idHits];
    const seen = new Set(idHits.map((h) => h.identifier));

    if (ftsRes.ok) {
      // FTS rank is positional — decay the score with position so
      // identifier hits and strong catalog matches outrank weak FTS tails.
      ftsRes.data.forEach((r, i) => {
        if (r.identifier && seen.has(r.identifier)) return;
        const project = catalog.projects.find((p) => p.id === r.project_id);
        const route =
          r.result_type === "page"
            ? project
              ? `/${project.identifier}/pages/${r.id}`
              : null
            : project && r.identifier
              ? `/${project.identifier}/issues/${r.identifier}`
              : null;
        if (!route) return;
        merged.push({
          kind: r.result_type === "page" ? "page" : "issue",
          title: r.title,
          identifier: r.identifier ?? undefined,
          sub: r.snippet || project?.name,
          route,
          score: 1 - i * 0.03,
        });
      });
    }

    merged.push(...catalogHits(trimmed));
    merged.sort((a, b) => b.score - a.score);

    // Cap per group.
    const counts = new Map<string, number>();
    results = merged.filter((r) => {
      const c = counts.get(r.kind) ?? 0;
      if (c >= GROUP_CAP) return false;
      counts.set(r.kind, c + 1);
      return true;
    });
    selectedIdx = 0;
    searching = false;
  }

  // Debounced search on keystroke.
  let debounce: ReturnType<typeof setTimeout> | null = null;
  function onInput() {
    if (mode.type === "prompt") return; // prompt input isn't a search
    if (mode.type === "submenu") {
      selectedIdx = 0; // childHits derives from query directly
      return;
    }
    if (debounce) clearTimeout(debounce);
    debounce = setTimeout(() => runSearch(query), 120);
  }

  // ── Selection + dispatch ─────────────────────────────

  type FlatItem =
    | { t: "action"; a: PaletteAction }
    | { t: "nav"; r: PaletteResult }
    | { t: "child"; c: PaletteActionChild };

  let flatItems = $derived.by<FlatItem[]>(() => {
    if (mode.type === "submenu") {
      return childHits.map((c) => ({ t: "child" as const, c }));
    }
    if (mode.type === "prompt") return [];
    return [
      ...actionHits.map((a) => ({ t: "action" as const, a })),
      ...grouped.flatMap((g) => g.entries.map((e) => ({ t: "nav" as const, r: e.r }))),
    ];
  });

  function pickItem(it: FlatItem) {
    if (it.t === "nav") {
      hide();
      navigate(it.r.route);
    } else if (it.t === "action") {
      enterAction(it.a);
    } else {
      hide();
      it.c.run();
    }
  }

  function onInputKeydown(e: KeyboardEvent) {
    if (mode.type === "prompt") {
      if (e.key === "Enter") {
        e.preventDefault();
        const submit = mode.action.prompt?.submit;
        const value = query.trim();
        hide();
        if (submit && value) submit(value);
      }
      return;
    }
    if (e.key === "ArrowDown") {
      e.preventDefault();
      selectedIdx = Math.min(selectedIdx + 1, flatItems.length - 1);
      scrollSelectedIntoView();
    } else if (e.key === "ArrowUp") {
      e.preventDefault();
      selectedIdx = Math.max(selectedIdx - 1, 0);
      scrollSelectedIntoView();
    } else if (e.key === "Enter") {
      e.preventDefault();
      const it = flatItems[selectedIdx];
      if (it) pickItem(it);
    } else if (e.key === "Backspace" && !query && mode.type === "submenu") {
      e.preventDefault();
      stepBack();
    }
  }

  function scrollSelectedIntoView() {
    requestAnimationFrame(() => {
      listEl
        ?.querySelector(`[data-flat-idx="${selectedIdx}"]`)
        ?.scrollIntoView({ block: "nearest" });
    });
  }
</script>

<svelte:window onkeydown={onWindowKeydown} />

{#if open}
  <!-- svelte-ignore a11y_no_static_element_interactions a11y_click_events_have_key_events -->
  <div
    class="fixed inset-0 z-[100] bg-black/25 flex items-start justify-center
           pt-[14vh] px-4"
    onclick={hide}
  >
    <!-- svelte-ignore a11y_no_static_element_interactions a11y_click_events_have_key_events -->
    <div
      class="w-full max-w-[580px] bg-[var(--surface)] border border-[var(--border)]
             rounded-xl shadow-[0_16px_48px_rgba(0,0,0,0.28)] overflow-hidden"
      onclick={(e) => e.stopPropagation()}
    >
      <!-- Input row -->
      <div class="flex items-center gap-2.5 px-4 py-3 border-b border-[var(--border)]">
        {#if mode.type === "root"}
          <Search size={15} class="shrink-0 text-[var(--text-faint)]" />
        {:else}
          <Zap size={15} class="shrink-0 text-[var(--accent)]" />
          <!-- Breadcrumb chip: which action's submenu/prompt this is. -->
          <span
            class="shrink-0 text-caption font-medium text-[var(--accent)]
                   bg-[var(--accent-subtle)] px-2 py-0.5 rounded-full
                   whitespace-nowrap"
          >
            {mode.action.title.replace(/…$/, "")}
          </span>
        {/if}
        <input
          bind:this={inputEl}
          bind:value={query}
          type="text"
          class="flex-1 bg-transparent border-0 outline-none text-[0.9375rem]
                 text-[var(--text)] placeholder:text-[var(--text-faint)]"
          placeholder={mode.type === "prompt"
            ? (mode.action.prompt?.placeholder ?? "Type a value…")
            : mode.type === "submenu"
              ? "Filter…"
              : "Jump or act… (try OMN156, doc 3, or “status”)"}
          oninput={onInput}
          onkeydown={onInputKeydown}
        />
        <kbd
          class="px-1.5 py-0.5 rounded border border-[var(--border)]
                 bg-[var(--bg-subtle)] text-[var(--text-faint)]
                 font-mono text-micro leading-none shrink-0"
        >
          esc
        </kbd>
      </div>

      <!-- Results -->
      {#if mode.type === "prompt"}
        <p class="px-4 py-3 text-caption text-[var(--text-faint)]">
          Enter to save · Esc to cancel
        </p>
      {:else}
      <div class="max-h-[420px] overflow-y-auto py-1.5" bind:this={listEl}>
        {#if flatItems.length === 0}
          <p class="px-4 py-6 text-center text-body-sm text-[var(--text-faint)]">
            {searching
              ? "Searching…"
              : query.trim()
                ? `Nothing matches “${query.trim()}”`
                : mode.type === "submenu"
                  ? "Nothing here"
                  : "No projects yet"}
          </p>
        {:else if mode.type === "submenu"}
          {#each childHits as c, i (c.title)}
            <button
              class="w-full flex items-center gap-2.5 px-4 py-2 text-left
                     transition-colors
                     {i === selectedIdx
                ? 'bg-[var(--accent-subtle)]'
                : 'hover:bg-[var(--bg-subtle)]'}"
              data-flat-idx={i}
              onclick={() => pickItem({ t: "child", c })}
              onmouseenter={() => { selectedIdx = i; }}
            >
              <span class="size-5 flex items-center justify-center shrink-0">
                {#if c.status !== undefined}
                  <StatusIcon status={c.status} size={14} />
                {:else if c.priority !== undefined}
                  <PriorityIcon priority={c.priority} size={14} />
                {:else if c.color}
                  <span
                    class="size-2.5 rounded-full"
                    style="background: {c.color}"
                  ></span>
                {/if}
              </span>
              <span class="flex-1 text-body text-[var(--text)] capitalize truncate">
                {c.title}
              </span>
              {#if c.hint}
                <span class="text-micro text-[var(--text-faint)] shrink-0">
                  {c.hint}
                </span>
              {/if}
              {#if i === selectedIdx}
                <CornerDownLeft size={12} class="shrink-0 text-[var(--text-faint)]" />
              {/if}
            </button>
          {/each}
        {:else}
          <!-- Context actions first: on a detail page they're the most
               likely intent. -->
          {#if actionHits.length > 0}
            <div
              class="px-4 pt-2 pb-1 text-micro font-semibold uppercase
                     tracking-widest text-[var(--text-faint)]"
            >
              Actions
            </div>
            {#each actionHits as a, i (a.id)}
              <button
                class="w-full flex items-center gap-2.5 px-4 py-2 text-left
                       transition-colors
                       {i === selectedIdx
                  ? 'bg-[var(--accent-subtle)]'
                  : 'hover:bg-[var(--bg-subtle)]'}"
                data-flat-idx={i}
                onclick={() => pickItem({ t: "action", a })}
                onmouseenter={() => { selectedIdx = i; }}
              >
                <span class="size-5 flex items-center justify-center shrink-0 text-[var(--accent)]">
                  <Zap size={14} />
                </span>
                <span class="flex-1 text-body text-[var(--text)] truncate">
                  {a.title}
                </span>
                {#if a.hint}
                  <span class="text-micro text-[var(--text-faint)] capitalize shrink-0">
                    {a.hint}
                  </span>
                {/if}
                {#if a.children}
                  <ChevronRight size={12} class="shrink-0 text-[var(--text-faint)]" />
                {:else if i === selectedIdx}
                  <CornerDownLeft size={12} class="shrink-0 text-[var(--text-faint)]" />
                {/if}
              </button>
            {/each}
          {/if}
          {#each grouped as group (group.label)}
            <div
              class="px-4 pt-2 pb-1 text-micro font-semibold uppercase
                     tracking-widest text-[var(--text-faint)]"
            >
              {group.label}
            </div>
            {#each group.entries as { r, flatIdx } (r.route + (r.identifier ?? r.title))}
              <button
                class="w-full flex items-center gap-2.5 px-4 py-2 text-left
                       transition-colors
                       {flatIdx === selectedIdx
                  ? 'bg-[var(--accent-subtle)]'
                  : 'hover:bg-[var(--bg-subtle)]'}"
                data-flat-idx={flatIdx}
                onclick={() => pickItem({ t: "nav", r })}
                onmouseenter={() => { selectedIdx = flatIdx; }}
              >
                <!-- Kind icon (project/module emoji wins when set) -->
                <span class="size-5 flex items-center justify-center shrink-0 text-[var(--text-faint)]">
                  {#if r.emoji}
                    <ProjectIcon value={r.emoji} size={15} />
                  {:else if r.kind === "issue"}
                    <CircleDot size={14} />
                  {:else if r.kind === "page"}
                    <FileText size={14} />
                  {:else if r.kind === "module"}
                    <Layers size={14} />
                  {:else if r.kind === "folder"}
                    <FolderClosed size={14} />
                  {:else}
                    <Box size={14} />
                  {/if}
                </span>

                <span class="flex-1 min-w-0 flex items-baseline gap-2">
                  <span class="text-body text-[var(--text)] truncate">
                    {r.title}
                  </span>
                  {#if r.sub}
                    <span class="text-caption text-[var(--text-faint)] truncate shrink-[2]">
                      {r.sub}
                    </span>
                  {/if}
                </span>

                {#if r.identifier}
                  <span class="font-mono text-micro text-[var(--text-faint)] shrink-0">
                    {r.identifier}
                  </span>
                {/if}
                {#if flatIdx === selectedIdx}
                  <CornerDownLeft size={12} class="shrink-0 text-[var(--text-faint)]" />
                {/if}
              </button>
            {/each}
          {/each}
        {/if}
      </div>
      {/if}
    </div>
  </div>
{/if}

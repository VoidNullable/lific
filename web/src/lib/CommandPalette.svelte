<script lang="ts">
  // LIF-159 — command palette. cmd+k / ctrl+k from anywhere.
  //
  // One input that understands what you mean:
  //   "OMN156" / "omn 156" / "OMN-156"  → issue OMN-156, resolved directly
  //   "lif doc 3" / "LIF-DOC-3"         → that page
  //   "156" / "#156"                     → issue #156 probed in EVERY project,
  //                                        the current project's pinned first
  //   "doc 3"                            → page 3 of the current project
  //   (empty)                            → recently viewed, then projects
  //   anything else                      → local search over the selected
  //                                        project's warm read model, merged
  //                                        with client fuzzy over projects,
  //                                        modules and folders, then topped
  //                                        up by server FTS if thin
  //
  // LIF-445: the local pass is synchronous and runs on the keystroke, so a
  // warm project answers before the next frame. The server round trip is
  // debounced and only fires when the in-memory answer is thin (fewer than
  // LOCAL_HIT_SERVER_THRESHOLD issue/page hits) or the project is cold.
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
  import { mobileNavState } from "./mobileNavState.svelte";
  import {
    CURRENT_PROJECT_REF_SCORE,
    EXACT_REF_SCORE,
    LOCAL_HIT_SERVER_THRESHOLD,
    dedupeByIdentifier,
    dedupeByKey,
    isStaleSearch,
    localScoreToPaletteScore,
    parseRefQuery,
    preserveSelection,
    refIdentifier,
    projectCatalogChanged,
    searchLocalDocsPerKind,
    type RefQuery,
  } from "./paletteSearch";
  import { cachedProject, getProjectModel, peekProjectModel } from "./sync/readModel.svelte";
  import { getRecents, recentRoute } from "./home/recents";
  import { safeLabelColor } from "./labelColors";
  import { commandPaletteState } from "./commandPaletteState.svelte";
  import { shortcutHelpState } from "./shortcutHelpState.svelte";
  import ProjectIcon from "./ProjectIcon.svelte";
  import {
    Search, CircleDot, FileText, Layers, FolderClosed, Box,
    Zap, ChevronRight, X, ListChecks,
  } from "lucide-svelte";
  import { tick, untrack } from "svelte";
  import StatusIcon from "./StatusIcon.svelte";
  import PriorityIcon from "./PriorityIcon.svelte";
  import type { PaletteAction, PaletteActionChild } from "./palette";

  let {
    navigate,
    route = "",
    actions = [],
  }: {
    navigate: (path: string) => void;
    /** The current path, from Layout. The palette is mounted once and
     *  outlives every route, so this prop (not the location bar) is the
     *  authoritative answer to "which project am I in". */
    route?: string;
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
  // Phones get a shorter syntax hint: the overlay is full-screen there and
  // the line has no card to sit in.
  let innerWidth = $state(1024);
  let narrow = $derived(innerWidth < 640);
  let mode = $state<Mode>({ type: "root" });
  let query = $state("");
  let inputEl = $state<HTMLInputElement | null>(null);
  let listEl = $state<HTMLDivElement | null>(null);
  let selectedIdx = $state(0);
  let openingGeneration = 0;

  async function show() {
    const generation = ++openingGeneration;
    open = true;
    commandPaletteState.open = true;
    cancelSearch();
    mode = { type: "root" };
    query = "";
    selectedIdx = 0;
    await tick();
    if (generation !== openingGeneration || !open) return;
    inputEl?.focus();
    // The project switcher only needs projects. Module and folder metadata
    // can finish loading after the palette becomes usable.
    await ensureProjects();
    if (generation === openingGeneration && open && mode.type === "root") {
      void runSearch(query);
    }
    void ensureCatalog();
  }

  function hide() {
    openingGeneration++;
    open = false;
    commandPaletteState.open = false;
    // A response that lands after the palette closes must not repopulate it.
    cancelSearch();
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
      cancelSearch(); // a nav response must not land on top of the submenu
      mode = { type: "submenu", action: a };
      query = "";
      selectedIdx = 0;
      inputEl?.focus();
      return;
    }
    if (a.prompt) {
      cancelSearch();
      mode = { type: "prompt", action: a };
      query = a.prompt.initial ?? "";
      selectedIdx = 0;
      tick().then(() => inputEl?.select());
    }
  }

  function onWindowKeydown(e: KeyboardEvent) {
    // cmd/ctrl+K and cmd/ctrl+P both summon the palette (P overrides
    // the browser print dialog — jumping beats printing). LIF-245: don't
    // summon it on top of the shortcut help overlay — that one owns Esc
    // via its own listener and the two stacking would just be confusing.
    if ((e.metaKey || e.ctrlKey) && ["k", "p"].includes(e.key.toLowerCase())) {
      if (shortcutHelpState.open || mobileNavState.open) return;
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
  let projectsAt = 0;
  const CATALOG_TTL = 60_000;
  let catalogLoad: Promise<void> | null = null;
  let projectsLoad: Promise<Project[] | null> | null = null;
  let catalogGeneration = 0;

  function ensureProjects(): Promise<Project[] | null> {
    if (Date.now() - projectsAt < CATALOG_TTL) return Promise.resolve(catalog.projects);
    if (projectsLoad) return projectsLoad;

    projectsLoad = listProjects()
      .then((response) => {
        if (!response.ok) return null;
        const projectsChanged = projectCatalogChanged(catalog.projects, response.data);
        catalog = projectsChanged
          ? { projects: response.data, modules: [], folders: [] }
          : { ...catalog, projects: response.data };
        if (projectsChanged) {
          catalogAt = 0;
          catalogGeneration += 1;
        }
        projectsAt = Date.now();
        return response.data;
      })
      .finally(() => {
        projectsLoad = null;
      });
    return projectsLoad;
  }

  async function ensureCatalog(): Promise<void> {
    if (Date.now() - catalogAt < CATALOG_TTL) return;
    if (catalogLoad) return catalogLoad;
    let loadedGeneration: number | null = null;
    catalogLoad = (async () => {
      const projects = await ensureProjects();
      if (!projects) return;
      const generation = catalogGeneration;
      loadedGeneration = generation;
      const modules: Catalog["modules"] = [];
      const folders: Catalog["folders"] = [];
      // Bound the fanout: two requests per project, four projects per batch.
      for (let start = 0; start < projects.length; start += 4) {
        if (!open || generation !== catalogGeneration) return;
        const batch = await Promise.all(
          projects.slice(start, start + 4).map(async (project) => {
            const [mods, flds] = await Promise.all([
              listModules(project.id),
              listFolders(project.id),
            ]);
            return { project, mods, flds };
          }),
        );
        for (const { project, mods, flds } of batch) {
          if (mods.ok) modules.push(...mods.data.map((mod) => ({
            ...mod, projectIdent: project.identifier,
          })));
          if (flds.ok) folders.push(...flds.data.map((folder) => ({
            ...folder, projectIdent: project.identifier,
          })));
        }
      }
      if (generation !== catalogGeneration) return;
      catalog = { projects, modules, folders };
      catalogAt = Date.now();
      if (open && mode.type === "root" && query.trim()) refreshCatalogSearch();
    })().finally(() => {
      catalogLoad = null;
      if (open && loadedGeneration !== null && loadedGeneration !== catalogGeneration) {
        void ensureCatalog();
      }
    });
    return catalogLoad;
  }

  // ── Results ──────────────────────────────────────────

  type PaletteResult = {
    kind: "issue" | "page" | "plan" | "project" | "module" | "folder";
    title: string;
    identifier?: string;
    /** Owning project's name, shown as the row's muted right column. */
    project?: string;
    /** Server FTS snippet with `**match**` markers: why a server hit matched. */
    snippet?: string;
    emoji?: string | null;
    /** Issue status, when known, drawn as the row's icon. */
    status?: string;
    /** Issue priority, when known, drawn as a small trailing icon. */
    priority?: string;
    route: string;
    score: number;
    remote?: boolean;
    /** Server hit that contains only some of the query's words. */
    partial?: boolean;
    /** A recently viewed item, listed in its own group on an empty query. */
    recent?: boolean;
  };

  type SnippetSegment = { text: string; highlighted: boolean };

  /** Split paired FTS `**match**` markers without interpreting any HTML. */
  function snippetSegments(snippet: string): SnippetSegment[] {
    const segments: SnippetSegment[] = [];
    let cursor = 0;

    while (cursor < snippet.length) {
      const start = snippet.indexOf("**", cursor);
      if (start === -1) {
        segments.push({ text: snippet.slice(cursor), highlighted: false });
        break;
      }

      const end = snippet.indexOf("**", start + 2);
      if (end === -1) {
        // An unmatched trailing marker is content, not formatting.
        segments.push({ text: snippet.slice(cursor), highlighted: false });
        break;
      }

      if (start > cursor) {
        segments.push({ text: snippet.slice(cursor, start), highlighted: false });
      }
      if (end > start + 2) {
        segments.push({ text: snippet.slice(start + 2, end), highlighted: true });
      }
      cursor = end + 2;
    }

    return segments;
  }

  const GROUP_ORDER: PaletteResult["kind"][] = [
    "issue", "page", "plan", "project", "module", "folder",
  ];
  const GROUP_LABEL: Record<PaletteResult["kind"], string> = {
    issue: "Issues",
    page: "Pages",
    plan: "Plans",
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
    // A reference is navigation. Actions list above results, so letting
    // one fuzzy-match "34" would bury the issue it names.
    if (parseRefQuery(q)) return [] as PaletteAction[];
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
    const recent = results.filter((r) => r.recent);
    const groups = [false, true].flatMap((remote) => {
      const section = GROUP_ORDER.map((kind, gi) => {
        const rs = results.filter(
          (r) => !r.recent && r.kind === kind && Boolean(r.remote) === remote,
        );
        return { kind, gi, rs, remote, best: rs.reduce((m, r) => Math.max(m, r.score), 0) };
      }).filter((g) => g.rs.length > 0);
      return section.sort((a, b) => b.best - a.best || a.gi - b.gi);
    });
    const labelled = groups.map((g) => ({
      label: g.remote
        ? g.rs.some((r) => r.partial)
          ? `${GROUP_LABEL[g.kind]} (server, partial matches)`
          : `${GROUP_LABEL[g.kind]} (server)`
        : GROUP_LABEL[g.kind],
      rs: g.rs,
    }));
    if (recent.length > 0) labelled.unshift({ label: "Recent", rs: recent });
    return labelled.map((g) => ({
      label: g.label,
      entries: g.rs.map((r) => ({ r, flatIdx: flatIdx++ })),
    }));
  });

  function projectName(ident: string): string {
    return projectByIdent(ident)?.name ?? ident;
  }

  function projectByIdent(ident: string): Project | undefined {
    return catalog.projects.find(
      (p) => p.identifier.toLowerCase() === ident.toLowerCase(),
    );
  }

  /** The project a reference points into. An unqualified one ("34",
   *  "doc 3") means the project you are in. */
  function refProject(ref: RefQuery): Project | null {
    return ref.project === null ? activeProject() : (projectByIdent(ref.project) ?? null);
  }

  /** Unqualified references into the current project are pinned above
   *  everything; any other exact reference ranks as before. */
  function refScore(ref: RefQuery, project: Project): number {
    return ref.project === null && project.id === activeProject()?.id
      ? CURRENT_PROJECT_REF_SCORE
      : EXACT_REF_SCORE;
  }

  function issueResult(
    project: Project,
    issue: { title: string; identifier: string; status: string; priority?: string },
    score: number,
  ): PaletteResult {
    return {
      kind: "issue",
      title: issue.title,
      identifier: issue.identifier,
      project: project.name,
      status: issue.status,
      priority: issue.priority,
      route: `/${project.identifier}/issues/${issue.identifier}`,
      score,
    };
  }

  /** Resolve a reference from a warm replica, without a network call, so
   *  "34" lands on the current project's issue in the same frame. Only
   *  peeks: a lookup must not register a replica for an unopened project. */
  function modelRefHit(project: Project, ref: RefQuery, score: number): PaletteResult | null {
    const model = peekProjectModel(project.id);
    if (model?.status !== "ready") return null;
    const want = refIdentifier(project.identifier, ref).toLowerCase();
    if (ref.kind === "issue") {
      const row = model.issueList.find((r) => r.identifier.toLowerCase() === want);
      return row ? issueResult(project, row, score) : null;
    }
    const page = model.pageList.find((r) => r.identifier.toLowerCase() === want);
    return page
      ? {
          kind: "page",
          title: page.title,
          identifier: page.identifier,
          project: project.name,
          route: `/${project.identifier}/pages/${page.id}`,
          score,
        }
      : null;
  }

  /** The synchronous half of the reference fast path. */
  function localRefHits(q: string): PaletteResult[] {
    const ref = parseRefQuery(q);
    if (!ref) return [];
    const project = refProject(ref);
    if (!project) return [];
    const hit = modelRefHit(project, ref, refScore(ref, project));
    return hit ? [hit] : [];
  }

  /** Identifier fast-paths. Returns results for exact-shape queries. */
  async function identifierHits(q: string): Promise<PaletteResult[]> {
    const ref = parseRefQuery(q);
    if (!ref) return [];

    // PROJ-DOC-n / "proj doc n" / "doc n" → page
    if (ref.kind === "page") {
      const project = refProject(ref);
      if (!project) return [];
      const res = await listPages(project.id);
      if (!res.ok) return [];
      const page = res.data.find((p) => p.sequence === ref.n);
      return page
        ? [{
            kind: "page",
            title: page.title,
            identifier: page.identifier,
            project: project.name,
            route: `/${project.identifier}/pages/${page.id}`,
            score: refScore(ref, project),
          }]
        : [];
    }

    // PROJ-n / "proj n" / "PROJn" → issue
    if (ref.project !== null) {
      const project = projectByIdent(ref.project);
      if (!project) return [];
      const res = await resolveIssue(refIdentifier(project.identifier, ref));
      return res.ok ? [issueResult(project, res.data, EXACT_REF_SCORE)] : [];
    }

    // Bare number → probe every project for issue #n. The current project
    // leads (and scores above the rest), so Enter means "#n here".
    const current = activeProject();
    const others = catalog.projects.filter((p) => p.id !== current?.id);
    const probes = await Promise.all(
      (current ? [current, ...others] : others).map(async (p) => {
        const res = await resolveIssue(refIdentifier(p.identifier, ref));
        return res.ok ? issueResult(p, res.data, refScore(ref, p)) : null;
      }),
    );
    return probes.filter((h): h is PaletteResult => h !== null);
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
          project: projectName(mod.projectIdent),
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
          project: projectName(f.projectIdent),
          route: `/${f.projectIdent}/pages`,
          score: m.score,
        });
      }
    }
    return hits;
  }

  // ── Local pass (LIF-445) ─────────────────────────────
  //
  // Which project's read model we search is derived reactively from the
  // `route` prop, so navigating out from under an open palette both cancels
  // the in-flight request and re-runs the local pass against the new
  // project — rather than merging project A's local rows with project B's
  // server rows.

  let activeProjectIdent = $derived(
    route.match(/^\/([A-Za-z][A-Za-z0-9_-]*)\//)?.[1] ?? null,
  );

  function activeProject(): Project | null {
    const ident = activeProjectIdent;
    if (!ident) return null;
    return projectByIdent(ident) ?? cachedProject(ident);
  }

  /** Issue + page hits from the selected project's read model, but only when
   *  it is `ready`. A loading or cold model has nothing to say, and guessing
   *  from a half-filled replica would rank worse than the server. */
  function localHits(q: string): PaletteResult[] {
    const project = activeProject();
    if (!project) return [];
    const model = getProjectModel(project.id);
    if (model.status !== "ready") return [];

    // Per kind, not a shared budget: pages must not be crowded out by a
    // project whose issues all match.
    const docs = [...model.issueList, ...model.pageList];
    return searchLocalDocsPerKind(q, docs, GROUP_CAP).map(({ doc, score }) => ({
      kind: doc.kind,
      title: doc.title,
      identifier: doc.identifier,
      project: project.name,
      status: doc.kind === "issue" ? doc.status : undefined,
      priority: doc.kind === "issue" ? doc.priority : undefined,
      route:
        doc.kind === "page"
          ? `/${project.identifier}/pages/${doc.id}`
          : `/${project.identifier}/issues/${doc.identifier}`,
      score: localScoreToPaletteScore(score),
    }));
  }

  /** Stable identity for a result row, matching the `{#each}` key. */
  function resultKey(r: PaletteResult): string {
    return r.route + (r.identifier ?? r.title);
  }

  function flatKey(it: FlatItem): string {
    if (it.t === "action") return `a:${it.a.id}`;
    if (it.t === "child") return `c:${it.c.title}`;
    return `n:${resultKey(it.r)}`;
  }

  /** Sort, cap per group, and publish. */
  function publish(merged: PaletteResult[], keepSelection: boolean) {
    const previousKey = keepSelection
      ? (flatItems[selectedIdx] ? flatKey(flatItems[selectedIdx]) : null)
      : null;
    const previousIdx = selectedIdx;

    merged.sort((a, b) => b.score - a.score);
    // Sort THEN dedupe: the identifier fast path and the read model both
    // answer "LIF-445" with the same row, and the higher-scoring exact
    // reference is the one worth keeping. Two rows sharing a `{#each}` key
    // is a Svelte runtime error, so this is not optional.
    const unique = dedupeByKey(merged, resultKey);

    const counts = new Map<string, number>();
    results = unique.filter((r) => {
      const c = counts.get(r.kind) ?? 0;
      if (c >= GROUP_CAP) return false;
      counts.set(r.kind, c + 1);
      return true;
    });

    // Only a cursor the user moved is worth keeping. An untouched one sits on
    // the top row by default, and a late exact reference belongs there.
    selectedIdx = keepSelection && cursorMoved
      ? preserveSelection(previousKey, flatItems.map(flatKey), previousIdx)
      : 0;
  }

  /** Set by the arrow keys, cleared by every new query. */
  let cursorMoved = false;

  const RECENT_CAP = 5;

  /** Recently viewed items for the empty palette, minus the one on screen.
   *  Titles and statuses refresh from a warm replica when there is one, and
   *  an item that replica no longer has is dropped as deleted or moved. */
  function recentResults(): PaletteResult[] {
    const here = route.toLowerCase();
    const known = new Set(catalog.projects.map((p) => p.identifier.toLowerCase()));
    const out: PaletteResult[] = [];
    for (const e of getRecents()) {
      if (out.length >= RECENT_CAP) break;
      const path = recentRoute(e);
      if (path.toLowerCase() === here) continue;
      if (known.size > 0 && !known.has(e.project.toLowerCase())) continue;
      const project = projectByIdent(e.project) ?? cachedProject(e.project);
      const r: PaletteResult = {
        kind: e.type,
        title: e.title,
        identifier: e.identifier,
        project: project?.name ?? e.project,
        route: path,
        score: 1,
        recent: true,
      };
      const model = project ? peekProjectModel(project.id) : null;
      if (model?.status === "ready" && e.type !== "plan") {
        if (e.type === "issue") {
          const want = e.identifier.toLowerCase();
          const row = model.issueList.find((i) => i.identifier.toLowerCase() === want);
          if (!row) continue;
          r.title = row.title;
          r.status = row.status;
          r.priority = row.priority;
        } else {
          const row = model.pages.get(Number(e.routeId));
          if (!row) continue;
          r.title = row.title;
        }
      }
      out.push(r);
    }
    return out;
  }

  // The synchronous half of the last search, replayed when the server half
  // lands so a slow response never drops the local answer.
  let pendingQuery = "";
  let pendingLocal: PaletteResult[] = [];
  let pendingCatalog: PaletteResult[] = [];
  let completedRemote: {
    query: string;
    projectIdent: string | null;
    generation: number;
    results: PaletteResult[];
  } | null = null;
  /** The project the pending local rows were computed from. A response is
   *  only allowed to merge with local rows from the same project. */
  let pendingProjectIdent: string | null = null;

  /** Everything answerable without a network call. Renders immediately. */
  function runLocal(q: string): number {
    const trimmed = q.trim();
    cursorMoved = false;

    // Empty query: jump back to something recent, or switch project.
    if (!trimmed) {
      pendingQuery = "";
      pendingLocal = [];
      pendingCatalog = [];
      pendingProjectIdent = activeProjectIdent;
      results = [
        ...recentResults(),
        ...catalog.projects.map((p) => ({
          kind: "project" as const,
          title: p.name,
          identifier: p.identifier,
          emoji: p.emoji,
          route: `/${p.identifier}/overview`,
          score: 1,
        })),
      ];
      selectedIdx = 0;
      return 0;
    }

    updatePendingHits(trimmed);
    publish([...pendingLocal, ...pendingCatalog], false);
    return pendingLocal.length;
  }

  function updatePendingHits(trimmed: string) {
    pendingQuery = trimmed;
    pendingProjectIdent = activeProjectIdent;
    pendingLocal = [...localRefHits(trimmed), ...localHits(trimmed)];
    pendingCatalog = catalogHits(trimmed);
  }

  /** Rebuild local/catalog hits without restarting an already completed
   *  identifier or FTS request for the same query and project. */
  function refreshCatalogSearch() {
    const trimmed = query.trim();
    if (!trimmed) {
      runLocal(query);
      return;
    }

    updatePendingHits(trimmed);
    const cached = completedRemote;
    const remote =
      cached &&
      cached.query === trimmed &&
      cached.projectIdent === activeProjectIdent &&
      cached.generation === searchGen
        ? cached.results
        : [];
    publish([...remote, ...pendingLocal, ...pendingCatalog], true);
  }

  /** The network half: identifier fast paths always, server FTS only when
   *  the local answer was thin. Both are guarded by `gen` and by the project
   *  they were issued against. */
  async function runRemote(q: string, gen: number, wantFts: boolean) {
    const trimmed = q.trim();
    if (!trimmed) return;
    const issued = { gen, projectIdent: activeProjectIdent };

    searching = true;
    let idHits: PaletteResult[] = [];
    let fts: Awaited<ReturnType<typeof searchApi>> | null = null;
    try {
      [idHits, fts] = await Promise.all([
        identifierHits(trimmed),
        wantFts ? searchApi(trimmed) : Promise.resolve(null),
      ]);
    } finally {
      if (gen === searchGen) searching = false;
    }

    // Superseded by a newer keystroke, a mode change, a close, or a project
    // switch. `searchGen` covers the first three; the project check covers
    // navigating out from under an in-flight request.
    if (isStaleSearch(issued, { gen: searchGen, projectIdent: activeProjectIdent })) {
      return;
    }
    // The local rows must come from the same project as this response, or
    // the merge would splice project A's issues into project B's results.
    if (pendingProjectIdent !== issued.projectIdent || pendingQuery !== trimmed) {
      enterWhenSettled = null;
      return;
    }

    const remoteResults: PaletteResult[] = [...idHits];

    if (fts?.ok) {
      // FTS rank is positional — decay the score with position so identifier
      // hits, local hits and strong catalog matches outrank weak FTS tails.
      // A failed request is simply not merged: the local results stand.
      const fresh = dedupeByIdentifier(fts.data, [
        ...idHits.map((h) => h.identifier),
        ...pendingLocal.map((h) => h.identifier),
      ]);
      fresh.forEach((r, i) => {
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
        remoteResults.push({
          kind: r.result_type === "page" ? "page" : "issue",
          title: r.title,
          identifier: r.identifier ?? undefined,
          project: project?.name,
          snippet: r.snippet || undefined,
          route,
          score: 1 - i * 0.03,
          remote: true,
          partial: r.partial_match === true,
        });
      });
    }

    completedRemote = {
      query: trimmed,
      projectIdent: issued.projectIdent,
      generation: gen,
      results: remoteResults,
    };
    publish([...remoteResults, ...pendingLocal, ...pendingCatalog], true);

    // Enter pressed while this reference was still resolving: open the top
    // result now that it is the right one.
    if (enterWhenSettled) {
      const { newTab } = enterWhenSettled;
      enterWhenSettled = null;
      const it = flatItems[selectedIdx];
      if (it) pickItem(it, newTab);
    }
  }

  /** Cancel the debounce AND invalidate any response already in flight, so a
   *  stale answer cannot land after the query, mode or project moved on. */
  function cancelSearch() {
    searchGen++;
    if (debounce) {
      clearTimeout(debounce);
      debounce = null;
    }
    flushRemote = null;
    enterWhenSettled = null;
    searching = false;
  }

  // Typing "fic 34⏎" faster than the debounce used to open whatever the
  // local pass had ranked first. For a reference whose answer is still on
  // its way, Enter is remembered and carried out when the answer lands.
  let flushRemote: (() => void) | null = null;
  let enterWhenSettled: { newTab: boolean } | null = null;

  function shouldAwaitReference(): boolean {
    if (mode.type !== "root" || cursorMoved) return false;
    if (!debounce && !searching) return false;
    const ref = parseRefQuery(query);
    if (!ref) return false;
    const current = activeProject();
    // A bare number probes every project even outside one; anything else
    // needs a project to resolve against.
    if (!refProject(ref) && !(ref.kind === "issue" && ref.project === null)) return false;
    const top = flatItems[0];
    if (top?.t === "nav" && top.r.score >= EXACT_REF_SCORE) {
      // Already resolved locally. An unqualified reference inside a project
      // is only settled once that project's own match is on top.
      if (ref.project !== null || !current) return false;
      if (top.r.score >= CURRENT_PROJECT_REF_SCORE) return false;
    }
    return true;
  }

  function runSearch(q: string) {
    cancelSearch();
    const gen = searchGen;
    const local = runLocal(q);
    if (!q.trim()) return;
    void runRemote(q, gen, local < LOCAL_HIT_SERVER_THRESHOLD);
  }

  // The route can move under an open palette (a peek panel, a background
  // navigation). When it crosses a project boundary the pending local rows
  // are from the wrong replica, so cancel whatever is in flight and redo the
  // search against the new project instead of letting the two merge.
  $effect(() => {
    const ident = activeProjectIdent;
    if (!open || mode.type !== "root") return;
    if (ident === pendingProjectIdent) return;
    untrack(() => runSearch(query));
  });

  // Local results render on the keystroke; only the server half is debounced.
  let debounce: ReturnType<typeof setTimeout> | null = null;
  function onInput() {
    if (mode.type === "prompt") return; // prompt input isn't a search
    if (mode.type === "submenu") {
      selectedIdx = 0; // childHits derives from query directly
      return;
    }
    cancelSearch();
    const gen = searchGen;
    const q = query;
    const local = runLocal(q);
    if (!q.trim()) return;
    const fire = () => {
      if (debounce) clearTimeout(debounce);
      debounce = null;
      flushRemote = null;
      void runRemote(q, gen, local < LOCAL_HIT_SERVER_THRESHOLD);
    };
    flushRemote = fire;
    debounce = setTimeout(fire, 120);
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

  // ⌘ on Mac, Ctrl elsewhere, for the footer hint.
  const isMac =
    typeof navigator !== "undefined" && /Mac|iP(hone|ad|od)/.test(navigator.platform);

  /** Routes are hash paths, so a new tab is this page with another hash. */
  function openInNewTab(path: string) {
    window.open(`${location.pathname}${location.search}#${path}`, "_blank", "noopener");
  }

  function pickItem(it: FlatItem, newTab = false) {
    if (it.t === "nav") {
      hide();
      if (newTab) openInNewTab(it.r.route);
      else navigate(it.r.route);
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
      cursorMoved = true;
      selectedIdx = Math.min(selectedIdx + 1, flatItems.length - 1);
      scrollSelectedIntoView();
    } else if (e.key === "ArrowUp") {
      e.preventDefault();
      cursorMoved = true;
      selectedIdx = Math.max(selectedIdx - 1, 0);
      scrollSelectedIntoView();
    } else if (e.key === "Enter") {
      e.preventDefault();
      const newTab = e.metaKey || e.ctrlKey;
      if (shouldAwaitReference()) {
        enterWhenSettled = { newTab };
        flushRemote?.();
        return;
      }
      const it = flatItems[selectedIdx];
      if (it) pickItem(it, newTab);
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

<svelte:window onkeydown={onWindowKeydown} bind:innerWidth />

{#if open}
  <!-- svelte-ignore a11y_no_static_element_interactions a11y_click_events_have_key_events -->
  <div
    class="fixed inset-0 z-[100] bg-black/25 flex items-start justify-center
           sm:pt-[14dvh] sm:px-4"
    onclick={hide}
  >
    <!-- svelte-ignore a11y_no_static_element_interactions a11y_click_events_have_key_events -->
    <!-- LIF-227: full-screen on phones rather than a bottom sheet. The sheet
         shape is wrong for anything whose first act is to focus a text field:
         `position: fixed` is anchored to the layout viewport, so the software
         keyboard slides straight over a bottom-anchored panel and hides the
         results the user is typing to filter. Taking the whole screen puts
         the input at the top, above the keyboard, with the results between
         them — which is also what every native search surface does. -->
    <div
      data-command-palette
      class="command-palette w-full h-full flex flex-col bg-[var(--surface)]
             border-[var(--border)] shadow-[0_16px_48px_rgba(0,0,0,0.28)] overflow-hidden
             sm:h-auto sm:max-w-[640px] sm:border sm:rounded-xl"
      onclick={(e) => e.stopPropagation()}
    >
      <!-- Input row -->
      <div
        class="shrink-0 flex items-center gap-2.5 px-4 py-3 border-b border-[var(--border)]
               pt-[max(0.75rem,env(safe-area-inset-top))] sm:pt-3"
      >
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
          class="flex-1 bg-transparent border-0 outline-none text-body-lg
                 text-[var(--text)] placeholder:text-[var(--text-faint)]"
          placeholder={mode.type === "prompt"
            ? (mode.action.prompt?.placeholder ?? "Type a value…")
            : mode.type === "submenu"
              ? "Filter…"
              : "Search or jump to…"}
          oninput={onInput}
          onkeydown={onInputKeydown}
        />
        <!-- The `esc` hint means nothing without a keyboard, so on phones it
             is swapped for a real close control (LIF-227). -->
        <kbd
          class="hidden sm:block px-1.5 py-0.5 rounded border border-[var(--border)]
                 bg-[var(--bg-subtle)] text-[var(--text-faint)]
                 font-mono text-micro leading-none shrink-0"
        >
          esc
        </kbd>
        <button
          class="sm:hidden size-11 -mr-2 shrink-0 grid place-items-center rounded-lg
                 text-[var(--text-muted)] active:bg-[var(--bg-subtle)] transition-colors"
          aria-label="Close search"
          onclick={hide}
        >
          <X size={20} />
        </button>
      </div>

      <!-- Results -->
      {#if mode.type === "prompt"}
        <p class="px-4 py-3 text-caption text-[var(--text-faint)]">
          Enter to save · Esc to cancel
        </p>
      {:else}
      <div
        class="flex-1 min-h-0 overflow-y-auto overscroll-contain py-1
               pb-[env(safe-area-inset-bottom)] sm:flex-none sm:pb-1 sm:max-h-[420px]"
        role="listbox"
        aria-label="Results"
        bind:this={listEl}
      >
        {#if mode.type === "root" && !query}
          <!-- Syntax tips live here rather than in the placeholder: they are
               only useful before the first keystroke. -->
          <p
            data-palette-hint
            class="px-4 pt-1.5 pb-1 text-caption text-[var(--text-faint)] truncate"
          >
            <code class="palette-code">34</code>{#if !narrow} or <code class="palette-code">OMN-156</code>{/if}
            jumps to an issue, <code class="palette-code">doc 3</code> to a page
          </p>
        {/if}
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
              class="palette-row mx-1.5 w-[calc(100%-0.75rem)] h-9 flex items-center gap-2.5 px-2.5
                     rounded-md text-left"
              role="option"
              aria-selected={i === selectedIdx}
              data-flat-idx={i}
              data-kind="child"
              onclick={() => pickItem({ t: "child", c })}
              onmouseenter={() => { selectedIdx = i; }}
            >
              <span class="size-4 grid place-items-center shrink-0">
                {#if c.status !== undefined}
                  <StatusIcon status={c.status} size={14} />
                {:else if c.priority !== undefined}
                  <PriorityIcon priority={c.priority} size={14} />
                {:else if c.color}
                  <span
                    class="size-2.5 rounded-full"
                    style="background: {safeLabelColor(c.color)}"
                  ></span>
                {/if}
              </span>
              <span class="flex-1 min-w-0 text-body text-[var(--text)] capitalize truncate">
                {c.title}
              </span>
              {#if c.hint}
                <span class="text-caption text-[var(--text-faint)] shrink-0">
                  {c.hint}
                </span>
              {/if}
            </button>
          {/each}
        {:else}
          <!-- Context actions first: on a detail page they're the most
               likely intent. -->
          {#if actionHits.length > 0}
            <div class="palette-section">Actions</div>
            {#each actionHits as a, i (a.id)}
              <button
                class="palette-row mx-1.5 w-[calc(100%-0.75rem)] h-9 flex items-center gap-2.5 px-2.5
                       rounded-md text-left"
                role="option"
                aria-selected={i === selectedIdx}
                data-flat-idx={i}
                data-kind="action"
                onclick={() => pickItem({ t: "action", a })}
                onmouseenter={() => { selectedIdx = i; }}
              >
                <span class="size-4 grid place-items-center shrink-0 text-[var(--text-faint)]">
                  <Zap size={14} />
                </span>
                <span class="flex-1 min-w-0 text-body text-[var(--text)] truncate">
                  {a.title}
                </span>
                {#if a.hint}
                  <span class="max-w-[12rem] truncate text-caption text-[var(--text-faint)] capitalize shrink-0">
                    {a.hint}
                  </span>
                {/if}
                {#if a.children}
                  <ChevronRight size={14} class="shrink-0 text-[var(--text-faint)]" />
                {/if}
              </button>
            {/each}
          {/if}
          {#each grouped as group (group.label)}
            <div class="palette-section">{group.label}</div>
            {#each group.entries as { r, flatIdx } (r.route + (r.identifier ?? r.title))}
              <button
                class="palette-row mx-1.5 w-[calc(100%-0.75rem)] h-9 flex items-center gap-2.5 px-2.5
                       rounded-md text-left"
                role="option"
                aria-selected={flatIdx === selectedIdx}
                data-flat-idx={flatIdx}
                data-kind={r.kind}
                onclick={(e) => pickItem({ t: "nav", r }, e.metaKey || e.ctrlKey)}
                onmousedown={(e) => { if (e.button === 1) e.preventDefault(); }}
                onauxclick={(e) => {
                  if (e.button !== 1) return;
                  e.preventDefault();
                  pickItem({ t: "nav", r }, true);
                }}
                onmouseenter={() => { selectedIdx = flatIdx; }}
              >
                <!-- Kind icon (project/module emoji wins when set) -->
                <span class="size-4 grid place-items-center shrink-0 text-[var(--text-faint)]">
                  {#if r.emoji}
                    <ProjectIcon value={r.emoji} size={15} />
                  {:else if r.kind === "issue" && r.status}
                    <StatusIcon status={r.status} size={14} />
                  {:else if r.kind === "issue"}
                    <CircleDot size={14} />
                  {:else if r.kind === "page"}
                    <FileText size={14} />
                  {:else if r.kind === "plan"}
                    <ListChecks size={14} />
                  {:else if r.kind === "module"}
                    <Layers size={14} />
                  {:else if r.kind === "folder"}
                    <FolderClosed size={14} />
                  {:else}
                    <Box size={14} />
                  {/if}
                </span>
                {#if r.identifier && r.kind !== "project"}
                  <span
                    class="shrink-0 min-w-[4.5rem] font-mono text-caption text-[var(--text-faint)]"
                  >
                    {r.identifier}
                  </span>
                {/if}
                <span
                  class="min-w-0 truncate text-body text-[var(--text)]
                         {r.snippet ? 'shrink' : 'flex-1'}"
                >
                  {r.title}
                </span>
                {#if r.snippet}
                  <span class="flex-1 min-w-0 truncate text-caption text-[var(--text-faint)]">
                    {#each snippetSegments(r.snippet) as segment}
                      {#if segment.highlighted}
                        <span class="font-medium text-[var(--text-muted)]">{segment.text}</span>
                      {:else}
                        {segment.text}
                      {/if}
                    {/each}
                  </span>
                {/if}
                {#if r.kind === "project" && r.identifier}
                  <span class="shrink-0 font-mono text-caption text-[var(--text-faint)]">
                    {r.identifier}
                  </span>
                {:else if r.project}
                  <span
                    class="hidden sm:block shrink-0 max-w-[9rem] truncate text-caption
                           text-[var(--text-faint)]"
                  >
                    {r.project}
                  </span>
                {/if}
                {#if r.priority && r.priority !== "none"}
                  <span class="shrink-0 grid place-items-center" title="{r.priority} priority">
                    <PriorityIcon priority={r.priority} size={13} />
                  </span>
                {/if}
              </button>
            {/each}
          {/each}
        {/if}
      </div>
      {#if mode.type === "root" && flatItems.length > 0}
        <!-- Keyboard legend; phones have no keyboard to hint at. -->
        <div
          class="hidden sm:flex shrink-0 items-center gap-4 px-4 py-1.5 border-t border-[var(--border)]
                 text-micro text-[var(--text-faint)]"
        >
          <span><kbd class="font-mono">↑↓</kbd> move</span>
          <span><kbd class="font-mono">↵</kbd> open</span>
          <span><kbd class="font-mono">{isMac ? "⌘" : "Ctrl"}↵</kbd> new tab</span>
        </div>
      {/if}
      {/if}
    </div>
  </div>
{/if}

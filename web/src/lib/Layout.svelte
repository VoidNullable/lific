<script lang="ts">
  import {
    me,
    clearSession,
    listProjects,
    reorderProjects,
    type AuthUser,
    type Project,
  } from "./api";
  import ProjectIcon from "./ProjectIcon.svelte";
  import CommandPalette from "./CommandPalette.svelte";
  import { dndzone, type DndEvent } from "svelte-dnd-action";
  import { flip } from "svelte/animate";
  import { getPreference, setPreference, resolveTheme, type ThemePreference } from "./theme";
  import { Settings, List, LayoutGrid, FileText, Plus, Layers, History, ListChecks, LayoutDashboard, Search, ChevronRight, Sun, Moon, Monitor, Menu, X } from "lucide-svelte";
  import { setContext } from "svelte";

  // Ref to the command palette so the sidebar's "Jump to…" affordance can
  // summon it (LIF-192).
  let palette = $state<{ openPalette: () => void } | null>(null);

  // LIF-223: below md the sidebar is an off-canvas drawer. This tracks its
  // open state; it's meaningless at md+ (the sidebar is statically docked).
  let drawerOpen = $state(false);
  function closeDrawer() {
    drawerOpen = false;
  }

  // Escape dismisses the drawer. Registered as a window listener via effect
  // because <svelte:window> may only appear at the component's top level, and
  // our markup is gated behind {#if user}.
  $effect(() => {
    function onKey(e: KeyboardEvent) {
      if (e.key === "Escape" && drawerOpen) closeDrawer();
    }
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  });

  // Compact icon-only theme cycle for the footer (full Light/Dark/System
  // control lives in Settings → Appearance).
  let themePref = $state<ThemePreference>(getPreference());
  let themeResolved = $derived(resolveTheme(themePref));
  function cycleTheme() {
    const order: ThemePreference[] = ["light", "dark", "system"];
    themePref = order[(order.indexOf(themePref) + 1) % order.length];
    setPreference(themePref);
  }

  let {
    navigate,
    route,
    children,
    onProjectChange = $bindable(),
  }: {
    navigate: (path: string) => void;
    route: string;
    children: import("svelte").Snippet;
    onProjectChange?: () => void;
  } = $props();

  // Routes register their topbar content here via getContext("lific:topbar").
  // Layout persists across route changes (mounted once in App), so this
  // avoids the sidebar/user/projects re-fetch flicker we'd get if each
  // route owned its own Layout instance.
  let topbarSnippet = $state<import("svelte").Snippet | undefined>(undefined);
  setContext("lific:topbar", {
    set: (s: import("svelte").Snippet | undefined) => {
      topbarSnippet = s;
    },
  });

  // Routes register context-aware command-palette actions here (same
  // lifecycle pattern as the topbar): set on mount, clear on unmount.
  let paletteActions = $state<import("./palette").PaletteAction[]>([]);
  setContext("lific:palette", {
    set: (a: import("./palette").PaletteAction[] | undefined) => {
      paletteActions = a ?? [];
    },
  });

  // Expose refreshProjects to parent so it can pass it to child routes
  $effect(() => {
    onProjectChange = refreshProjects;
  });

  let user = $state<AuthUser | null>(null);
  let projects = $state<Project[]>([]);
  let loading = $state(true);

  // Load user once on mount
  $effect(() => {
    loadUser();
  });

  // Re-fetch projects whenever route changes (catches new/deleted projects).
  // Also dismiss the mobile drawer on navigation so it never lingers over the
  // newly-loaded route (LIF-223).
  $effect(() => {
    route; // track route changes
    refreshProjects();
    closeDrawer();
  });

  async function loadUser() {
    const res = await me();
    if (res.ok) {
      user = res.data;
    } else {
      clearSession();
      navigate("/login");
      return;
    }
    await refreshProjects();
    loading = false;
  }

  async function refreshProjects() {
    // LIF-233: never swap the projects array out from under an in-flight drag —
    // svelte-dnd-action owns it during the consider/finalize lifecycle, and a
    // route-change refresh landing mid-drag would corrupt the zone. The
    // finalize handler re-syncs from the server response once the drop settles.
    if (dragActive) return;
    const res = await listProjects();
    if (res.ok) {
      projects = res.data;
    }
  }

  // ── LIF-233: drag-to-reorder projects in the sidebar ────────
  // The dndzone owns `projects` during a drag. We veto auto-refresh while
  // dragActive, then persist the new order on finalize (server reindexes
  // sort_order and returns the canonical list).
  let dragActive = $state(false);
  const FLIP_MS = 150;

  function handleProjectConsider(e: CustomEvent<DndEvent<Project>>) {
    dragActive = true;
    projects = e.detail.items;
  }

  async function handleProjectFinalize(e: CustomEvent<DndEvent<Project>>) {
    projects = e.detail.items;
    const ids = projects.map((p) => p.id);
    const res = await reorderProjects(ids);
    if (res.ok) {
      projects = res.data;
    } else {
      // Persist failed — re-sync from server to undo the optimistic order.
      const fresh = await listProjects();
      if (fresh.ok) projects = fresh.data;
    }
    dragActive = false;
  }

  function initials(name: string): string {
    return name
      .split(/[\s_-]+/)
      .slice(0, 2)
      .map((w) => w[0]?.toUpperCase() ?? "")
      .join("");
  }

  function isActive(path: string): boolean {
    return route === path || route.startsWith(path + "/");
  }

  function projectFromRoute(): string | null {
    // Routes like /LIF/issues or /LIF/board
    const match = route.match(/^\/([A-Z][A-Z0-9_-]*)\//);
    return match ? match[1] : null;
  }

  let activeProject = $derived(projectFromRoute());
</script>

{#if loading}
  <div class="min-h-dvh flex items-center justify-center">
    <div
      class="size-6 rounded-full border-2 border-[var(--border)]
             border-t-[var(--accent)] animate-spin"
    ></div>
  </div>
{:else if user}
  <!-- L-shaped chrome (sidebar + topbar share --chrome, no internal seams).
       The chrome floats above the recessed content panel; --chrome is its
       own token, distinct from --surface (which is reserved for cards
       INSIDE the content), so in-content elements never merge with the
       chrome surrounding them. -->
  <div class="h-dvh flex overflow-hidden bg-[var(--chrome)]">
    <!-- Mobile drawer backdrop. Only rendered below md while the drawer is
         open; tapping it dismisses the drawer (LIF-223). -->
    {#if drawerOpen}
      <button
        class="md:hidden fixed inset-0 z-40 bg-black/40 backdrop-blur-[1px]"
        aria-label="Close menu"
        onclick={closeDrawer}
      ></button>
    {/if}

    <!-- ── SIDEBAR (LIF-192 redesign) ──────────────────────────
         Below md it's a fixed off-canvas drawer that slides in over the
         backdrop; at md+ it docks statically into the flex row as before
         (LIF-223). -->
    <aside
      class="w-[230px] shrink-0 flex flex-col bg-[var(--chrome)] select-none
             fixed inset-y-0 left-0 z-50 transition-transform duration-200 ease-out
             {drawerOpen ? 'translate-x-0 shadow-2xl' : '-translate-x-full'}
             md:static md:z-auto md:translate-x-0 md:shadow-none md:transition-none"
    >
      <!-- Brand header -->
      <div class="px-3 pt-3 pb-2 flex items-center gap-1.5">
        <a
          href="https://github.com/VoidNullable/lific"
          target="_blank"
          rel="noopener noreferrer"
          title="View Lific on GitHub"
          class="group flex flex-1 min-w-0 items-center gap-2.5 px-1 py-1 rounded-lg hover:bg-[var(--bg-subtle)] transition-colors"
        >
          <img src="/logo.webp" alt="" width="26" height="26" class="rounded-md shrink-0" />
          <span class="font-display text-heading tracking-tight text-[var(--text)] leading-none flex-1">
            Lific
          </span>
          <span
            class="font-mono text-micro tracking-tight text-[var(--text-faint)]
                   px-1.5 py-0.5 rounded-md bg-[var(--bg-subtle)]
                   group-hover:bg-[var(--surface)] transition-colors"
          >
            v{__APP_VERSION__}
          </span>
        </a>
        <!-- Drawer close affordance (mobile only). -->
        <button
          class="md:hidden size-9 shrink-0 grid place-items-center rounded-md
                 text-[var(--text-muted)] hover:text-[var(--text)] hover:bg-[var(--bg-subtle)] transition-colors"
          aria-label="Close menu"
          onclick={closeDrawer}
        >
          <X size={18} />
        </button>
      </div>

      <!-- Jump-to / command palette trigger -->
      <div class="px-3 pb-2">
        <button
          class="w-full h-8 flex items-center gap-2 px-2.5 rounded-md
                 bg-[var(--bg)] shadow-[inset_0_1px_2px_rgba(0,0,0,0.08)]
                 text-[var(--text-muted)] hover:text-[var(--text)] transition-colors"
          onclick={() => palette?.openPalette()}
        >
          <Search size={14} class="shrink-0" />
          <span class="flex-1 text-left text-body-sm">Jump to…</span>
          <kbd class="font-mono text-micro leading-none text-[var(--text-faint)]
                      border border-[var(--border)] rounded px-1 py-0.5">⌘K</kbd>
        </button>
      </div>

      <!-- Navigation -->
      <nav class="flex-1 px-2 py-1 overflow-y-auto">
        {#if projects.length > 0}
          <div class="flex items-center justify-between px-2 pt-1.5 pb-1">
            <span class="text-micro font-semibold uppercase tracking-widest text-[var(--text-faint)]">
              Projects
            </span>
            <button
              class="size-5 flex items-center justify-center rounded
                     text-[var(--text-faint)] hover:text-[var(--accent)]
                     hover:bg-[var(--bg-subtle)] transition-colors"
              title="New project"
              onclick={() => navigate("/projects/new")}
            >
              <Plus size={13} />
            </button>
          </div>

          <!-- LIF-233: drag-to-reorder zone. Each project is a SINGLE direct
               child of the zone (pill + its sub-nav wrapped together), so
               svelte-dnd-action's one-item-per-child model stays 1:1 — the
               active project's expanded sub-nav must NOT become its own
               draggable item. The header/+button above sit OUTSIDE the zone. -->
          <div
            use:dndzone={{
              items: projects,
              flipDurationMs: FLIP_MS,
              type: "lific-projects",
              dropTargetStyle: {},
              dragDisabled: projects.length < 2,
            }}
            onconsider={handleProjectConsider}
            onfinalize={handleProjectFinalize}
          >
          {#each projects as project (project.id)}
            {@const isProjectActive = activeProject === project.identifier}
            <!-- One draggable item per project. animate:flip gives the reorder
                 its slide; the wrapper holds both the pill and (when active)
                 the sub-nav so they move as a unit. -->
            <div animate:flip={{ duration: FLIP_MS }}>
            <!-- Project pill. Active = elevated surface + chevron down; the
                 click navigates into the project, which makes it active and
                 expands its sub-nav (collapsed-until-focused UX preserved). -->
            <button
              class="group w-full flex items-center gap-1.5 pl-1.5 pr-2 py-1.5 rounded-md
                     text-left text-body-sm transition-all
                     {isProjectActive
                ? 'text-[var(--text)] bg-[var(--bg-subtle)] font-medium'
                : 'text-[var(--text-muted)] hover:text-[var(--text)] hover:bg-[var(--bg-subtle)]'}"
              onclick={() => navigate(`/${project.identifier}/overview`)}
            >
              <ChevronRight
                size={13}
                class="shrink-0 transition-transform
                       {isProjectActive ? 'rotate-90 text-[var(--text-muted)]' : 'text-[var(--text-faint)] group-hover:text-[var(--text-muted)]'}"
              />
              {#if project.emoji}
                <span class="size-5 flex items-center justify-center shrink-0">
                  <ProjectIcon value={project.emoji} size={16} />
                </span>
              {:else}
                <span
                  class="size-5 rounded-md border border-[var(--border)] bg-[var(--bg-subtle)]
                         flex items-center justify-center text-micro font-semibold
                         tracking-tight shrink-0
                         {isProjectActive ? 'text-[var(--text)]' : 'text-[var(--text-muted)]'}"
                >
                  {project.identifier.slice(0, 2)}
                </span>
              {/if}
              <span class="truncate flex-1">{project.name}</span>
            </button>

            {#if isProjectActive}
              <!-- Sub-nav: indented under the project with a vertical guide
                   line, matching the tree language used in Pages. -->
              <div class="ml-[1.125rem] pl-2.5 mt-0.5 mb-1.5 border-l border-[var(--border)] flex flex-col gap-px">
                {#snippet subItem(href: string, label: string, Icon: typeof List)}
                  {@const active = isActive(href)}
                  <button
                    class="w-full flex items-center gap-2 px-2 py-1 rounded-md
                           text-left text-body-sm transition-colors
                           {active
                      ? 'text-[var(--text)] bg-[var(--bg-subtle)] font-medium'
                      : 'text-[var(--text-muted)] hover:text-[var(--text)] hover:bg-[var(--bg-subtle)]'}"
                    onclick={() => navigate(href)}
                  >
                    <Icon size={14} class="shrink-0 {active ? 'text-[var(--accent)]' : ''}" />
                    {label}
                  </button>
                {/snippet}
                {@render subItem(`/${project.identifier}/overview`, "Overview", LayoutDashboard)}
                {@render subItem(`/${project.identifier}/issues`, "Issues", List)}
                {@render subItem(`/${project.identifier}/board`, "Board", LayoutGrid)}
                {@render subItem(`/${project.identifier}/modules`, "Modules", Layers)}
                {@render subItem(`/${project.identifier}/pages`, "Pages", FileText)}
                {@render subItem(`/${project.identifier}/plans`, "Plans", ListChecks)}
                {@render subItem(`/${project.identifier}/activity`, "Activity", History)}
              </div>
            {/if}
            </div>
          {/each}
          </div>
        {:else}
          <div class="px-3 py-6">
            <p class="text-body-sm text-[var(--text-faint)] mb-2">No projects yet.</p>
            <button
              class="text-body-sm text-[var(--accent)] hover:underline"
              onclick={() => navigate("/projects/new")}
            >
              Create a project
            </button>
          </div>
        {/if}
      </nav>

      <!-- Footer: the user identity IS the Settings entry (logout now lives
           inside Settings → Security). A compact theme toggle sits beside it. -->
      <div class="p-2 flex items-center gap-1">
        <button
          class="flex-1 min-w-0 flex items-center gap-2.5 px-2 py-1.5 rounded-md text-left transition-colors
                 {isActive('/settings')
            ? 'bg-[var(--bg-subtle)]'
            : 'hover:bg-[var(--bg-subtle)]'}"
          onclick={() => navigate("/settings")}
          title="Account settings"
        >
          <div
            class="size-7 rounded-full bg-[var(--accent)] text-[var(--accent-text)]
                   flex items-center justify-center text-micro font-semibold
                   tracking-wide select-none shrink-0"
          >
            {initials(user.display_name || user.username)}
          </div>
          <div class="flex-1 min-w-0">
            <div class="text-body-sm text-[var(--text)] truncate leading-tight">
              {user.display_name || user.username}
            </div>
            <div class="text-micro text-[var(--text-faint)] flex items-center gap-1 leading-tight mt-0.5">
              <Settings size={9} /> Settings
            </div>
          </div>
        </button>
        <button
          class="size-8 shrink-0 grid place-items-center rounded-md
                 text-[var(--text-muted)] hover:text-[var(--text)] hover:bg-[var(--bg-subtle)] transition-colors"
          onclick={cycleTheme}
          title="Theme: {themePref}"
          aria-label="Cycle theme, current: {themePref}"
        >
          {#if themePref === "system"}
            <Monitor size={15} />
          {:else if themeResolved === "dark"}
            <Moon size={15} />
          {:else}
            <Sun size={15} />
          {/if}
        </button>
      </div>
    </aside>

    <!-- Right column: chrome topbar (continuous with sidebar) + inset panel -->
    <div class="flex-1 min-w-0 flex flex-col">
      <!-- Mobile header (below md only): hamburger summons the drawer, since
           the sidebar is off-canvas at this width (LIF-223). -->
      <header
        class="md:hidden shrink-0 flex items-center gap-2 h-12 px-2 bg-[var(--chrome)]"
      >
        <button
          class="size-10 grid place-items-center rounded-md
                 text-[var(--text-muted)] hover:text-[var(--text)] hover:bg-[var(--bg-subtle)] transition-colors"
          aria-label="Open menu"
          aria-expanded={drawerOpen}
          onclick={() => (drawerOpen = true)}
        >
          <Menu size={20} />
        </button>
        <img src="/logo.webp" alt="" width="22" height="22" class="rounded-md shrink-0" />
        <span class="font-display text-heading tracking-tight text-[var(--text)] leading-none">
          Lific
        </span>
      </header>

      <!-- Chrome topbar slot. Routes pass a `topbar` snippet for breadcrumb,
           filters, search, etc. Background matches the sidebar so the L is
           visually seamless. -->
      {#if topbarSnippet}
        <!-- The topbar deliberately uses muted text/icon colors so it
             reads as quieter than the content panel below. We avoid
             `opacity` for the dimming effect because it creates a CSS
             stacking context that traps absolutely-positioned dropdowns
             (filters, display, help popovers) BEHIND the content panel. -->
        <div class="shrink-0 flex items-stretch min-h-0 bg-[var(--chrome)]">
          {@render topbarSnippet()}
        </div>
      {/if}

      <!-- Inset content panel. Recessed (--bg is darker than --chrome)
           with a soft inset shadow on its top + left edges, simulating
           the chrome casting down onto the content. No border — the
           shadow + color step define the boundary, so the chrome reads
           as physically floating above. -->
      <!-- Recessed content panel with cast-shadow overlays.

           Inset box-shadows don't work here: child elements inside main
           (sticky group headers, dropdowns, the inline-create row) paint
           their own opaque backgrounds, which render ON TOP of the
           parent's inset shadow and erase it along the top edge.

           Instead, we use a relative wrapper with rounded-tl + overflow
           hidden, then layer two pointer-events-none gradient overlays
           ABOVE main via z-index. The chrome's cast shadow now renders
           on top of every child, indelibly. -->
      <div class="relative flex-1 min-w-0 overflow-hidden md:rounded-tl-xl">
        <main class="absolute inset-0 bg-[var(--bg)] overflow-y-auto">
          {@render children()}
        </main>
        <!-- Top edge: TL → TR. -->
        <div
          class="pointer-events-none absolute top-0 left-0 right-0 h-6 z-10
                 bg-gradient-to-b from-[var(--shadow-recess)] to-transparent"
        ></div>
        <!-- Left edge: TL → BL. Only meaningful at md+ where the sidebar is
             docked to cast the shadow; on mobile there's nothing to its left. -->
        <div
          class="hidden md:block pointer-events-none absolute top-0 left-0 bottom-0 w-6 z-10
                 bg-gradient-to-r from-[var(--shadow-recess)] to-transparent"
        ></div>
      </div>
    </div>
  </div>

  <!-- LIF-159: cmd+k / ctrl+p jump-anywhere. Mounted here (once, above
       routes) so its session catalog cache survives navigation. -->
  <CommandPalette bind:this={palette} {navigate} actions={paletteActions} />
{/if}

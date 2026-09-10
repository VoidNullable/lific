<script lang="ts">
  // LIF-471: the shell for the public view.
  //
  // The same L-shaped chrome as `Layout` (sidebar + topbar on --chrome, the
  // recessed --bg content panel with its cast shadows) so a published project
  // reads as the same product a signed-in user sees. What is missing is
  // everything that assumes an account: Home, the project list, groups,
  // drag-to-reorder, recents, the palette, the context menu, the peek panels,
  // the user footer. The sidebar holds exactly one project with two entries,
  // Issues and Pages, and the footer holds a theme toggle and a sign-in link.
  //
  // Routes register their topbar through the same `lific:topbar` context
  // `Layout` provides, so IssueList and PageList mount here unchanged.
  import { listProjects, type Project } from "./api";
  import ProjectIcon from "./ProjectIcon.svelte";
  import ErrorState from "./ErrorState.svelte";
  import { getPreference, setPreference, resolveTheme, type ThemePreference } from "./theme";
  import {
    List,
    FileText,
    ChevronRight,
    Sun,
    Moon,
    Monitor,
    Menu,
    X,
    LogIn,
    Globe,
    PanelLeftClose,
    PanelLeftOpen,
  } from "lucide-svelte";
  import { setContext } from "svelte";
  import { loadSidebarWidth, loadSidebarCollapsed, saveSidebarCollapsed } from "./sidebarWidth";

  let {
    navigate,
    route,
    projectIdentifier,
    children,
  }: {
    /** Already scoped: `navigate("/LIF/issues")` lands on `/public/LIF/issues`. */
    navigate: (path: string) => void;
    /** The unscoped route (`/LIF/issues`), for the active-state highlights. */
    route: string;
    projectIdentifier: string;
    children: import("svelte").Snippet;
  } = $props();

  let topbarSnippet = $state<import("svelte").Snippet | undefined>(undefined);
  setContext("lific:topbar", {
    set: (s: import("svelte").Snippet | undefined) => {
      topbarSnippet = s;
    },
  });
  // DocumentDetail registers palette actions; there is no palette here, so
  // the registrations go nowhere.
  setContext("lific:palette", { set: () => {} });

  let project = $state<Project | null>(null);
  let projectError = $state(false);

  // The one project this shell shows. `listProjects` mirrors onto the
  // published project in public scope, so the same call the private sidebar
  // makes answers with a one-element list here.
  $effect(() => {
    const identifier = projectIdentifier;
    project = null;
    projectError = false;
    void listProjects().then((res) => {
      if (identifier !== projectIdentifier) return;
      if (res.ok) {
        project =
          res.data.find((p) => p.identifier.toLowerCase() === identifier.toLowerCase()) ?? null;
        projectError = project === null;
      } else {
        projectError = true;
      }
    });
  });

  let sidebarWidth = $state(loadSidebarWidth());
  let sidebarCollapsed = $state(loadSidebarCollapsed());
  function toggleSidebar() {
    sidebarCollapsed = !sidebarCollapsed;
    saveSidebarCollapsed(sidebarCollapsed);
  }

  let navOpen = $state(false);
  $effect(() => {
    route;
    navOpen = false;
  });

  let themePref = $state<ThemePreference>(getPreference());
  let themeResolved = $derived(resolveTheme(themePref));
  function cycleTheme() {
    const order: ThemePreference[] = ["light", "dark", "system"];
    themePref = order[(order.indexOf(themePref) + 1) % order.length];
    setPreference(themePref);
  }

  function isActive(path: string): boolean {
    return route === path || route.startsWith(path + "/");
  }
  // The board is the issue list in another layout; the Issues entry stays
  // lit while it is showing.
  let issuesActive = $derived(
    isActive(`/${projectIdentifier}/issues`) || isActive(`/${projectIdentifier}/board`),
  );
  let pagesActive = $derived(isActive(`/${projectIdentifier}/pages`));

  let mobileSection = $derived(issuesActive ? "Issues" : pagesActive ? "Pages" : null);

  const entries = $derived([
    { href: `/${projectIdentifier}/issues`, label: "Issues", Icon: List, active: issuesActive },
    { href: `/${projectIdentifier}/pages`, label: "Pages", Icon: FileText, active: pagesActive },
  ]);
</script>

<div class="h-dvh flex overflow-hidden bg-[var(--chrome)]">
  <aside
    class="{sidebarCollapsed ? 'hidden' : 'hidden md:flex'} w-[var(--sidebar-w)]
           shrink-0 relative flex-col bg-[var(--chrome)] select-none"
    style={`--sidebar-w: ${sidebarWidth}px`}
  >
    <div class="px-3 pt-3 pb-2 flex items-center gap-1.5">
      <a
        href="https://lific.dev"
        target="_blank"
        rel="noopener noreferrer"
        title="Lific"
        class="group flex flex-1 min-w-0 items-center gap-2.5 px-1 py-1 rounded-lg hover:bg-[var(--bg-subtle)] transition-colors"
      >
        <img src="/logo.webp" alt="" width="26" height="26" class="rounded-md shrink-0" />
        <span class="font-display text-heading tracking-tight text-[var(--text)] leading-none flex-1">
          Lific
        </span>
        <span
          class="inline-flex items-center gap-1 font-mono text-micro tracking-tight text-[var(--text-faint)]
                 px-1.5 py-0.5 rounded-md bg-[var(--bg-subtle)]
                 group-hover:bg-[var(--surface)] transition-colors"
          title="Anyone with the link can read this project"
        >
          <Globe size={10} /> public
        </span>
      </a>
      <button
        class="size-7 shrink-0 grid place-items-center rounded-md
               text-[var(--text-faint)] hover:text-[var(--text)]
               hover:bg-[var(--bg-subtle)] transition-colors"
        onclick={toggleSidebar}
        title="Collapse sidebar"
        aria-label="Collapse sidebar"
      >
        <PanelLeftClose size={15} />
      </button>
    </div>

    <nav class="flex-1 px-2 py-1 overflow-y-auto">
      <div class="flex items-center justify-between px-2 pt-1.5 pb-1">
        <span class="text-micro font-semibold uppercase tracking-widest text-[var(--text-faint)]">
          Project
        </span>
      </div>

      {#if project}
        <!-- The project pill, open and not collapsible: there is nowhere else
             in this shell to go. -->
        <div
          class="w-full flex items-center gap-1.5 pl-1.5 pr-2 py-1.5 rounded-md
                 text-left text-body-sm text-[var(--text)] font-medium"
        >
          <ChevronRight size={13} class="shrink-0 rotate-90 text-[var(--text-muted)]" />
          {#if project.emoji}
            <span class="size-5 flex items-center justify-center shrink-0">
              <ProjectIcon value={project.emoji} size={16} />
            </span>
          {:else}
            <span
              class="size-5 rounded-md border border-[var(--border)] bg-[var(--bg-subtle)]
                     flex items-center justify-center text-micro font-semibold
                     tracking-tight shrink-0 text-[var(--text)]"
            >
              {project.identifier.slice(0, 2)}
            </span>
          {/if}
          <span class="truncate flex-1">{project.name}</span>
        </div>
        <div class="ml-[1.125rem] pl-2.5 mt-0.5 mb-1.5 border-l border-[var(--border)] flex flex-col gap-px">
          {#each entries as entry (entry.href)}
            <button
              class="w-full flex items-center gap-2 px-2 py-1 rounded-md
                     text-left text-body-sm transition-colors
                     {entry.active
                ? 'text-[var(--text)] bg-[var(--bg-subtle)] font-medium'
                : 'text-[var(--text-muted)] hover:text-[var(--text)] hover:bg-[var(--bg-subtle)]'}"
              onclick={() => navigate(entry.href)}
            >
              <entry.Icon size={14} class="shrink-0 {entry.active ? 'text-[var(--accent)]' : ''}" />
              {entry.label}
            </button>
          {/each}
        </div>
        {#if project.description}
          <p class="px-2.5 pt-2 text-caption text-[var(--text-faint)] leading-snug line-clamp-6">
            {project.description}
          </p>
        {/if}
      {:else if projectError}
        <p class="px-2.5 py-2 text-body-sm text-[var(--text-faint)]">
          This project isn't public.
        </p>
      {:else}
        <div class="px-2.5 py-2 flex flex-col gap-2">
          <div class="h-4 w-2/3 rounded bg-[var(--bg-subtle)] animate-pulse"></div>
          <div class="h-3 w-1/2 rounded bg-[var(--bg-subtle)] animate-pulse"></div>
        </div>
      {/if}
    </nav>

    <div class="p-2 flex items-center gap-1">
      <a
        href="#/login"
        class="flex-1 min-w-0 flex items-center gap-2 px-2 py-1.5 rounded-md text-left
               text-body-sm text-[var(--text-muted)] hover:text-[var(--text)]
               hover:bg-[var(--bg-subtle)] transition-colors"
        title="Sign in to edit or comment"
      >
        <LogIn size={14} class="shrink-0" />
        Sign in
      </a>
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

  <div class="flex-1 min-w-0 flex flex-col">
    <header
      class="md:hidden shrink-0 flex items-center gap-1 h-12 px-1
             pt-[env(safe-area-inset-top)] box-content bg-[var(--chrome)]"
    >
      <button
        class="size-11 shrink-0 grid place-items-center rounded-lg
               text-[var(--text-muted)] active:bg-[var(--bg-subtle)] transition-colors"
        aria-label={navOpen ? "Close navigation" : "Open navigation"}
        aria-expanded={navOpen}
        onclick={() => (navOpen = !navOpen)}
      >
        {#if navOpen}
          <X size={20} />
        {:else}
          <Menu size={20} />
        {/if}
      </button>
      <span class="min-w-0 flex-1 h-11 flex items-center gap-2 px-1.5">
        {#if project?.emoji}
          <span class="size-6 grid place-items-center shrink-0">
            <ProjectIcon value={project.emoji} size={18} />
          </span>
        {:else}
          <img src="/logo.webp" alt="" width="22" height="22" class="rounded-md shrink-0" />
        {/if}
        <span class="min-w-0 flex items-baseline gap-1.5">
          <span class="truncate font-display text-body-lg tracking-tight text-[var(--text)]">
            {project?.name ?? projectIdentifier}
          </span>
          {#if mobileSection}
            <span class="shrink-0 text-body-sm text-[var(--text-faint)]">{mobileSection}</span>
          {/if}
        </span>
      </span>
      <span
        class="mr-2 inline-flex items-center gap-1 font-mono text-micro text-[var(--text-faint)]
               px-1.5 py-0.5 rounded-md bg-[var(--bg-subtle)]"
      >
        <Globe size={10} /> public
      </span>
    </header>

    {#if navOpen}
      <!-- Phone navigation: a short list under the header rather than the
           full-screen drilldown, since there are only two places to go. -->
      <nav class="md:hidden shrink-0 px-2 pb-2 bg-[var(--chrome)] flex flex-col gap-px">
        {#each entries as entry (entry.href)}
          <button
            class="w-full flex items-center gap-2 px-3 py-2.5 rounded-md
                   text-left text-body transition-colors
                   {entry.active
              ? 'text-[var(--text)] bg-[var(--bg-subtle)] font-medium'
              : 'text-[var(--text-muted)] active:bg-[var(--bg-subtle)]'}"
            onclick={() => navigate(entry.href)}
          >
            <entry.Icon size={16} class="shrink-0 {entry.active ? 'text-[var(--accent)]' : ''}" />
            {entry.label}
          </button>
        {/each}
        <div class="flex items-center gap-1 pt-1">
          <a
            href="#/login"
            class="flex-1 flex items-center gap-2 px-3 py-2.5 rounded-md text-body
                   text-[var(--text-muted)] active:bg-[var(--bg-subtle)]"
          >
            <LogIn size={16} class="shrink-0" /> Sign in
          </a>
          <button
            class="size-10 grid place-items-center rounded-md text-[var(--text-muted)]"
            onclick={cycleTheme}
            aria-label="Cycle theme, current: {themePref}"
          >
            {#if themePref === "system"}
              <Monitor size={16} />
            {:else if themeResolved === "dark"}
              <Moon size={16} />
            {:else}
              <Sun size={16} />
            {/if}
          </button>
        </div>
      </nav>
    {/if}

    {#if topbarSnippet || sidebarCollapsed}
      <div class="shrink-0 flex items-stretch min-h-0 bg-[var(--chrome)]">
        {#if sidebarCollapsed}
          <div class="hidden md:flex items-center shrink-0 pl-2 pr-0.5 py-2">
            <button
              class="size-7 grid place-items-center rounded-md
                     text-[var(--text-faint)] hover:text-[var(--text)]
                     hover:bg-[var(--bg-subtle)] transition-colors"
              onclick={toggleSidebar}
              title="Expand sidebar"
              aria-label="Expand sidebar"
            >
              <PanelLeftOpen size={15} />
            </button>
          </div>
        {/if}
        {#if topbarSnippet}
          <div class="flex-1 min-w-0 flex items-stretch">
            {@render topbarSnippet()}
          </div>
        {/if}
      </div>
    {/if}

    <div class="relative flex-1 min-w-0 overflow-hidden md:rounded-tl-xl">
      <main class="absolute inset-0 bg-[var(--bg)] overflow-y-auto">
        {#if projectError}
          <!-- A private project and a missing one answer identically, on
               purpose (see src/api/public.rs), so this says neither. -->
          <ErrorState
            title="This project isn't public"
            message="There is nothing to show here without signing in. If you were sent this link, the project may have been unpublished or the address may be wrong."
          >
            <a
              href="#/login"
              class="text-body-sm font-medium text-[var(--btn-success-text)] bg-[var(--btn-success)]
                     px-3 py-1.5 rounded-md hover:bg-[var(--btn-success-hover)] transition-colors"
            >
              Sign in
            </a>
          </ErrorState>
        {:else}
          {@render children()}
        {/if}
      </main>
      <div
        class="pointer-events-none absolute top-0 left-0 right-0 h-6 z-10
               bg-gradient-to-b from-[var(--shadow-recess)] to-transparent"
      ></div>
      {#if !sidebarCollapsed}
        <div
          class="hidden md:block pointer-events-none absolute top-0 left-0 bottom-0 w-6 z-10
                 bg-gradient-to-r from-[var(--shadow-recess)] to-transparent"
        ></div>
      {/if}
    </div>
  </div>
</div>

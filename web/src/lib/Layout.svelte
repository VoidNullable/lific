<script lang="ts">
  import {
    me,
    logout,
    clearSession,
    listProjects,
    type AuthUser,
    type Project,
  } from "./api";
  import ThemeToggle from "./ThemeToggle.svelte";
  import ProjectIcon from "./ProjectIcon.svelte";
  import { Settings, LogOut, List, FileText, Plus } from "lucide-svelte";

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

  // Re-fetch projects whenever route changes (catches new/deleted projects)
  $effect(() => {
    route; // track route changes
    refreshProjects();
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
    const res = await listProjects();
    if (res.ok) {
      projects = res.data;
    }
  }

  async function handleLogout() {
    await logout();
    clearSession();
    navigate("/login");
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
  <div class="h-dvh flex overflow-hidden">
    <!-- Sidebar -->
    <aside
      class="w-[220px] shrink-0 flex flex-col border-r border-[var(--border)]
             bg-[var(--surface)] select-none overflow-y-auto"
    >
      <!-- Brand -->
      <div class="px-4 py-3 border-b border-[var(--border)]">
        <span class="font-display text-lg tracking-tight text-[var(--text)]">
          Lific
        </span>
      </div>

      <!-- Navigation -->
      <nav class="flex-1 py-2 overflow-y-auto">
        <!-- Projects -->
        {#if projects.length > 0}
          <div class="flex items-center justify-between px-3 pt-2 pb-1">
            <span
              class="text-[0.6875rem] font-semibold uppercase tracking-widest
                     text-[var(--text-faint)]"
            >
              Projects
            </span>
            <button
              class="size-4 flex items-center justify-center rounded
                     text-[var(--text-faint)] hover:text-[var(--accent)]
                     hover:bg-[var(--bg-subtle)] transition-colors"
              title="New project"
              onclick={() => navigate("/projects/new")}
            >
              <Plus size={12} />
            </button>
          </div>
          {#each projects as project (project.id)}
            {@const isProjectActive = activeProject === project.identifier}
            <div>
              <button
                class="w-full flex items-center gap-2 px-3 py-1.5 text-left
                       text-[0.8125rem] rounded-md mx-1 transition-colors
                       {isProjectActive
                  ? 'text-[var(--text)] bg-[var(--bg-subtle)] font-medium'
                  : 'text-[var(--text-muted)] hover:text-[var(--text)] hover:bg-[var(--bg-subtle)]'}"
                style="width: calc(100% - 8px);"
                onclick={() => navigate(`/${project.identifier}/issues`)}
              >
                {#if project.emoji}
                  <span class="size-5 flex items-center justify-center shrink-0">
                    <ProjectIcon value={project.emoji} size={16} />
                  </span>
                {:else}
                  <span
                    class="size-5 rounded bg-[var(--accent)] text-[var(--accent-text)]
                           flex items-center justify-center text-[0.625rem]
                           font-bold shrink-0"
                  >
                    {project.identifier.slice(0, 2)}
                  </span>
                {/if}
                <span class="truncate">{project.name}</span>
              </button>

              <!-- Sub-nav when project is active -->
              {#if isProjectActive}
                <div class="ml-5 mt-0.5 mb-1">
                  <button
                    class="w-full flex items-center gap-2 px-3 py-1 text-left
                           text-[0.8125rem] rounded-md transition-colors
                           {isActive(`/${project.identifier}/issues`)
                      ? 'text-[var(--accent)] font-medium'
                      : 'text-[var(--text-muted)] hover:text-[var(--text)]'}"
                    onclick={() => navigate(`/${project.identifier}/issues`)}
                  >
                    <List size={14} class="shrink-0" />
                    Issues
                  </button>
                  <button
                    class="w-full flex items-center gap-2 px-3 py-1 text-left
                           text-[0.8125rem] rounded-md transition-colors
                           {isActive(`/${project.identifier}/pages`)
                      ? 'text-[var(--accent)] font-medium'
                      : 'text-[var(--text-muted)] hover:text-[var(--text)]'}"
                    onclick={() => navigate(`/${project.identifier}/pages`)}
                  >
                    <FileText size={14} class="shrink-0" />
                    Pages
                  </button>
                  <button
                    class="w-full flex items-center gap-2 px-3 py-1 text-left
                           text-[0.8125rem] rounded-md transition-colors
                           {isActive(`/${project.identifier}/settings`)
                      ? 'text-[var(--accent)] font-medium'
                      : 'text-[var(--text-muted)] hover:text-[var(--text)]'}"
                    onclick={() => navigate(`/${project.identifier}/settings`)}
                  >
                    <Settings size={14} class="shrink-0" />
                    Settings
                  </button>
                </div>
              {/if}
            </div>
          {/each}
        {:else}
          <div class="px-4 py-6">
            <p class="text-[0.8125rem] text-[var(--text-faint)] mb-2">No projects yet.</p>
            <button
              class="text-[0.8125rem] text-[var(--accent)] hover:underline"
              onclick={() => navigate("/projects/new")}
            >
              Create a project
            </button>
          </div>
        {/if}
      </nav>

      <!-- Bottom: settings + user -->
      <div class="border-t border-[var(--border)] p-2 space-y-1">
        <button
          class="w-full flex items-center gap-2 px-3 py-1.5 text-left
                 text-[0.8125rem] rounded-md transition-colors
                 {isActive('/settings')
            ? 'text-[var(--text)] bg-[var(--bg-subtle)] font-medium'
            : 'text-[var(--text-muted)] hover:text-[var(--text)] hover:bg-[var(--bg-subtle)]'}"
          onclick={() => navigate("/settings")}
        >
          <Settings size={16} class="shrink-0" />
          Settings
        </button>

        <div class="flex items-center justify-between px-3 py-1.5">
          <div class="flex items-center gap-2 min-w-0">
            <div
              class="size-6 rounded-full bg-[var(--accent)] text-[var(--accent-text)]
                     flex items-center justify-center text-[0.625rem] font-semibold
                     tracking-wide select-none shrink-0"
              title={user.username}
            >
              {initials(user.display_name || user.username)}
            </div>
            <span
              class="text-[0.8125rem] text-[var(--text-muted)] truncate"
              title={user.username}
            >
              {user.display_name || user.username}
            </span>
          </div>
          <div class="flex items-center gap-1 shrink-0">
            <ThemeToggle />
            <button
              class="text-[0.75rem] text-[var(--text-faint)] px-1.5 py-0.5
                     rounded transition-colors
                     hover:text-[var(--text)] hover:bg-[var(--bg-subtle)]"
              onclick={handleLogout}
              title="Sign out"
            >
              <LogOut size={14} />
            </button>
          </div>
        </div>
      </div>
    </aside>

    <!-- Main content -->
    <main class="flex-1 min-w-0 bg-[var(--bg)] overflow-y-auto">
      {@render children()}
    </main>
  </div>
{/if}

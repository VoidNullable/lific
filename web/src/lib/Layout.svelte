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

  let {
    navigate,
    route,
    children,
  }: {
    navigate: (path: string) => void;
    route: string;
    children: import("svelte").Snippet;
  } = $props();

  let user = $state<AuthUser | null>(null);
  let projects = $state<Project[]>([]);
  let loading = $state(true);

  $effect(() => {
    loadData();
  });

  async function loadData() {
    const [userRes, projRes] = await Promise.all([me(), listProjects()]);
    if (userRes.ok) {
      user = userRes.data;
    } else {
      clearSession();
      navigate("/login");
      return;
    }
    if (projRes.ok) {
      projects = projRes.data;
    }
    loading = false;
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
  <div class="min-h-dvh flex">
    <!-- Sidebar -->
    <aside
      class="w-[220px] shrink-0 flex flex-col border-r border-[var(--border)]
             bg-[var(--surface)] select-none"
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
              <svg class="size-3" viewBox="0 0 16 16" fill="currentColor">
                <path d="M7.25 1a.75.75 0 0 1 .75.75V7h5.25a.75.75 0 0 1 0 1.5H8v5.25a.75.75 0 0 1-1.5 0V8.5H1.25a.75.75 0 0 1 0-1.5H6.5V1.75A.75.75 0 0 1 7.25 1Z"/>
              </svg>
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
                  <span class="text-sm">{project.emoji}</span>
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
                    <!-- List icon -->
                    <svg class="size-3.5 shrink-0" viewBox="0 0 16 16" fill="currentColor">
                      <path d="M2 4a1 1 0 1 1 0-2 1 1 0 0 1 0 2zm3.75-1.5a.75.75 0 0 0 0 1.5h8.5a.75.75 0 0 0 0-1.5h-8.5zm0 5a.75.75 0 0 0 0 1.5h8.5a.75.75 0 0 0 0-1.5h-8.5zm0 5a.75.75 0 0 0 0 1.5h8.5a.75.75 0 0 0 0-1.5h-8.5zM3 8a1 1 0 1 1-2 0 1 1 0 0 1 2 0zm-1 6a1 1 0 1 1 0-2 1 1 0 0 1 0 2z"/>
                    </svg>
                    Issues
                  </button>
                  <button
                    class="w-full flex items-center gap-2 px-3 py-1 text-left
                           text-[0.8125rem] rounded-md transition-colors
                           {isActive(`/${project.identifier}/settings`)
                      ? 'text-[var(--accent)] font-medium'
                      : 'text-[var(--text-muted)] hover:text-[var(--text)]'}"
                    onclick={() => navigate(`/${project.identifier}/settings`)}
                  >
                    <!-- Gear icon -->
                    <svg class="size-3.5 shrink-0" viewBox="0 0 16 16" fill="currentColor">
                      <path fill-rule="evenodd" d="M6.955.9A.75.75 0 0 1 7.68.316l.656.007a.75.75 0 0 1 .723.591l.24 1.108c.333.12.65.273.947.456l1.043-.44a.75.75 0 0 1 .89.243l.398.562a.75.75 0 0 1-.054.906l-.753.72a5.535 5.535 0 0 1 .06 1.062l.752.72a.75.75 0 0 1 .055.906l-.399.562a.75.75 0 0 1-.89.243l-1.042-.44c-.297.183-.614.336-.947.456l-.24 1.109a.75.75 0 0 1-.723.59l-.656.008a.75.75 0 0 1-.726-.584l-.26-1.117a5.503 5.503 0 0 1-.94-.457l-1.05.442a.75.75 0 0 1-.891-.244l-.398-.562a.75.75 0 0 1 .054-.906l.762-.726a5.535 5.535 0 0 1-.06-1.055l-.762-.726a.75.75 0 0 1-.054-.906l.398-.562a.75.75 0 0 1 .89-.244l1.05.443c.293-.183.607-.336.941-.457l.26-1.117A.75.75 0 0 1 6.955.9ZM8 10.5a2.5 2.5 0 1 0 0-5 2.5 2.5 0 0 0 0 5Z" clip-rule="evenodd"/>
                    </svg>
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
          <!-- Gear icon -->
          <svg class="size-4 shrink-0" viewBox="0 0 16 16" fill="currentColor">
            <path fill-rule="evenodd" d="M6.955.9A.75.75 0 0 1 7.68.316l.656.007a.75.75 0 0 1 .723.591l.24 1.108c.333.12.65.273.947.456l1.043-.44a.75.75 0 0 1 .89.243l.398.562a.75.75 0 0 1-.054.906l-.753.72a5.535 5.535 0 0 1 .06 1.062l.752.72a.75.75 0 0 1 .055.906l-.399.562a.75.75 0 0 1-.89.243l-1.042-.44c-.297.183-.614.336-.947.456l-.24 1.109a.75.75 0 0 1-.723.59l-.656.008a.75.75 0 0 1-.726-.584l-.26-1.117a5.503 5.503 0 0 1-.94-.457l-1.05.442a.75.75 0 0 1-.891-.244l-.398-.562a.75.75 0 0 1 .054-.906l.762-.726a5.535 5.535 0 0 1-.06-1.055l-.762-.726a.75.75 0 0 1-.054-.906l.398-.562a.75.75 0 0 1 .89-.244l1.05.443c.293-.183.607-.336.941-.457l.26-1.117A.75.75 0 0 1 6.955.9ZM8 10.5a2.5 2.5 0 1 0 0-5 2.5 2.5 0 0 0 0 5Z" clip-rule="evenodd"/>
          </svg>
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
              <!-- Logout icon -->
              <svg class="size-3.5" viewBox="0 0 16 16" fill="currentColor">
                <path fill-rule="evenodd" d="M2 3.75C2 2.784 2.784 2 3.75 2h3.5a.75.75 0 0 1 0 1.5h-3.5a.25.25 0 0 0-.25.25v8.5c0 .138.112.25.25.25h3.5a.75.75 0 0 1 0 1.5h-3.5A1.75 1.75 0 0 1 2 12.25v-8.5Zm9.22.22a.75.75 0 0 1 1.06 0l2.75 2.75a.75.75 0 0 1 0 1.06l-2.75 2.75a.75.75 0 1 1-1.06-1.06l1.47-1.47H6.75a.75.75 0 0 1 0-1.5h5.94l-1.47-1.47a.75.75 0 0 1 0-1.06Z" clip-rule="evenodd"/>
              </svg>
            </button>
          </div>
        </div>
      </div>
    </aside>

    <!-- Main content -->
    <main class="flex-1 min-w-0 bg-[var(--bg)]">
      {@render children()}
    </main>
  </div>
{/if}

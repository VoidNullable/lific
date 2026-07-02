<script lang="ts">
  // LIF-242: "Views" dropdown for the issue list/board topbar — named
  // filter/group/sort/display presets, personal to each user. Self-
  // contained: owns its own popover open/close (window mousedown +
  // Escape), resolves the numeric project id itself from `projectIdentifier`
  // via `listProjects()` (Topbar only has the string identifier — adding a
  // numeric-id prop would mean editing IssueList.svelte's `<Topbar />` call
  // site, out of this feature's edit scope this session), and hides itself
  // entirely when `/api/me` fails (logged out, OAuth-token-only, or a
  // legacy API key with no resolved user — saved views are inherently
  // per-user, so there's no sensible anonymous fallback).
  import { Bookmark, Star, Pencil, Trash2, Check, Plus, ChevronDown } from "lucide-svelte";
  import { me, listProjects } from "../api";
  import type { IssueListState } from "./state.svelte";
  import {
    buildConfig,
    applyConfig,
    parseConfig,
    configsDiffer,
    fetchViews,
    saveNewView,
    saveViewUpdate,
    removeView,
    shouldAutoApplyDefault,
    getActiveViewId,
    setActiveViewId,
    type SavedView,
    type Layout,
  } from "./views";

  let {
    view,
    projectIdentifier,
    layout,
    navigate,
  }: {
    view: IssueListState;
    projectIdentifier: string;
    layout: Layout;
    navigate: (path: string) => void;
  } = $props();

  // null = still checking; hides the control until resolved either way.
  let authOk = $state<boolean | null>(null);
  let projectId = $state<number | null>(null);
  let views = $state<SavedView[]>([]);
  let viewsLoaded = $state(false);
  let activeViewId = $state<number | null>(null);

  let open = $state(false);
  let mode = $state<"menu" | "create" | "rename">("menu");
  let nameInput = $state("");
  let renameTargetId = $state<number | null>(null);
  let busy = $state(false);
  let formError = $state("");
  let rootEl = $state<HTMLDivElement | undefined>();
  let nameInputEl = $state<HTMLInputElement | undefined>();

  let activeView = $derived(views.find((v) => v.id === activeViewId) ?? null);
  let activeConfig = $derived(activeView ? parseConfig(activeView.config) : null);
  let currentConfig = $derived(buildConfig(view, layout));
  let drifted = $derived(!!activeConfig && configsDiffer(currentConfig, activeConfig));

  // ── Auth check (once) — hides the whole control on failure ──────────
  $effect(() => {
    let cancelled = false;
    (async () => {
      const resp = await me();
      if (!cancelled) authOk = resp.ok;
    })();
    return () => {
      cancelled = true;
    };
  });

  // ── Resolve project id + load views whenever the project changes ────
  $effect(() => {
    const identifier = projectIdentifier;
    if (authOk !== true) return;

    activeViewId = getActiveViewId(identifier);
    projectId = null;
    views = [];
    viewsLoaded = false;
    open = false;
    mode = "menu";

    let cancelled = false;
    (async () => {
      const projectsResp = await listProjects();
      if (cancelled || !projectsResp.ok) return;
      const project = projectsResp.data.find((p) => p.identifier === identifier);
      if (!project || cancelled) return;
      projectId = project.id;
      const viewsResp = await fetchViews(project.id);
      if (cancelled) return;
      if (viewsResp.ok) views = viewsResp.data;
      viewsLoaded = true;
    })();
    return () => {
      cancelled = true;
    };
  });

  // ── Auto-apply the default view once per (project, tab session) ─────
  // Gated on view.hydrated so this never races IssueList.svelte's own
  // hydrate() pass (state.svelte.ts, out of this feature's edit scope) —
  // whichever settles last, this effect re-runs on every dependency change
  // until both are true.
  $effect(() => {
    const identifier = projectIdentifier;
    const hydrated = view.hydrated;
    if (!hydrated || !viewsLoaded || authOk !== true) return;
    if (!shouldAutoApplyDefault(identifier)) return;
    const def = views.find((v) => v.is_default);
    if (def) applyAndMaybeNavigate(def);
  });

  // ── Popover: click-outside + Escape ──────────────────────────────────
  $effect(() => {
    if (!open) return;
    function onDocClick(e: MouseEvent) {
      if (rootEl && !rootEl.contains(e.target as Node)) closeMenu();
    }
    function onKey(e: KeyboardEvent) {
      if (e.key === "Escape") {
        e.stopPropagation();
        closeMenu();
      }
    }
    window.addEventListener("mousedown", onDocClick);
    window.addEventListener("keydown", onKey);
    return () => {
      window.removeEventListener("mousedown", onDocClick);
      window.removeEventListener("keydown", onKey);
    };
  });

  $effect(() => {
    if (mode !== "menu") nameInputEl?.focus();
  });

  function closeMenu() {
    open = false;
    mode = "menu";
    formError = "";
  }

  function applyAndMaybeNavigate(v: SavedView) {
    const config = parseConfig(v.config);
    if (!config) return;
    const targetLayout = applyConfig(view, projectIdentifier, config);
    activeViewId = v.id;
    setActiveViewId(projectIdentifier, v.id);
    if (targetLayout !== layout) {
      navigate(`/${projectIdentifier}/${targetLayout === "board" ? "board" : "issues"}`);
    }
  }

  function applyView(v: SavedView) {
    applyAndMaybeNavigate(v);
    closeMenu();
  }

  async function refresh() {
    if (projectId == null) return;
    const resp = await fetchViews(projectId);
    if (resp.ok) views = resp.data;
  }

  function startCreate() {
    mode = "create";
    nameInput = "";
    formError = "";
  }

  async function submitCreate() {
    if (projectId == null || busy) return;
    const name = nameInput.trim();
    if (!name) {
      formError = "Name is required.";
      return;
    }
    busy = true;
    formError = "";
    const resp = await saveNewView(projectId, name, currentConfig, false);
    busy = false;
    if (!resp.ok) {
      formError = resp.error;
      return;
    }
    activeViewId = resp.data.id;
    setActiveViewId(projectIdentifier, resp.data.id);
    await refresh();
    closeMenu();
  }

  function startRename(v: SavedView) {
    mode = "rename";
    renameTargetId = v.id;
    nameInput = v.name;
    formError = "";
  }

  async function submitRename() {
    if (projectId == null || renameTargetId == null || busy) return;
    const name = nameInput.trim();
    if (!name) {
      formError = "Name is required.";
      return;
    }
    busy = true;
    formError = "";
    const resp = await saveViewUpdate(projectId, renameTargetId, { name });
    busy = false;
    if (!resp.ok) {
      formError = resp.error;
      return;
    }
    await refresh();
    closeMenu();
  }

  async function updateCurrentView() {
    if (projectId == null || !activeView || busy) return;
    busy = true;
    const resp = await saveViewUpdate(projectId, activeView.id, { config: currentConfig });
    busy = false;
    if (resp.ok) await refresh();
  }

  async function toggleDefault(v: SavedView) {
    if (projectId == null || busy) return;
    busy = true;
    const resp = await saveViewUpdate(projectId, v.id, { is_default: !v.is_default });
    busy = false;
    if (resp.ok) await refresh();
  }

  async function deleteView(v: SavedView) {
    if (projectId == null || busy) return;
    busy = true;
    const resp = await removeView(projectId, v.id);
    busy = false;
    if (!resp.ok) return;
    if (activeViewId === v.id) {
      activeViewId = null;
      setActiveViewId(projectIdentifier, null);
    }
    await refresh();
  }

  function onFormKeydown(e: KeyboardEvent, submit: () => void) {
    if (e.key === "Enter") {
      e.preventDefault();
      submit();
    } else if (e.key === "Escape") {
      e.preventDefault();
      e.stopPropagation();
      mode = "menu";
      formError = "";
    }
  }
</script>

{#if authOk === true}
  <div class="relative" bind:this={rootEl}>
    <button
      class="h-7 flex items-center gap-1.5 px-2 rounded-md
             text-caption font-medium transition-colors
             text-[var(--text-muted)] hover:text-[var(--text)]
             hover:bg-[var(--bg-subtle)]
             {open ? 'text-[var(--text)] bg-[var(--bg-subtle)]' : ''}"
      onclick={(e) => {
        e.stopPropagation();
        open = !open;
        if (!open) mode = "menu";
      }}
    >
      <Bookmark size={12} class="shrink-0" />
      <span class="hidden sm:inline max-w-[120px] truncate">
        {activeView?.name ?? "Default view"}
      </span>
      {#if drifted}
        <span
          class="size-1.5 rounded-full bg-[var(--accent)] shrink-0"
          title="Unsaved changes"
        ></span>
      {/if}
      <ChevronDown size={12} class="shrink-0 hidden sm:block" />
    </button>

    {#if open}
      <!-- svelte-ignore a11y_no_static_element_interactions a11y_click_events_have_key_events -->
      <div
        class="absolute right-0 top-full mt-1.5 z-30 w-[260px]
               bg-[var(--surface)] border border-[var(--border)]
               rounded-lg shadow-lg py-1.5 text-body-sm"
        onclick={(e) => e.stopPropagation()}
      >
        {#if mode === "menu"}
          <div class="px-3 pt-1 pb-1.5 text-[var(--text-faint)] text-micro uppercase tracking-widest font-semibold">
            Views
          </div>

          <button
            class="w-full flex items-center justify-between gap-2 px-3 py-1.5 text-left transition-colors
                   {activeViewId === null
              ? 'text-[var(--text)] bg-[var(--bg-subtle)] font-medium'
              : 'text-[var(--text-muted)] hover:text-[var(--text)] hover:bg-[var(--bg-subtle)]'}"
            onclick={() => {
              activeViewId = null;
              setActiveViewId(projectIdentifier, null);
              closeMenu();
            }}
          >
            <span>Default view</span>
            {#if activeViewId === null}<Check size={13} class="text-[var(--accent)]" />{/if}
          </button>

          {#if !viewsLoaded}
            <div class="px-3 py-2 text-caption text-[var(--text-faint)]">Loading…</div>
          {:else if views.length === 0}
            <div class="px-3 py-2 text-caption text-[var(--text-faint)]">No saved views yet.</div>
          {:else}
            {#each views as v (v.id)}
              {@const active = activeViewId === v.id}
              <div
                class="group flex items-center gap-1 px-1.5 py-0.5 rounded-md mx-1.5
                       {active ? 'bg-[var(--bg-subtle)]' : 'hover:bg-[var(--bg-subtle)]'}"
              >
                <button
                  class="shrink-0 size-6 grid place-items-center rounded
                         text-[var(--text-faint)] hover:text-[var(--warn)] transition-colors
                         {v.is_default ? 'text-[var(--warn)]' : ''}"
                  title={v.is_default ? "Unset as default" : "Set as default"}
                  onclick={() => toggleDefault(v)}
                  disabled={busy}
                >
                  <Star size={13} fill={v.is_default ? "currentColor" : "none"} />
                </button>
                <button
                  class="flex-1 min-w-0 flex items-center gap-1.5 py-1 text-left truncate
                         {active ? 'text-[var(--text)] font-medium' : 'text-[var(--text-muted)] hover:text-[var(--text)]'}"
                  onclick={() => applyView(v)}
                >
                  <span class="truncate">{v.name}</span>
                  {#if active}<Check size={13} class="text-[var(--accent)] shrink-0" />{/if}
                </button>
                <button
                  class="shrink-0 size-6 grid place-items-center rounded
                         text-[var(--text-faint)] opacity-0 group-hover:opacity-100
                         hover:text-[var(--text)] transition-all"
                  title="Rename"
                  onclick={() => startRename(v)}
                >
                  <Pencil size={12} />
                </button>
                <button
                  class="shrink-0 size-6 grid place-items-center rounded
                         text-[var(--text-faint)] opacity-0 group-hover:opacity-100
                         hover:text-[var(--error)] transition-all"
                  title="Delete"
                  onclick={() => deleteView(v)}
                >
                  <Trash2 size={12} />
                </button>
              </div>
            {/each}
          {/if}

          <div class="my-1.5 h-px bg-[var(--border)]"></div>

          {#if activeView && drifted}
            <button
              class="w-full flex items-center gap-2.5 px-3 py-1.5 text-left
                     text-body-sm text-[var(--text)] hover:bg-[var(--bg-subtle)] transition-colors"
              onclick={updateCurrentView}
              disabled={busy}
            >
              <span
                class="size-1.5 rounded-full bg-[var(--accent)] shrink-0 ml-[1px] mr-[1px]"
              ></span>
              <span class="flex-1 truncate">Update "{activeView.name}"</span>
            </button>
          {/if}

          <button
            class="w-full flex items-center gap-2.5 px-3 py-1.5 text-left
                   text-body-sm text-[var(--text)] hover:bg-[var(--bg-subtle)] transition-colors"
            onclick={startCreate}
          >
            <Plus size={14} class="text-[var(--text-muted)]" />
            <span class="flex-1">Save current as new view</span>
          </button>
        {:else if mode === "create"}
          <div class="px-3 pt-1 pb-1.5 text-[var(--text-faint)] text-micro uppercase tracking-widest font-semibold">
            Save current view
          </div>
          <div class="px-3 pb-2">
            <input
              type="text"
              bind:this={nameInputEl}
              bind:value={nameInput}
              placeholder="View name"
              onkeydown={(e) => onFormKeydown(e, submitCreate)}
              class="w-full px-2 py-1.5 text-body-sm rounded-md
                     border border-[var(--border)] bg-[var(--bg)]
                     text-[var(--text)] placeholder:text-[var(--text-faint)]
                     focus:border-[var(--accent)] outline-none"
            />
            {#if formError}
              <div class="mt-1 text-caption text-[var(--error)]">{formError}</div>
            {/if}
            <div class="mt-2 flex items-center justify-end gap-2">
              <button
                class="px-2.5 py-1 text-caption text-[var(--text-muted)] hover:text-[var(--text)] transition-colors"
                onclick={() => { mode = "menu"; formError = ""; }}
              >
                Cancel
              </button>
              <button
                class="px-2.5 py-1 rounded-md text-caption font-medium
                       bg-[var(--accent)] text-[var(--accent-text)]
                       hover:opacity-90 transition-opacity disabled:opacity-50"
                onclick={submitCreate}
                disabled={busy}
              >
                Save
              </button>
            </div>
          </div>
        {:else if mode === "rename"}
          <div class="px-3 pt-1 pb-1.5 text-[var(--text-faint)] text-micro uppercase tracking-widest font-semibold">
            Rename view
          </div>
          <div class="px-3 pb-2">
            <input
              type="text"
              bind:this={nameInputEl}
              bind:value={nameInput}
              placeholder="View name"
              onkeydown={(e) => onFormKeydown(e, submitRename)}
              class="w-full px-2 py-1.5 text-body-sm rounded-md
                     border border-[var(--border)] bg-[var(--bg)]
                     text-[var(--text)] placeholder:text-[var(--text-faint)]
                     focus:border-[var(--accent)] outline-none"
            />
            {#if formError}
              <div class="mt-1 text-caption text-[var(--error)]">{formError}</div>
            {/if}
            <div class="mt-2 flex items-center justify-end gap-2">
              <button
                class="px-2.5 py-1 text-caption text-[var(--text-muted)] hover:text-[var(--text)] transition-colors"
                onclick={() => { mode = "menu"; formError = ""; }}
              >
                Cancel
              </button>
              <button
                class="px-2.5 py-1 rounded-md text-caption font-medium
                       bg-[var(--accent)] text-[var(--accent-text)]
                       hover:opacity-90 transition-opacity disabled:opacity-50"
                onclick={submitRename}
                disabled={busy}
              >
                Rename
              </button>
            </div>
          </div>
        {/if}
      </div>
    {/if}
  </div>
{/if}

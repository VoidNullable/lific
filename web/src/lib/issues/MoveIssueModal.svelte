<script lang="ts">
  // LIF-488: "Move to project". Lists the other projects the caller can
  // write to, then confirms before moving. The issue takes a new identifier
  // in the target; the old one keeps resolving, so links already pasted
  // elsewhere still land on it. Chrome mirrors IssuePickerModal.

  import {
    getMyProjectRole,
    listProjects,
    moveIssue,
    type Issue,
    type MovedIssue,
    type Project,
  } from "../api";
  import { deriveCanEdit } from "../projectRole.svelte";
  import { filterTargets, moveTargets } from "./move";
  import ProjectIcon from "../ProjectIcon.svelte";
  import { ArrowRight, CornerDownLeft, Search, X } from "lucide-svelte";
  import { tick } from "svelte";

  let {
    open = $bindable(false),
    issue,
    onMoved,
  }: {
    open?: boolean;
    issue: Issue;
    onMoved: (moved: MovedIssue) => void;
  } = $props();

  let targets = $state<Project[]>([]);
  let loading = $state(false);
  let query = $state("");
  let selectedIdx = $state(0);
  let chosen = $state<Project | null>(null);
  let moving = $state(false);
  let errorMsg = $state("");
  let inputEl = $state<HTMLInputElement | null>(null);
  let listEl = $state<HTMLDivElement | null>(null);
  let shown = $derived(filterTargets(targets, query));

  // Bumped on every open and close, so a load or move that resolves after
  // the modal closed (or reopened for another issue) is dropped.
  let session = 0;
  $effect(() => {
    void open;
    session += 1;
    return () => {
      session += 1;
    };
  });

  $effect(() => {
    if (!open) return;
    query = "";
    selectedIdx = 0;
    chosen = null;
    errorMsg = "";
    void load(issue.project_id);
    tick().then(() => inputEl?.focus());
  });

  async function load(currentProjectId: number) {
    const started = session;
    loading = true;
    const res = await listProjects();
    if (started !== session) return;
    if (!res.ok) {
      loading = false;
      errorMsg = res.error;
      return;
    }
    const others = res.data.filter((p) => p.id !== currentProjectId);
    const roles = await Promise.all(others.map((p) => getMyProjectRole(p.id)));
    if (started !== session) return;
    const writable = new Set<number>();
    others.forEach((p, i) => {
      const role = roles[i];
      // A failed role read stays listed: the gate is the server's, and it
      // answers a move it refuses with a clear error.
      if (
        !role.ok ||
        deriveCanEdit({ role: role.data.role, enforced: role.data.enforced, isAdmin: role.data.is_admin })
      ) {
        writable.add(p.id);
      }
    });
    targets = moveTargets(res.data, currentProjectId, (id) => writable.has(id));
    loading = false;
  }

  function close() {
    open = false;
  }

  async function confirmMove() {
    if (!chosen || moving) return;
    const started = session;
    moving = true;
    errorMsg = "";
    const res = await moveIssue(issue.id, { project_id: chosen.id });
    moving = false;
    if (started !== session) return;
    if (!res.ok) {
      errorMsg = res.error;
      return;
    }
    onMoved(res.data);
    close();
  }

  // Focus stays in the search field through both steps, so the detail
  // view's own shortcuts (Esc back to the list, E to edit) stand down.
  function choose(project: Project | null) {
    chosen = project;
    inputEl?.focus();
  }

  function onKeydown(e: KeyboardEvent) {
    if (e.key === "Escape") {
      e.preventDefault();
      e.stopPropagation();
      if (chosen) choose(null);
      else close();
      return;
    }
    if (chosen) {
      if (e.key === "Enter") {
        e.preventDefault();
        void confirmMove();
      }
      return;
    }
    if (e.key === "ArrowDown") {
      e.preventDefault();
      selectedIdx = Math.min(selectedIdx + 1, shown.length - 1);
      scrollSelectedIntoView();
    } else if (e.key === "ArrowUp") {
      e.preventDefault();
      selectedIdx = Math.max(selectedIdx - 1, 0);
      scrollSelectedIntoView();
    } else if (e.key === "Enter") {
      e.preventDefault();
      const target = shown[selectedIdx];
      if (target) choose(target);
    }
  }

  function scrollSelectedIntoView() {
    requestAnimationFrame(() => {
      listEl?.querySelector(`[data-idx="${selectedIdx}"]`)?.scrollIntoView({ block: "nearest" });
    });
  }
</script>

{#if open}
  <!-- svelte-ignore a11y_no_static_element_interactions a11y_click_events_have_key_events -->
  <div
    class="fixed inset-0 z-[100] bg-black/25 flex items-start justify-center
           sm:pt-[14dvh] sm:px-4"
    onclick={close}
    onkeydown={onKeydown}
  >
    <!-- svelte-ignore a11y_no_static_element_interactions a11y_click_events_have_key_events -->
    <div
      class="w-full h-full flex flex-col bg-[var(--surface)]
             border-[var(--border)] shadow-[0_16px_48px_rgba(0,0,0,0.28)] overflow-hidden
             sm:h-auto sm:max-w-[460px] sm:border sm:rounded-xl"
      role="dialog"
      aria-modal="true"
      aria-label="Move {issue.identifier} to another project"
      tabindex="-1"
      onclick={(e) => e.stopPropagation()}
    >
      <div
        class="shrink-0 flex items-center gap-2.5 px-4 py-3 border-b border-[var(--border)]
               pt-[max(0.75rem,env(safe-area-inset-top))] sm:pt-3"
      >
        <Search size={15} class="shrink-0 text-[var(--text-faint)]" />
        <input
          bind:this={inputEl}
          bind:value={query}
          type="text"
          class="flex-1 bg-transparent border-0 outline-none text-body-lg
                 text-[var(--text)] placeholder:text-[var(--text-faint)]"
          placeholder="Move to project…"
          readonly={chosen !== null}
          oninput={() => (selectedIdx = 0)}
        />
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
          aria-label="Close"
          onclick={close}
        >
          <X size={20} />
        </button>
      </div>

      <div
        class="px-4 py-1.5 text-micro uppercase tracking-widest
               font-semibold text-[var(--text-faint)] flex items-center gap-2"
      >
        <span>Move issue</span>
        <span class="font-mono text-[var(--text-muted)] normal-case tracking-normal">
          · {issue.identifier}
        </span>
      </div>

      {#if errorMsg}
        <div class="mx-4 mb-2 text-body-sm text-[var(--error)] bg-[var(--error-bg,var(--bg-subtle))] rounded-md px-3 py-1.5">
          {errorMsg}
        </div>
      {/if}

      {#if chosen}
        <div class="px-4 pt-2 pb-4 flex flex-col gap-3">
          <p class="flex items-center gap-2 text-body text-[var(--text)]">
            <span class="font-mono">{issue.identifier}</span>
            <ArrowRight size={14} class="text-[var(--text-faint)]" />
            <ProjectIcon value={chosen.emoji} size={14} class="shrink-0" />
            <span class="font-medium">{chosen.name}</span>
          </p>
          <p class="text-body-sm text-[var(--text-muted)]">
            It gets the next {chosen.identifier} number, and {issue.identifier} keeps pointing to
            it. Comments, relations, history and plan steps come along. Labels and a module without
            a match of the same name in {chosen.identifier} are dropped.
          </p>
          <div class="flex items-center gap-2">
            <button
              class="text-body-sm font-medium text-[var(--accent-text,white)] bg-[var(--accent)]
                     px-3 py-1.5 rounded-md hover:opacity-90 transition-opacity
                     disabled:opacity-50 disabled:cursor-not-allowed"
              disabled={moving}
              onclick={confirmMove}
            >
              {moving ? "Moving..." : `Move to ${chosen.identifier}`}
            </button>
            <button
              class="text-body-sm text-[var(--text-muted)] px-3 py-1.5
                     rounded-md hover:bg-[var(--bg-subtle)] transition-colors"
              disabled={moving}
              onclick={() => choose(null)}
            >
              Back
            </button>
          </div>
        </div>
      {:else}
        <div
          class="flex-1 min-h-0 overflow-y-auto overscroll-contain py-1.5
                 sm:flex-none sm:max-h-[320px]"
          bind:this={listEl}
        >
          {#if shown.length === 0}
            <p class="px-4 py-6 text-center text-body-sm text-[var(--text-faint)]">
              {loading
                ? "Loading projects…"
                : query.trim()
                  ? `No project matches “${query.trim()}”`
                  : "No other project you can write to"}
            </p>
          {:else}
            {#each shown as project, i (project.id)}
              <button
                class="w-full flex items-center gap-2.5 px-4 py-2 text-left transition-colors
                       {i === selectedIdx ? 'bg-[var(--accent-subtle)]' : 'hover:bg-[var(--bg-subtle)]'}"
                data-idx={i}
                onclick={() => choose(project)}
                onmouseenter={() => (selectedIdx = i)}
              >
                <span class="size-5 flex items-center justify-center shrink-0">
                  <ProjectIcon value={project.emoji} size={14} />
                </span>
                <span class="flex-1 min-w-0 text-body text-[var(--text)] truncate">
                  {project.name}
                </span>
                <span class="font-mono text-micro text-[var(--text-faint)] shrink-0">
                  {project.identifier}
                </span>
                {#if i === selectedIdx}
                  <CornerDownLeft size={12} class="shrink-0 text-[var(--text-faint)]" />
                {/if}
              </button>
            {/each}
          {/if}
        </div>
      {/if}
    </div>
  </div>
{/if}

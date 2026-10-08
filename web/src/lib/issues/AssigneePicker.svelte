<script lang="ts">
  // LIF-147: choose who an issue is for. Shared by the issue sidebar and the
  // list's bulk-action bar. Three choices: leave it to agents, any person,
  // or one or more named people. The first two apply on click; people are a
  // multi-select applied with the footer button, since picking several
  // names one write at a time would leave a trail of half-done states.
  //
  // Chrome follows FilterModal: a bottom sheet on phones (LIF-227) and a
  // centered card from `sm` up. The search field only appears for rosters
  // long enough to need it, and is not focused on phones, so the software
  // keyboard does not cover the list unless someone asks for it. It is
  // portaled to <body> because the issue sidebar is a translated drawer
  // that would otherwise contain (and clip) a fixed overlay.
  import { tick, untrack } from "svelte";
  import { Bot, User, Check, X, Search } from "lucide-svelte";
  import { listMentionCandidates } from "../api";
  import { currentUser } from "../userState";
  import { toast } from "../toast/toast.svelte";
  import PersonAvatar from "./PersonAvatar.svelte";
  import { portal } from "../actions/portal";
  import {
    HUMAN,
    matchesPerson,
    orderPeople,
    personName,
    sameNames,
    type AssignmentKind,
    type Person,
  } from "./assignees";

  let {
    open = $bindable(false),
    projectId,
    subject,
    current,
    onApply,
  }: {
    open?: boolean;
    projectId: number;
    /** What is being assigned, e.g. "LIF-12" or "3 issues". */
    subject: string;
    /** The current write value (`[]`, `["human"]`, usernames), or null when
     *  several issues with different assignments are selected. */
    current: string[] | null;
    /** Apply a new assignment. `people` resolves the names for display. */
    onApply: (names: string[], people: Person[]) => void;
  } = $props();

  /** Rosters this long get a search field. */
  const SEARCH_AT = 7;

  let candidates = $state<Person[] | null>(null);
  let loadedFor: number | null = null;
  let query = $state("");
  let selected = $state<string[]>([]);
  let panelEl = $state<HTMLDivElement | null>(null);
  let searchEl = $state<HTMLInputElement | null>(null);
  let innerWidth = $state(1024);

  const me = $derived<Person | null>(
    $currentUser
      ? {
          user_id: $currentUser.id,
          username: $currentUser.username,
          display_name: $currentUser.display_name,
        }
      : null,
  );
  const people = $derived(candidates ? orderPeople(candidates, me) : []);
  const visible = $derived(people.filter((p) => matchesPerson(p, query)));
  const currentKind = $derived<AssignmentKind | null>(
    current === null
      ? null
      : current.length === 0
        ? "agents"
        : current.length === 1 && current[0] === HUMAN
          ? "human"
          : "people",
  );
  const changed = $derived(selected.length > 0 && (current === null || !sameNames(selected, current)));

  $effect(() => {
    if (!open) return;
    // Seed once per opening; a realtime refresh of `current` while the
    // dialog is up must not wipe what the person is choosing.
    untrack(() => {
      query = "";
      selected = currentKind === "people" && current ? [...current] : [];
      void load();
      tick().then(() => {
        if (innerWidth >= 640 && searchEl) searchEl.focus();
        else panelEl?.focus();
      });
    });
  });

  async function load() {
    if (loadedFor !== projectId) candidates = null;
    const pid = projectId;
    const res = await listMentionCandidates(pid);
    if (pid !== projectId) return;
    if (!res.ok) {
      candidates ??= [];
      toast(`Couldn't load people: ${res.error}`, { kind: "error" });
      return;
    }
    loadedFor = pid;
    candidates = res.data;
    if (candidates.length >= SEARCH_AT && innerWidth >= 640) {
      await tick();
      searchEl?.focus();
    }
  }

  function close() {
    open = false;
  }

  function choose(names: string[]) {
    open = false;
    onApply(names, people);
  }

  function toggle(username: string) {
    const lower = username.toLowerCase();
    selected = selected.some((s) => s.toLowerCase() === lower)
      ? selected.filter((s) => s.toLowerCase() !== lower)
      : [...selected, username];
  }

  function isSelected(username: string): boolean {
    const lower = username.toLowerCase();
    return selected.some((s) => s.toLowerCase() === lower);
  }

  function onKeydown(e: KeyboardEvent) {
    // Keep the route's own shortcuts (Esc back to the list, j/k, x) from
    // acting behind the dialog.
    e.stopPropagation();
    if (e.key === "Escape") {
      e.preventDefault();
      close();
    }
  }

  function onSearchKeydown(e: KeyboardEvent) {
    if (e.key !== "Enter") return;
    e.preventDefault();
    if (query.trim() && visible.length === 1) {
      toggle(visible[0].username);
      query = "";
    } else if (changed) {
      choose(selected);
    }
  }

  const applyLabel = $derived.by(() => {
    if (selected.length === 0) return "Assign";
    if (selected.length > 1) return `Assign ${selected.length} people`;
    const person = people.find((p) => p.username.toLowerCase() === selected[0].toLowerCase());
    return `Assign ${person ? personName(person) : `@${selected[0]}`}`;
  });
</script>

<svelte:window bind:innerWidth />

{#if open}
  <!-- svelte-ignore a11y_no_static_element_interactions a11y_click_events_have_key_events -->
  <div
    use:portal
    class="fixed inset-0 z-[100] bg-black/25 flex items-end justify-center
           sm:items-start sm:pt-[12dvh] sm:px-4"
    onclick={close}
    onkeydown={onKeydown}
  >
    <!-- svelte-ignore a11y_click_events_have_key_events -->
    <div
      bind:this={panelEl}
      class="w-full max-h-[85dvh] flex flex-col outline-none
             bg-[var(--surface)] border-t border-[var(--border)]
             rounded-t-2xl shadow-[0_-8px_48px_rgba(0,0,0,0.28)] overflow-hidden
             sm:max-w-[420px] sm:max-h-[76dvh] sm:border sm:rounded-xl
             sm:shadow-[0_16px_48px_rgba(0,0,0,0.28)]"
      role="dialog"
      aria-modal="true"
      aria-label="Assign {subject}"
      tabindex="-1"
      data-testid="assignee-picker"
      onclick={(e) => e.stopPropagation()}
    >
      <div class="sm:hidden flex justify-center pt-2 pb-0.5 shrink-0">
        <div class="h-1 w-9 rounded-full bg-[var(--border)]"></div>
      </div>

      <div class="shrink-0 flex items-center gap-2 px-5 py-3.5 border-b border-[var(--border)]">
        <h2 class="text-body-lg font-semibold text-[var(--text)]">Assign</h2>
        <span class="text-body-sm text-[var(--text-muted)] truncate">{subject}</span>
        <button
          type="button"
          class="ml-auto size-11 sm:size-7 grid place-items-center rounded-md
                 text-[var(--text-muted)] hover:text-[var(--text)]
                 hover:bg-[var(--bg-subtle)] transition-colors"
          aria-label="Close"
          onclick={close}
        >
          <X size={15} />
        </button>
      </div>

      <div class="flex-1 min-h-0 overflow-y-auto overscroll-contain px-3 py-3 flex flex-col gap-0.5">
        {#each [
          { kind: "agents" as const, names: [] as string[], label: "Any agent", hint: "Agents can pick it up. This is the default." },
          { kind: "human" as const, names: [HUMAN], label: "Any person", hint: "A person has to do it, nobody in particular." },
        ] as option (option.kind)}
          {@const active = currentKind === option.kind}
          <button
            type="button"
            class="w-full flex items-start gap-2.5 px-2.5 py-2 rounded-md text-left transition-colors
                   {active ? 'bg-[var(--accent-subtle)]' : 'hover:bg-[var(--bg-subtle)]'}"
            data-assign-choice={option.kind}
            aria-pressed={active}
            onclick={() => choose(option.names)}
          >
            <span class="size-6 shrink-0 grid place-items-center text-[var(--text-muted)]">
              {#if option.kind === "agents"}<Bot size={16} />{:else}<User size={16} />{/if}
            </span>
            <span class="flex-1 min-w-0">
              <span class="block text-body-sm font-medium text-[var(--text)]">{option.label}</span>
              <span class="block text-caption text-[var(--text-faint)]">{option.hint}</span>
            </span>
            {#if active}<Check size={14} class="mt-1 shrink-0 text-[var(--accent)]" />{/if}
          </button>
        {/each}

        <div class="px-2.5 pt-4 pb-1.5 text-micro uppercase tracking-widest font-semibold text-[var(--text-faint)]">
          Specific people
        </div>

        {#if people.length >= SEARCH_AT}
          <label class="relative mx-1 mb-1.5 block">
            <span class="sr-only">Find a person</span>
            <Search size={13} class="absolute left-2.5 top-1/2 -translate-y-1/2 text-[var(--text-faint)] pointer-events-none" />
            <input
              bind:this={searchEl}
              bind:value={query}
              type="text"
              placeholder="Find a person"
              class="w-full pl-8 pr-2 py-1.5 text-body-sm rounded-md
                     border border-[var(--border)] bg-[var(--bg)]
                     text-[var(--text)] placeholder:text-[var(--text-faint)]
                     outline-none focus:border-[var(--accent)]"
              onkeydown={onSearchKeydown}
            />
          </label>
        {/if}

        {#if candidates === null}
          <p class="px-2.5 py-2 text-body-sm text-[var(--text-faint)]">Loading people...</p>
        {:else if people.length === 0}
          <p class="px-2.5 py-2 text-body-sm text-[var(--text-faint)]">Nobody else can see this project.</p>
        {:else if visible.length === 0}
          <p class="px-2.5 py-2 text-body-sm text-[var(--text-faint)]">No one matches "{query.trim()}".</p>
        {:else}
          {#each visible as person (person.user_id)}
            {@const on = isSelected(person.username)}
            <button
              type="button"
              class="w-full flex items-center gap-2.5 px-2.5 py-2 rounded-md text-left transition-colors
                     {on ? 'bg-[var(--accent-subtle)]' : 'hover:bg-[var(--bg-subtle)]'}"
              role="checkbox"
              aria-checked={on}
              data-assign-person={person.username}
              onclick={() => toggle(person.username)}
            >
              <PersonAvatar {person} />
              <span class="flex-1 min-w-0 flex items-baseline gap-1.5">
                <span class="text-body-sm font-medium text-[var(--text)] truncate">{personName(person)}</span>
                {#if person.user_id === me?.user_id}
                  <span class="text-caption text-[var(--text-faint)] shrink-0">you</span>
                {:else if person.display_name && person.display_name !== person.username}
                  <span class="text-caption text-[var(--text-faint)] truncate">@{person.username}</span>
                {/if}
              </span>
              <span
                class="size-4 shrink-0 rounded border grid place-items-center transition-colors
                       {on
                  ? 'bg-[var(--accent)] border-[var(--accent)] text-[var(--accent-text)]'
                  : 'border-[var(--border)]'}"
              >
                {#if on}<Check size={11} strokeWidth={3} />{/if}
              </span>
            </button>
          {/each}
        {/if}
      </div>

      <div
        class="shrink-0 flex items-center justify-end gap-2 px-5 py-3 border-t border-[var(--border)]
               pb-[max(0.75rem,env(safe-area-inset-bottom))] sm:pb-3"
      >
        <button
          type="button"
          class="text-body-sm text-[var(--text-muted)] px-3 py-1.5 rounded-md hover:bg-[var(--bg-subtle)] transition-colors"
          onclick={close}
        >
          Cancel
        </button>
        <button
          type="button"
          class="text-body-sm font-medium text-[var(--btn-success-text)] bg-[var(--btn-success)]
                 px-3 py-1.5 rounded-md hover:bg-[var(--btn-success-hover)] transition-colors
                 disabled:opacity-40 disabled:cursor-not-allowed max-w-[60%] truncate"
          data-testid="assign-apply"
          disabled={!changed}
          onclick={() => choose(selected)}
        >
          {applyLabel}
        </button>
      </div>
    </div>
  </div>
{/if}

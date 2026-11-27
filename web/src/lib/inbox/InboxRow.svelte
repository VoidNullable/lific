<script lang="ts">
  // One inbox entry. The title is a real link, so new-tab clicks work. In
  // pane mode a plain click shows the issue in the reading pane; otherwise
  // it opens the issue. Hover (or keyboard focus) swaps the meta column for
  // quick actions: read state, later, done.
  import { Check, CircleCheck, Clock, Mail, MailOpen, Undo2 } from "lucide-svelte";
  import type { Issue } from "../api";
  import StatusIcon from "../StatusIcon.svelte";
  import PriorityIcon from "../PriorityIcon.svelte";
  import TimeAgo from "../TimeAgo.svelte";
  import Tooltip from "../Tooltip.svelte";
  import WaitChip from "../issues/WaitChip.svelte";
  import { openPeek } from "../issues/peek.svelte";
  import { longpress } from "../actions/longpress";
  import { projectCodeOf } from "../references";
  import LaterMenu from "./LaterMenu.svelte";
  import { REASON_CHIP, REASON_LABEL, chipWaits, type InboxEntry } from "./inbox";

  let {
    entry,
    projectName,
    unread,
    active = false,
    checked = false,
    selecting = false,
    paneMode,
    showProject = true,
    today,
    onSelect,
    onOpen,
    onToggleCheck,
    onToggleRead,
    onLater,
    onBringBack,
    onDone,
  }: {
    entry: InboxEntry;
    projectName?: string;
    unread: boolean;
    /** The keyboard cursor and the issue in the reading pane. */
    active?: boolean;
    checked?: boolean;
    /** Something is checked: show every checkbox, not just on hover. */
    selecting?: boolean;
    paneMode: boolean;
    /** Off when the rows are already grouped by project. */
    showProject?: boolean;
    today: string | null;
    onSelect: () => void;
    onOpen: (path: string) => void;
    onToggleCheck: (shift: boolean) => void;
    onToggleRead: () => void;
    onLater: (day: string) => void;
    onBringBack: () => void;
    onDone: () => void;
  } = $props();

  const issue = $derived<Issue>(entry.issue);
  const path = $derived(`/${projectCodeOf(issue.identifier)}/issues/${issue.identifier}`);
  const waits = $derived(chipWaits(issue, today));
  let laterOpen = $state(false);

  function onclick(e: MouseEvent) {
    if (e.button !== 0) return;
    if (e.shiftKey) {
      e.preventDefault();
      openPeek(issue.identifier);
      return;
    }
    if (e.metaKey || e.ctrlKey || e.altKey) return;
    e.preventDefault();
    if (paneMode) onSelect();
    else onOpen(path);
  }

  function laterLabel(day: string): string {
    const [y, m, d] = day.split("-").map(Number);
    return new Date(y, m - 1, d).toLocaleDateString(undefined, { weekday: "short", month: "short", day: "numeric" });
  }
</script>

<svelte:window onclick={() => (laterOpen = false)} />

<div
  class="inbox-row group relative flex items-center gap-2 pl-1.5 pr-3 h-11 sm:h-10
         border-b border-[var(--border)] last:border-b-0 transition-colors
         first:rounded-t-[calc(var(--radius-lg)-1px)] last:rounded-b-[calc(var(--radius-lg)-1px)]
         {active ? 'bg-[var(--accent-subtle)]' : checked ? 'bg-[var(--bg-subtle)]' : 'hover:bg-[var(--bg-subtle)]'}
         {laterOpen ? 'z-20' : ''}"
  data-identifier={issue.identifier}
  data-unread={unread ? "true" : undefined}
  data-active={active ? "true" : undefined}
  role="option"
  aria-selected={active}
  tabindex="-1"
>
  {#if active}
    <span class="absolute left-0 top-1.5 bottom-1.5 w-[2px] rounded-full bg-[var(--accent)]" aria-hidden="true"></span>
  {/if}

  <button
    type="button"
    class="shrink-0 size-5 grid place-items-center rounded transition-opacity
           {checked || selecting ? 'opacity-100' : 'opacity-0 group-hover:opacity-100 focus-visible:opacity-100 [@media(hover:none)]:hidden'}"
    aria-label={checked ? `Deselect ${issue.identifier}` : `Select ${issue.identifier}`}
    aria-pressed={checked}
    onclick={(e) => {
      e.stopPropagation();
      onToggleCheck(e.shiftKey);
    }}
  >
    <span
      class="size-3.5 rounded-[4px] border grid place-items-center transition-colors
             {checked ? 'bg-[var(--accent)] border-[var(--accent)] text-[var(--accent-text)]' : 'border-[var(--text-faint)] bg-[var(--surface)]'}"
    >
      {#if checked}<Check size={10} strokeWidth={3} />{/if}
    </span>
  </button>

  <span class="w-1.5 shrink-0 flex items-center justify-center">
    {#if unread}
      <span class="block size-1.5 rounded-full bg-[var(--accent)]"></span>
      <span class="sr-only">Unread.</span>
    {/if}
  </span>

  <a
    href={`#${path}`}
    class="flex-1 min-w-0 flex items-center gap-2.5 self-stretch hover:no-underline
           focus-visible:outline-2 focus-visible:outline-[var(--accent)] focus-visible:-outline-offset-2 rounded-sm"
    use:longpress={{ onLongPress: () => openPeek(issue.identifier) }}
    {onclick}
    onmousedown={(e) => {
      if (e.shiftKey) e.preventDefault();
    }}
  >
    <span class="shrink-0 flex" title={issue.status[0].toUpperCase() + issue.status.slice(1)}>
      <StatusIcon status={issue.status} size={14} />
    </span>
    <span class="hidden @md:block text-caption font-mono text-[var(--text-faint)] w-[72px] shrink-0 truncate">
      {issue.identifier}
    </span>
    <span
      class="text-body-sm truncate min-w-0 {unread ? 'font-semibold text-[var(--text)]' : 'text-[color-mix(in_srgb,var(--text)_82%,var(--text-muted))]'}"
    >
      {issue.title}
    </span>
    <span class="hidden @2xl:flex items-center gap-1 shrink-0">
      {#each entry.reasons as r (r)}
        <span
          class="reason-chip shrink-0 text-micro px-1.5 py-px rounded-full border border-[var(--border)] text-[var(--text-muted)]"
          data-reason={r}
          title={REASON_LABEL[r]}
        >
          {REASON_CHIP[r]}
        </span>
      {/each}
      {#if entry.returned}
        <span class="shrink-0 inline-flex items-center gap-1 text-micro px-1.5 py-px rounded-full bg-[var(--accent-subtle)] text-[var(--accent)] font-medium">
          <Undo2 size={10} /> Back
        </span>
      {/if}
      {#each issue.labels.slice(0, 2) as l (l)}
        <span class="hidden @3xl:inline shrink-0 max-w-[96px] truncate text-micro px-1.5 py-px rounded-full bg-[var(--bg-subtle)] text-[var(--text-muted)]">{l}</span>
      {/each}
    </span>
    {#if waits.length}
      <span class="hidden @md:contents"><WaitChip {waits} /></span>
    {/if}
  </a>

  <!-- Meta: fades out under the hover actions on pointer devices. -->
  <div
    class="shrink-0 flex items-center gap-2.5 transition-opacity
           [@media(hover:hover)]:group-hover:opacity-0 [@media(hover:hover)]:group-focus-within:opacity-0
           {laterOpen ? 'opacity-0' : ''}"
  >
    {#if entry.later}
      <span class="inline-flex items-center gap-1 text-micro text-[var(--warn-text)]" title="Put off until {laterLabel(entry.later.earliest ?? '')}">
        <Clock size={11} />
        {laterLabel(entry.later.earliest ?? "")}
      </span>
    {/if}
    {#if showProject}
      <span class="hidden @xl:block max-w-[120px] truncate text-caption text-[var(--text-faint)]">
        {projectName ?? projectCodeOf(issue.identifier)}
      </span>
    {/if}
    <span class="w-[15px] flex justify-center" title={`Priority: ${issue.priority}`}>
      <PriorityIcon priority={issue.priority} size={15} />
    </span>
    <TimeAgo
      date={issue.updated_at.replace(" ", "T")}
      class="w-[52px] text-right text-micro tabular-nums text-[var(--text-faint)]"
    />
  </div>

  <div
    class="row-actions absolute right-2 top-1/2 -translate-y-1/2 hidden [@media(hover:hover)]:flex items-center gap-0.5
           p-0.5 rounded-lg bg-[var(--surface)] border border-[var(--border)] shadow-sm
           transition-opacity {laterOpen ? 'opacity-100' : 'opacity-0 pointer-events-none group-hover:opacity-100 group-hover:pointer-events-auto group-focus-within:opacity-100 group-focus-within:pointer-events-auto'}"
  >
    <Tooltip content={unread ? "Mark read (U)" : "Mark unread (U)"}>
      <button type="button" class="act" aria-label={unread ? "Mark read" : "Mark unread"} onclick={(e) => { e.stopPropagation(); onToggleRead(); }}>
        {#if unread}<MailOpen size={14} />{:else}<Mail size={14} />{/if}
      </button>
    </Tooltip>
    {#if entry.later}
      <Tooltip content="Bring back now">
        <button type="button" class="act" aria-label="Bring back now" onclick={(e) => { e.stopPropagation(); onBringBack(); }}>
          <Undo2 size={14} />
        </button>
      </Tooltip>
    {:else if today}
      <div class="relative">
        <Tooltip content="Later (L)">
          <button
            type="button"
            class="act"
            aria-label="Put off until later"
            aria-expanded={laterOpen}
            onclick={(e) => {
              e.stopPropagation();
              laterOpen = !laterOpen;
            }}
          >
            <Clock size={14} />
          </button>
        </Tooltip>
        {#if laterOpen}
          <LaterMenu
            {today}
            onPick={(day) => {
              laterOpen = false;
              onLater(day);
            }}
            onClose={() => (laterOpen = false)}
          />
        {/if}
      </div>
    {/if}
    <Tooltip content="Mark done (E)">
      <button type="button" class="act hover:!text-[var(--success)]" aria-label="Mark done" onclick={(e) => { e.stopPropagation(); onDone(); }}>
        <CircleCheck size={14} />
      </button>
    </Tooltip>
  </div>
</div>

<style>
  .act {
    display: grid;
    place-items: center;
    width: 1.75rem;
    height: 1.75rem;
    border-radius: 0.375rem;
    color: var(--text-muted);
    transition: background-color 120ms, color 120ms;
  }
  .act:hover {
    background: var(--bg-subtle);
    color: var(--text);
  }
</style>

<script lang="ts">
  // LIF-539: Inbox, everything that needs the signed-in person across the
  // projects they can see, as one triage feed. Each issue appears once with
  // every reason it is here. It can be read and unread (per device), put
  // off until a day (a real date wait, so it comes back on its own),
  // started or closed, and read in a pane without leaving the page.
  import { getContext } from "svelte";
  import {
    Inbox as InboxIcon,
    Search,
    CheckCheck,
    Flame,
    CircleAlert,
    Clock,
    PanelRight,
    PanelRightClose,
    ChevronRight,
    CircleCheck,
    Mail,
    MailOpen,
    X,
    UserCheck,
    Users,
    Hourglass,
  } from "lucide-svelte";
  import {
    addIssueWait,
    clearIssueWait,
    getAttention,
    listProjects,
    updateIssue,
    type Attention,
    type Issue,
    type Project,
  } from "../lib/api";
  import ErrorState from "../lib/ErrorState.svelte";
  import Mascot from "../lib/Mascot.svelte";
  import Select from "../lib/Select.svelte";
  import PriorityIcon from "../lib/PriorityIcon.svelte";
  import ProjectIcon from "../lib/ProjectIcon.svelte";
  import { startAutoRefresh } from "../lib/autoRefresh.svelte";
  import WorkspaceSkeleton from "../lib/workspace/WorkspaceSkeleton.svelte";
  import { toast } from "../lib/toast/toast.svelte";
  import { now } from "../lib/now.svelte";
  import { ensureServerClock, serverDay } from "../lib/serverClock.svelte";
  import { shortcutsSuppressed } from "../lib/shortcuts";
  import { updateIssueWithUndo } from "../lib/issues/state.svelte";
  import { TERMINAL_STATUSES } from "../lib/issues/grouping";
  import { projectCodeOf } from "../lib/references";
  import InboxRow from "../lib/inbox/InboxRow.svelte";
  import InboxPane from "../lib/inbox/InboxPane.svelte";
  import {
    LATER_NOTE,
    addDays,
    anyCapped,
    applyBaseFilter,
    buildEntries,
    groupEntries,
    hasOverdueWait,
    inTab,
    isPressing,
    isUnread,
    markRead,
    markUnread,
    reconcileRead,
    sortEntries,
    type GroupBy,
    type InboxEntry,
    type InboxTab,
    type ReadState,
    type SortBy,
  } from "../lib/inbox/inbox";
  import { loadReadState, loadView, saveReadState, saveView, type InboxView } from "../lib/inbox/store";

  let { navigate }: { navigate: (path: string) => void } = $props();

  const topbarCtx = getContext<{
    set: (s: import("svelte").Snippet | undefined) => void;
  } | undefined>("lific:topbar");

  $effect(() => {
    topbarCtx?.set(topbarContent);
    return () => topbarCtx?.set(undefined);
  });

  /** The attention endpoint returns at most this many issues per group. */
  const ATTENTION_LIMIT = 50;

  let attention = $state<Attention | null>(null);
  let projects = $state<Project[]>([]);
  let loading = $state(true);
  let error = $state("");

  let view = $state<InboxView>(loadView());
  $effect(() => saveView($state.snapshot(view)));

  let readState = $state.raw<ReadState>(loadReadState());
  function setRead(next: ReadState) {
    if (next === readState) return;
    readState = next;
    saveReadState(next);
  }

  // Issues read while the Unread tab is open stay on it until the tab
  // changes, so rows do not vanish from under the cursor.
  let keptUnread = $state<Set<number>>(new Set());
  function readIssues(issues: Issue[]) {
    if (view.tab === "unread") {
      const next = new Set(keptUnread);
      for (const i of issues) if (isUnread(readState, i)) next.add(i.id);
      keptUnread = next;
    }
    setRead(markRead(readState, issues));
  }

  let query = $state("");
  let projectFilter = $state<number | null>(null);
  let pressingOnly = $state(false);
  let overdueOnly = $state(false);
  let collapsed = $state<Set<string>>(new Set());
  let checked = $state<Set<number>>(new Set());
  let lastChecked = $state<number | null>(null);
  let activeId = $state<number | null>(null);
  let searchEl = $state<HTMLInputElement | null>(null);
  let listEl = $state<HTMLElement | null>(null);

  // The reading pane needs room: from lg up, and only when it is turned on.
  let wide = $state(typeof window !== "undefined" && window.matchMedia("(min-width: 1024px)").matches);
  $effect(() => {
    const mq = window.matchMedia("(min-width: 1024px)");
    const on = () => (wide = mq.matches);
    mq.addEventListener("change", on);
    return () => mq.removeEventListener("change", on);
  });
  const paneMode = $derived(wide && view.pane);

  const today = $derived(serverDay(now()));
  $effect(() => ensureServerClock(now()));

  $effect(() => {
    load();
  });

  $effect(() =>
    startAutoRefresh({
      refresh: () => load(false),
      isBusy: () => loading,
      realtimeDebounceMs: 750,
      realtimeMaxWaitMs: 5_000,
      shouldRefresh: (event) =>
        event.type === "resync.required" ||
        event.type.startsWith("project.") ||
        event.type.startsWith("issue."),
    }),
  );

  async function load(initial = true) {
    if (initial) {
      loading = true;
      error = "";
    }
    const [attentionRes, projectsRes] = await Promise.all([getAttention(), listProjects()]);
    if (projectsRes.ok) projects = projectsRes.data;
    if (attentionRes.ok) {
      attention = attentionRes.data;
      error = "";
    } else if (initial || !attention) {
      error = attentionRes.error;
    }
    if (initial) loading = false;
  }

  const entries = $derived(attention ? buildEntries(attention, today) : []);

  // First sight on this device seeds everything as read; afterwards records
  // for issues that left the inbox are dropped.
  $effect(() => {
    if (!attention) return;
    const issues = entries.map((e) => e.issue);
    setRead(reconcileRead(readState, issues));
  });

  const projectNames = $derived(new Map(projects.map((p) => [p.id, p.name])));
  const projectEmoji = $derived(new Map(projects.map((p) => [p.id, p.emoji])));
  const nameOf = (id: number) => projectNames.get(id);

  const filter = $derived({
    tab: view.tab,
    projects: projectFilter === null ? [] : [projectFilter],
    query,
    pressing: pressingOnly,
    overdue: overdueOnly,
  });

  const base = $derived(applyBaseFilter(entries, filter, today, nameOf));
  const unreadOf = (e: InboxEntry) => isUnread(readState, e.issue);
  const unreadForTab = (e: InboxEntry) => unreadOf(e) || keptUnread.has(e.issue.id);

  const TABS: { key: InboxTab; label: string }[] = [
    { key: "all", label: "All" },
    { key: "unread", label: "Unread" },
    { key: "assigned", label: "Assigned" },
    { key: "human", label: "For anyone" },
    { key: "waiting", label: "Waiting" },
    { key: "later", label: "Later" },
  ];

  const tabCounts = $derived(
    Object.fromEntries(TABS.map((t) => [t.key, base.filter((e) => inTab(e, t.key, t.key === "unread" ? unreadOf(e) : false)).length])) as Record<InboxTab, number>,
  );

  const visible = $derived(sortEntries(base.filter((e) => inTab(e, view.tab, unreadForTab(e))), view.sort));
  const groups = $derived(groupEntries(visible, view.group, nameOf));
  /** The rows in screen order, skipping collapsed groups: j/k walk this. */
  const order = $derived(groups.filter((g) => !collapsed.has(g.key)).flatMap((g) => g.entries));

  // Whole-inbox numbers for the header, ignoring the toolbar's filters.
  const current = $derived(entries.filter((e) => !e.later));
  const stats = $derived({
    unread: current.filter(unreadOf).length,
    pressing: current.filter((e) => isPressing(e.issue)).length,
    overdue: current.filter((e) => hasOverdueWait(e.issue, today)).length,
    later: entries.length - current.length,
    back: current.filter((e) => e.returned).length,
  });

  const capped = $derived(attention ? anyCapped(attention, ATTENTION_LIMIT) : false);
  const filtered = $derived(query.trim() !== "" || projectFilter !== null || pressingOnly || overdueOnly);

  const activeEntry = $derived(entries.find((e) => e.issue.id === activeId) ?? null);
  const checkedEntries = $derived(entries.filter((e) => checked.has(e.issue.id)));

  // Drop checks for rows that left the list.
  $effect(() => {
    const live = new Set(entries.map((e) => e.issue.id));
    if ([...checked].some((id) => !live.has(id))) checked = new Set([...checked].filter((id) => live.has(id)));
  });

  // In pane mode, start on the first row so the pane is never empty when
  // there is something to read.
  $effect(() => {
    if (!paneMode || loading) return;
    if (activeId === null || !entries.some((e) => e.issue.id === activeId)) {
      activeId = order[0]?.issue.id ?? null;
    }
  });

  // Reading in the pane marks an issue read once it has been shown for a
  // moment, so skimming past with j/k does not.
  $effect(() => {
    const entry = activeEntry;
    if (!paneMode || !entry || !isUnread(readState, entry.issue)) return;
    const t = setTimeout(() => readIssues([entry.issue]), 900);
    return () => clearTimeout(t);
  });

  // ── Local edits ────────────────────────────────────────────────────

  function replaceIssue(id: number, fn: (i: Issue) => Issue | null) {
    if (!attention) return;
    const map = (list: Issue[]) =>
      list.flatMap((i) => {
        if (i.id !== id) return [i];
        const next = fn(i);
        return next ? [next] : [];
      });
    attention = { assigned: map(attention.assigned), human: map(attention.human), waiting: map(attention.waiting) };
  }

  /** The row after `id` in screen order, then the one before. */
  function neighbour(id: number): number | null {
    const i = order.findIndex((e) => e.issue.id === id);
    if (i < 0) return null;
    return order[i + 1]?.issue.id ?? order[i - 1]?.issue.id ?? null;
  }

  function open(entry: InboxEntry) {
    readIssues([entry.issue]);
    navigate(`/${projectCodeOf(entry.issue.identifier)}/issues/${entry.issue.identifier}`);
  }

  function toggleRead(list: InboxEntry[]) {
    const issues = list.map((e) => e.issue);
    const anyUnread = issues.some((i) => isUnread(readState, i));
    if (anyUnread) readIssues(issues);
    else setRead(markUnread(readState, issues));
  }

  function markAllRead() {
    const list = visible.filter(unreadOf).map((e) => e.issue);
    if (list.length === 0) return;
    const before = readState;
    readIssues(list);
    toast(list.length === 1 ? "Marked 1 issue read" : `Marked ${list.length} issues read`, {
      kind: "success",
      action: { label: "Undo", fn: () => setRead(before) },
    });
  }

  function dayLabel(day: string): string {
    const [y, m, d] = day.split("-").map(Number);
    return new Date(y, m - 1, d).toLocaleDateString(undefined, { weekday: "short", month: "short", day: "numeric" });
  }

  async function later(list: InboxEntry[], day: string) {
    const targets = list.filter((e) => !e.later);
    if (targets.length === 0) return;
    if (activeId !== null && targets.some((e) => e.issue.id === activeId) && view.tab !== "later") {
      activeId = neighbour(activeId);
    }
    const added: { issueId: number; waitId: number }[] = [];
    const failed: string[] = [];
    await Promise.all(
      targets.map(async (e) => {
        const res = await addIssueWait(e.issue.id, { from: day, note: LATER_NOTE });
        if (!res.ok) {
          failed.push(e.issue.identifier);
          return;
        }
        added.push({ issueId: e.issue.id, waitId: res.data.id });
        replaceIssue(e.issue.id, (i) => ({ ...i, waits: [...(i.waits ?? []), res.data] }));
      }),
    );
    if (failed.length) toast(`Couldn't put off ${failed.join(", ")}`, { kind: "error" });
    if (added.length === 0) return;
    const what = added.length === 1 ? targets.find((e) => e.issue.id === added[0].issueId)!.issue.identifier : `${added.length} issues`;
    toast(`${what} put off until ${dayLabel(day)}`, {
      kind: "success",
      action: {
        label: "Undo",
        fn: async () => {
          await Promise.all(added.map((a) => clearWait(a.issueId, a.waitId, false)));
        },
      },
    });
  }

  async function clearWait(issueId: number, waitId: number, announce = true) {
    const res = await clearIssueWait(issueId, waitId);
    if (!res.ok) {
      toast(`Couldn't clear the wait: ${res.error}`, { kind: "error" });
      return;
    }
    replaceIssue(issueId, (i) => ({ ...i, waits: (i.waits ?? []).filter((w) => w.id !== waitId) }));
    if (announce) toast("Back in the inbox", { kind: "info", duration: 2500 });
  }

  function applyUpdated(updated: Issue) {
    if (TERMINAL_STATUSES.includes(updated.status)) {
      replaceIssue(updated.id, () => null);
    } else if (entries.some((e) => e.issue.id === updated.id)) {
      replaceIssue(updated.id, () => updated);
    } else {
      // Undone after it left the inbox: fetch it back.
      void load(false);
    }
  }

  async function setStatus(list: InboxEntry[], status: string) {
    const targets = list.filter((e) => e.issue.status !== status);
    if (targets.length === 0) return;
    if (TERMINAL_STATUSES.includes(status) && activeId !== null && targets.some((e) => e.issue.id === activeId)) {
      activeId = neighbour(activeId);
    }
    if (targets.length === 1) {
      const issue = targets[0].issue;
      await updateIssueWithUndo({
        id: issue.id,
        identifier: issue.identifier,
        patch: { status },
        prevPatch: { status: issue.status },
        modules: [],
        onApplied: (_patch, updated) => applyUpdated(updated),
      });
      return;
    }
    const done: { id: number; prev: string }[] = [];
    const failed: string[] = [];
    await Promise.all(
      targets.map(async (e) => {
        const res = await updateIssue(e.issue.id, { status });
        if (res.ok) {
          done.push({ id: e.issue.id, prev: e.issue.status });
          applyUpdated(res.data);
        } else failed.push(e.issue.identifier);
      }),
    );
    if (failed.length) toast(`Couldn't update ${failed.join(", ")}`, { kind: "error" });
    if (done.length === 0) return;
    toast(`${done.length} issues moved to ${status}`, {
      kind: "success",
      action: {
        label: "Undo",
        fn: async () => {
          const results = await Promise.all(done.map((d) => updateIssue(d.id, { status: d.prev })));
          if (results.some((r) => !r.ok)) toast("Couldn't undo every change", { kind: "error" });
          void load(false);
        },
      },
    });
  }

  // ── Selection ──────────────────────────────────────────────────────

  function toggleCheck(id: number, shift: boolean) {
    const next = new Set(checked);
    if (shift && lastChecked !== null) {
      const a = order.findIndex((e) => e.issue.id === lastChecked);
      const b = order.findIndex((e) => e.issue.id === id);
      if (a >= 0 && b >= 0) {
        const on = !checked.has(id);
        for (const e of order.slice(Math.min(a, b), Math.max(a, b) + 1)) {
          if (on) next.add(e.issue.id);
          else next.delete(e.issue.id);
        }
        checked = next;
        lastChecked = id;
        return;
      }
    }
    if (next.has(id)) next.delete(id);
    else next.add(id);
    checked = next;
    lastChecked = id;
  }

  function selectAllShown() {
    const all = order.every((e) => checked.has(e.issue.id));
    checked = all ? new Set() : new Set(order.map((e) => e.issue.id));
  }

  function toggleGroup(key: string) {
    const next = new Set(collapsed);
    if (next.has(key)) next.delete(key);
    else next.add(key);
    collapsed = next;
  }

  function setTab(tab: InboxTab) {
    view.tab = tab;
    checked = new Set();
    keptUnread = new Set();
  }

  function clearFilters() {
    query = "";
    projectFilter = null;
    pressingOnly = false;
    overdueOnly = false;
  }

  function moveCursor(delta: number) {
    if (order.length === 0) return;
    const i = order.findIndex((e) => e.issue.id === activeId);
    const next = i < 0 ? (delta >= 0 ? 0 : order.length - 1) : Math.max(0, Math.min(order.length - 1, i + delta));
    activeId = order[next].issue.id;
    requestAnimationFrame(() => {
      listEl?.querySelector(`[data-identifier="${order[next].issue.identifier}"]`)?.scrollIntoView({ block: "nearest" });
    });
  }

  /** Bulk actions take the checked rows; single-key actions the cursor row. */
  function actionTargets(): InboxEntry[] {
    if (checkedEntries.length) return checkedEntries;
    return activeEntry ? [activeEntry] : [];
  }

  function onKeydown(e: KeyboardEvent) {
    if (shortcutsSuppressed() || e.metaKey || e.ctrlKey || e.altKey) return;
    const key = e.key;
    if (key === "/") {
      e.preventDefault();
      searchEl?.focus();
      return;
    }
    if (key === "j" || key === "ArrowDown") {
      e.preventDefault();
      moveCursor(1);
    } else if (key === "k" || key === "ArrowUp") {
      e.preventDefault();
      moveCursor(-1);
    } else if ((key === "Enter" || key === "o") && activeEntry) {
      e.preventDefault();
      open(activeEntry);
    } else if (key === "u") {
      toggleRead(actionTargets());
    } else if (key === "e") {
      void setStatus(actionTargets(), "done");
    } else if (key === "l" && today) {
      void later(actionTargets(), addDays(today, 1));
    } else if (key === "x" && activeEntry) {
      toggleCheck(activeEntry.issue.id, e.shiftKey);
    } else if (key === "A" && e.shiftKey) {
      markAllRead();
    } else if (/^[1-6]$/.test(key)) {
      setTab(TABS[Number(key) - 1].key);
    } else if (key === "Escape") {
      if (checked.size) checked = new Set();
      else if (filtered) clearFilters();
    }
  }

  const SORTS: { value: SortBy; label: string }[] = [
    { value: "priority", label: "Priority" },
    { value: "newest", label: "Newest" },
    { value: "oldest", label: "Oldest" },
  ];
  const GROUPS: { value: GroupBy; label: string }[] = [
    { value: "project", label: "Project" },
    { value: "priority", label: "Priority" },
    { value: "reason", label: "Reason" },
    { value: "updated", label: "Updated" },
    { value: "none", label: "No grouping" },
  ];
  const REASON_ICON = { assigned: UserCheck, human: Users, waiting: Hourglass } as const;

  const projectOptions = $derived([
    { value: null, label: "All projects" },
    ...[...new Set(entries.map((e) => e.issue.project_id))]
      .map((id) => ({ value: id, label: nameOf(id) ?? `Project ${id}` }))
      .sort((a, b) => a.label.localeCompare(b.label)),
  ]);

  const EMPTY: Record<InboxTab, { title: string; body: string }> = {
    all: { title: "Inbox zero", body: "Nothing needs you right now." },
    unread: { title: "All caught up", body: "Everything here has been read." },
    assigned: { title: "Nothing assigned to you", body: "Issues that name you land here." },
    human: { title: "Nothing for anyone", body: "Issues marked for any person, naming no one, land here." },
    waiting: { title: "No one is waiting on you", body: "Issues with a wait on you land here." },
    later: { title: "Nothing put off", body: "Use Later on a row to bring it back on a day you pick." },
  };
</script>

<svelte:window onkeydown={onKeydown} />

{#snippet topbarContent()}
  <div class="flex items-center gap-2 px-6 py-2 w-full">
    <InboxIcon size={14} class="shrink-0 text-[var(--text-faint)]" />
    <span class="text-body-sm font-medium text-[var(--text)]">Inbox</span>
    {#if stats.unread > 0}
      <span class="text-micro font-semibold tabular-nums px-1.5 py-px rounded-full bg-[var(--accent)] text-[var(--accent-text)]">
        {stats.unread}
      </span>
    {/if}
  </div>
{/snippet}

<div class="h-full flex flex-col min-h-0">
  {#if loading}
    <div class="max-w-[960px] w-full mx-auto px-6 md:px-8 py-8 md:py-10">
      <WorkspaceSkeleton groups={[3, 2, 2]} />
    </div>
  {:else if error}
    <ErrorState title="Couldn't load your inbox" message={error}>
      <button
        class="text-body-sm font-medium text-[var(--btn-success-text)] bg-[var(--btn-success)] px-3 py-1.5 rounded-md hover:bg-[var(--btn-success-hover)] transition-colors"
        onclick={() => load()}
      >
        Try again
      </button>
    </ErrorState>
  {:else}
    <!-- Header: title, the inbox at a glance, and the toolbar. -->
    <div class="shrink-0 border-b border-[var(--border)]">
      <div class="px-4 sm:px-6 pt-5 sm:pt-6 pb-3 flex items-start sm:items-end gap-4">
        <div class="flex-1 min-w-0">
          <h1 class="font-display text-title tracking-tight text-[var(--text)] leading-none flex items-baseline gap-2.5">
            Inbox
            <span class="text-body-sm font-sans tracking-normal text-[var(--text-faint)] tabular-nums" data-testid="inbox-total">
              {current.length}
            </span>
          </h1>
          <div class="mt-2 flex flex-wrap items-center gap-1.5 text-caption" data-testid="inbox-stats">
            <button type="button" class="stat {view.tab === 'unread' ? 'stat-on' : ''}" onclick={() => setTab(view.tab === "unread" ? "all" : "unread")}>
              <span class="size-1.5 rounded-full bg-[var(--accent)]"></span>
              <b class="tabular-nums">{stats.unread}</b> unread
            </button>
            <button type="button" class="stat {pressingOnly ? 'stat-on' : ''}" onclick={() => (pressingOnly = !pressingOnly)}>
              <Flame size={12} class="text-[var(--error)]" />
              <b class="tabular-nums">{stats.pressing}</b> urgent or high
            </button>
            {#if stats.overdue > 0}
              <button type="button" class="stat {overdueOnly ? 'stat-on' : ''}" onclick={() => (overdueOnly = !overdueOnly)}>
                <CircleAlert size={12} class="text-[var(--error)]" />
                <b class="tabular-nums">{stats.overdue}</b> overdue
              </button>
            {/if}
            {#if stats.later > 0}
              <button type="button" class="stat {view.tab === 'later' ? 'stat-on' : ''}" onclick={() => setTab(view.tab === "later" ? "all" : "later")}>
                <Clock size={12} class="text-[var(--warn-text)]" />
                <b class="tabular-nums">{stats.later}</b> put off
              </button>
            {/if}
            {#if stats.back > 0}
              <span class="text-[var(--text-muted)] px-1">{stats.back} back from later</span>
            {/if}
          </div>
        </div>
        <div class="shrink-0 flex items-center gap-1.5">
          <button
            type="button"
            class="tool-btn"
            disabled={!visible.some(unreadOf)}
            onclick={markAllRead}
            aria-label="Mark all read"
            title="Mark everything shown as read (Shift A)"
          >
            <CheckCheck size={14} /> <span class="hidden sm:inline">Mark all read</span>
          </button>
          {#if wide}
            <button
              type="button"
              class="tool-btn"
              aria-pressed={view.pane}
              onclick={() => (view.pane = !view.pane)}
              title={view.pane ? "Hide the reading pane" : "Show the reading pane"}
            >
              {#if view.pane}<PanelRightClose size={14} />{:else}<PanelRight size={14} />{/if}
            </button>
          {/if}
        </div>
      </div>

      <!-- Tabs -->
      <div class="px-4 sm:px-6 flex items-end gap-1 overflow-x-auto no-scrollbar" role="tablist" aria-label="Inbox views">
        {#each TABS as t, i (t.key)}
          <button
            type="button"
            role="tab"
            aria-selected={view.tab === t.key}
            data-tab={t.key}
            class="tab {view.tab === t.key ? 'tab-on' : ''}"
            title="{t.label} ({i + 1})"
            onclick={() => setTab(t.key)}
          >
            {t.label}
            {#if tabCounts[t.key] > 0}
              <span class="tab-count {t.key === 'unread' && tabCounts.unread > 0 ? 'tab-count-hot' : ''}">{tabCounts[t.key]}</span>
            {/if}
          </button>
        {/each}
      </div>
    </div>

    <!-- Toolbar: search, project, sort, grouping. -->
    <div class="shrink-0 px-4 sm:px-6 py-2 flex flex-wrap items-center gap-2 border-b border-[var(--border)] bg-[var(--bg)]">
      <label class="relative flex-1 min-w-[160px] max-w-[320px]">
        <Search size={13} class="absolute left-2.5 top-1/2 -translate-y-1/2 text-[var(--text-faint)] pointer-events-none" />
        <input
          bind:this={searchEl}
          bind:value={query}
          type="search"
          placeholder="Search the inbox"
          aria-label="Search the inbox"
          class="w-full h-7 pl-8 pr-7 rounded-md text-body-sm bg-[var(--surface)] border border-[var(--border)]
                 text-[var(--text)] outline-none focus:border-[var(--accent)] placeholder:text-[var(--text-faint)]"
          onkeydown={(e) => {
            if (e.key === "Escape") {
              if (query) query = "";
              else (e.currentTarget as HTMLInputElement).blur();
            } else if (e.key === "Enter" || e.key === "ArrowDown") {
              e.preventDefault();
              (e.currentTarget as HTMLInputElement).blur();
              moveCursor(0);
            }
          }}
        />
        {#if !query}
          <kbd
            class="absolute right-1.5 top-1/2 -translate-y-1/2 hidden md:[@media(hover:hover)]:grid place-items-center
                   size-[1.15rem] rounded border border-[var(--border)] bg-[var(--bg-subtle)]
                   text-[0.625rem] font-mono text-[var(--text-muted)] pointer-events-none">/</kbd>
        {/if}
      </label>
      <div class="w-[160px]">
        <Select size="sm" options={projectOptions} value={projectFilter} onchange={(o) => (projectFilter = o.value as number | null)} />
      </div>
      <div class="flex items-center gap-1.5 text-caption text-[var(--text-faint)]">
        <span class="hidden sm:inline">Sort</span>
        <div class="w-[104px]">
          <Select size="sm" options={SORTS} value={view.sort} onchange={(o) => (view.sort = o.value as SortBy)} />
        </div>
      </div>
      <div class="flex items-center gap-1.5 text-caption text-[var(--text-faint)]">
        <span class="hidden sm:inline">Group</span>
        <div class="w-[124px]">
          <Select size="sm" options={GROUPS} value={view.group} onchange={(o) => (view.group = o.value as GroupBy)} />
        </div>
      </div>
      {#if filtered}
        <button type="button" class="text-caption text-[var(--text-muted)] hover:text-[var(--text)] inline-flex items-center gap-1" onclick={clearFilters}>
          <X size={12} /> Clear filters
        </button>
      {/if}
    </div>

    <div class="flex-1 min-h-0 flex">
      <!-- The list -->
      <div class="@container flex-1 min-w-0 overflow-y-auto" bind:this={listEl}>
        {#if visible.length === 0}
          <div class="flex flex-col items-center justify-center text-center py-16 px-6 gap-3" data-testid="inbox-empty">
            {#if filtered}
              <Mascot src="/LizzyReading.png" nativeW={487} nativeH={714} scale={0.12} />
              <p class="text-body font-medium text-[var(--text)]">Nothing matches</p>
              <button type="button" class="tool-btn" onclick={clearFilters}>Clear filters</button>
            {:else}
              <Mascot src="/LizzySleep2.png" nativeW={1000} nativeH={420} scale={0.16} />
              <p class="text-body font-medium text-[var(--text)]">{EMPTY[view.tab].title}</p>
              <p class="text-body-sm text-[var(--text-muted)] max-w-[360px]">{EMPTY[view.tab].body}</p>
              {#if view.tab !== "all" && tabCounts.all > 0}
                <button type="button" class="tool-btn" onclick={() => setTab("all")}>Show all {tabCounts.all}</button>
              {/if}
            {/if}
          </div>
        {:else}
          <div class="px-2 sm:px-4 py-3 flex flex-col gap-4" role="listbox" aria-label="Inbox" aria-multiselectable="true">
            {#each groups as g (g.key)}
              {@const isCollapsed = collapsed.has(g.key)}
              {@const unreadInGroup = g.entries.filter(unreadOf).length}
              <section data-group={g.key}>
                {#if view.group !== "none"}
                  <h2>
                    <button
                      type="button"
                      class="w-full flex items-center gap-2 px-2 py-1 rounded-md text-left hover:bg-[var(--bg-subtle)] transition-colors"
                      aria-expanded={!isCollapsed}
                      onclick={() => toggleGroup(g.key)}
                    >
                      <ChevronRight size={13} class="shrink-0 text-[var(--text-faint)] transition-transform {isCollapsed ? '' : 'rotate-90'}" />
                      {#if g.projectId !== undefined}
                        <ProjectIcon value={projectEmoji.get(g.projectId) ?? null} size={14} />
                      {:else if g.priority}
                        <PriorityIcon priority={g.priority} size={14} />
                      {:else if g.reason}
                        {@const Icon = REASON_ICON[g.reason]}
                        <Icon size={13} class="text-[var(--text-muted)]" />
                      {/if}
                      <span class="text-caption font-semibold text-[var(--text)] truncate">{g.label}</span>
                      <span class="text-micro tabular-nums text-[var(--text-faint)]">{g.entries.length}</span>
                      {#if unreadInGroup > 0}
                        <span class="text-micro tabular-nums text-[var(--accent)]">{unreadInGroup} unread</span>
                      {/if}
                    </button>
                  </h2>
                {/if}
                {#if !isCollapsed}
                  <div class="mt-1 rounded-lg border border-[var(--border)] bg-[var(--surface)] overflow-visible">
                    {#each g.entries as entry (entry.issue.id)}
                      <InboxRow
                        {entry}
                        projectName={nameOf(entry.issue.project_id)}
                        unread={unreadOf(entry)}
                        active={entry.issue.id === activeId}
                        checked={checked.has(entry.issue.id)}
                        selecting={checked.size > 0}
                        {paneMode}
                        showProject={view.group !== "project"}
                        {today}
                        onSelect={() => (activeId = entry.issue.id)}
                        onOpen={() => open(entry)}
                        onToggleCheck={(shift) => toggleCheck(entry.issue.id, shift)}
                        onToggleRead={() => toggleRead([entry])}
                        onLater={(day) => later([entry], day)}
                        onBringBack={() => entry.later && clearWait(entry.issue.id, entry.later.id)}
                        onDone={() => setStatus([entry], "done")}
                      />
                    {/each}
                  </div>
                {/if}
              </section>
            {/each}
            {#if capped}
              <p class="text-caption text-[var(--text-faint)] px-2">
                Showing the {ATTENTION_LIMIT} most recently updated issues for each reason.
              </p>
            {/if}
          </div>
        {/if}
      </div>

      <!-- The reading pane -->
      {#if paneMode}
        <aside class="w-[min(460px,40%)] shrink-0 border-l border-[var(--border)] bg-[var(--surface)] min-h-0">
          {#if activeEntry}
            {#key activeEntry.issue.id}
              <InboxPane
                entry={activeEntry}
                projectName={nameOf(activeEntry.issue.project_id)}
                unread={unreadOf(activeEntry)}
                {today}
                onOpen={() => open(activeEntry)}
                onToggleRead={() => toggleRead([activeEntry])}
                onLater={(day) => later([activeEntry], day)}
                onClearWait={(waitId) => clearWait(activeEntry.issue.id, waitId)}
                onStatus={(status) => setStatus([activeEntry], status)}
                onClose={() => (view.pane = false)}
              />
            {/key}
          {:else}
            <div class="h-full flex flex-col items-center justify-center gap-3 text-center px-8">
              <Mascot src="/LizzyReading.png" nativeW={487} nativeH={714} scale={0.14} />
              <p class="text-body-sm text-[var(--text-muted)]">Pick an issue to read it here.</p>
            </div>
          {/if}
        </aside>
      {/if}
    </div>

    <!-- Bulk actions for checked rows -->
    {#if checked.size > 0}
      <div class="bulk-in fixed bottom-12 left-1/2 -translate-x-1/2 z-30 flex items-center gap-1 p-1 pl-3 rounded-xl
                  bg-[var(--surface)] border border-[var(--border)] shadow-lg text-body-sm" data-testid="inbox-bulk">
        <span class="font-medium tabular-nums text-[var(--text)] pr-2">{checked.size} selected</span>
        <button type="button" class="bulk-btn" onclick={selectAllShown}>
          {order.every((e) => checked.has(e.issue.id)) ? "None" : "All"}
        </button>
        <span class="w-px h-5 bg-[var(--border)]"></span>
        <button type="button" class="bulk-btn" onclick={() => toggleRead(checkedEntries)}>
          {#if checkedEntries.some(unreadOf)}<MailOpen size={13} /> Read{:else}<Mail size={13} /> Unread{/if}
        </button>
        {#if today}
          <button type="button" class="bulk-btn" onclick={() => void later(checkedEntries, addDays(today, 1))}>
            <Clock size={13} /> Tomorrow
          </button>
        {/if}
        <button type="button" class="bulk-btn" onclick={() => setStatus(checkedEntries, "done")}>
          <CircleCheck size={13} /> Done
        </button>
        <button type="button" class="bulk-btn" aria-label="Clear selection" onclick={() => (checked = new Set())}>
          <X size={13} />
        </button>
      </div>
    {/if}

    <!-- Keyboard hint bar -->
    <div
      class="hint-bar hidden md:[@media(hover:hover)]:flex shrink-0 items-center gap-4 px-6 h-8
             border-t border-[var(--border)] text-micro text-[var(--text-faint)] overflow-hidden"
    >
      <span><kbd class="hint-kbd">J</kbd><kbd class="hint-kbd">K</kbd> move</span>
      <span><kbd class="hint-kbd">Enter</kbd> open</span>
      <span><kbd class="hint-kbd">U</kbd> read</span>
      <span><kbd class="hint-kbd">E</kbd> done</span>
      <span><kbd class="hint-kbd">L</kbd> tomorrow</span>
      <span><kbd class="hint-kbd">X</kbd> select</span>
      <span><kbd class="hint-kbd">1</kbd>…<kbd class="hint-kbd">6</kbd> tabs</span>
      <span><kbd class="hint-kbd">/</kbd> search</span>
      <span><kbd class="hint-kbd">?</kbd> all shortcuts</span>
    </div>
  {/if}
</div>

<style>
  .stat {
    display: inline-flex;
    align-items: center;
    gap: 0.35rem;
    height: 1.5rem;
    padding: 0 0.55rem;
    border-radius: 999px;
    border: 1px solid var(--border);
    color: var(--text-muted);
    transition: background-color 120ms, border-color 120ms, color 120ms;
  }
  .stat b {
    color: var(--text);
    font-weight: 600;
  }
  .stat:hover {
    background: var(--bg-subtle);
  }
  .stat-on {
    border-color: var(--accent);
    background: var(--accent-subtle);
    color: var(--text);
  }
  .tool-btn {
    display: inline-flex;
    align-items: center;
    gap: 0.4rem;
    height: 1.75rem;
    padding: 0 0.6rem;
    border-radius: 0.4rem;
    border: 1px solid var(--border);
    background: var(--surface);
    color: var(--text);
    font-size: var(--text-caption);
    font-weight: 500;
    transition: background-color 120ms;
  }
  .tool-btn:hover:not(:disabled) {
    background: var(--bg-subtle);
  }
  .tool-btn:disabled {
    opacity: 0.45;
  }
  .tool-btn[aria-pressed="true"] {
    color: var(--accent);
  }
  .tab {
    position: relative;
    display: inline-flex;
    align-items: center;
    gap: 0.4rem;
    height: 2.25rem;
    padding: 0 0.7rem;
    flex-shrink: 0;
    font-size: var(--text-body-sm);
    color: var(--text-muted);
    transition: color 120ms;
  }
  .tab:hover {
    color: var(--text);
  }
  .tab-on {
    color: var(--text);
    font-weight: 500;
  }
  .tab-on::after {
    content: "";
    position: absolute;
    left: 0.5rem;
    right: 0.5rem;
    bottom: -1px;
    height: 2px;
    border-radius: 2px;
    background: var(--accent);
  }
  .tab-count {
    font-size: var(--text-micro);
    font-variant-numeric: tabular-nums;
    padding: 0 0.35rem;
    border-radius: 999px;
    background: var(--bg-subtle);
    color: var(--text-muted);
    line-height: 1.1rem;
  }
  .tab-count-hot {
    background: var(--accent);
    color: var(--accent-text);
  }
  .bulk-btn {
    display: inline-flex;
    align-items: center;
    gap: 0.35rem;
    height: 1.85rem;
    padding: 0 0.6rem;
    border-radius: 0.5rem;
    color: var(--text);
    font-size: var(--text-caption);
    font-weight: 500;
  }
  .bulk-btn:hover {
    background: var(--bg-subtle);
  }
  .bulk-in {
    animation: popover-in 160ms var(--ease-out-expo);
  }
  .hint-kbd {
    display: inline-grid;
    place-items: center;
    min-width: 1.15rem;
    height: 1.15rem;
    padding: 0 0.3rem;
    margin-right: 0.3rem;
    border-radius: 4px;
    border: 1px solid var(--border);
    background: var(--bg-subtle);
    color: var(--text-muted);
    font-family: var(--font-mono);
    font-size: 0.625rem;
    line-height: 1;
  }
  .hint-kbd + .hint-kbd {
    margin-left: -0.15rem;
  }
  .hint-bar > span {
    flex-shrink: 0;
  }
  .no-scrollbar {
    scrollbar-width: none;
  }
</style>

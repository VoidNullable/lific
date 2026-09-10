<script lang="ts">
  /*
   * LIF-349 — the phone's navigation surface.
   *
   * This is deliberately NOT the docked sidebar with a `md:` prefix on it.
   * Until now mobile got `Layout.svelte`'s sidebar squeezed into a 230px
   * off-canvas panel: half the screen, 13px rows, ~28px tap targets, and an
   * accordion sub-nav that shoved the project list around while the route
   * you were leaving stayed half-visible behind a scrim. It read as a peek
   * at the desktop UI rather than a place you had gone.
   *
   * So the phone gets its own structure:
   *
   *   - Full-viewport surface. No scrim, nothing of the old route showing.
   *   - Two levels, pushed horizontally the way a native master/detail
   *     navigator does. Root lists projects; tapping one PUSHES to that
   *     project's destinations instead of unfolding them in place. The
   *     push (plus the root pane's parallax drift and dim) is the whole
   *     point: it is what makes picking a project feel like arriving
   *     somewhere in an app that never actually changes documents.
   *   - Swipe to go back a level, or to dismiss from the root.
   *
   * Because the drilldown gives a project's sub-nav its own pane, LIF-272's
   * "first tap expands, second tap navigates" workaround is gone from the
   * docked sidebar too — that compromise only existed because the accordion
   * had nowhere to put a second level.
   */
  import ProjectIcon from "./ProjectIcon.svelte";
  import { onMount, tick, untrack } from "svelte";
  import { mobileNavState } from "./mobileNavState.svelte";
  import { createMobileNavHistory, type MobileNavEntry } from "./mobileNavHistory";
  import { contextMenuState, closeContextMenu } from "./contextMenuState.svelte";
  import {
    Search,
    Home,
    ChevronRight,
    ChevronLeft,
    X,
    Plus,
    Folder,
    Settings,
    Sun,
    Moon,
    Monitor,
    LayoutDashboard,
    List,
    LayoutGrid,
    Layers,
    FileText,
    ListChecks,
    History,
    TrendingUp,
    Waypoints,
    Paperclip,
    Ellipsis,
    ArrowUp,
    ArrowDown,
  } from "lucide-svelte";
  import type { AuthUser, Project, ProjectGroup } from "./api";
  import { NEW_GROUP } from "./projectGroups";
  import type { ThemePreference } from "./theme";

  let {
    open = $bindable(false),
    route,
    navigate,
    user,
    projects,
    groups,
    projectsIn,
    ungrouped,
    collapsedGroups,
    onToggleGroup,
    onOpenPalette,
    onOpenCreateMenu,
    onProjectMenu,
    onGroupMenu,
    editingGroupId = $bindable(),
    draftGroupName = $bindable(),
    onCommitGroupName,
    onCancelGroupEdit,
    groupEditError = "",
    orderError = "",
    onMoveProject,
    onMoveGroup,
    themePref,
    themeResolved,
    onCycleTheme,
  }: {
    open?: boolean;
    route: string;
    navigate: (path: string) => void;
    user: AuthUser;
    projects: Project[];
    groups: ProjectGroup[];
    projectsIn: (group: ProjectGroup) => Project[];
    ungrouped: Project[];
    collapsedGroups: Set<number>;
    onToggleGroup: (id: number) => void;
    onOpenPalette: () => void;
    onOpenCreateMenu: (e: MouseEvent) => void;
    onProjectMenu: (e: MouseEvent, project: Project) => void;
    onGroupMenu: (e: MouseEvent, group: ProjectGroup) => void;
    editingGroupId?: number | null;
    draftGroupName?: string;
    onCommitGroupName: () => void | boolean | Promise<void | boolean>;
    onCancelGroupEdit: () => void;
    groupEditError?: string;
    orderError?: string;
    onMoveProject?: (project: Project, direction: "up" | "down") => void | Promise<void>;
    onMoveGroup?: (group: ProjectGroup, direction: "up" | "down") => void | Promise<void>;
    themePref: ThemePreference;
    themeResolved: "light" | "dark";
    onCycleTheme: (event: MouseEvent) => void;
  } = $props();

  // ── Mount / visibility ──────────────────────────────────────
  // Mounted lazily on first open so desktop sessions never pay for a second
  // copy of the project tree, then kept mounted (parked off-canvas) so
  // reopening is a transform rather than a re-render. `shown` trails `open`
  // by a frame on that first open, which is what gives the initial slide
  // something to animate from.
  let mounted = $state(false);
  let shown = $state(false);
  let panelEl = $state<HTMLElement | null>(null);
  let panelWidth = $state(0);
  let restoreFocusTo: HTMLElement | null = null;
  let projectTrigger: HTMLElement | null = null;
  let rootEl = $state<HTMLDivElement>(null!);
  let projectEl = $state<HTMLDivElement>(null!);
  let controller: ReturnType<typeof createMobileNavHistory> | undefined;
  let desktop: MediaQueryList;
  let historyOpen = false;
  let lastRoute: string;
  let presentation = 0;

  // Opening at a level other than the one we closed at has to LAND there,
  // not animate there: otherwise the outgoing pane visibly slides away while
  // the panel is still sliding in. `paneSnap` freezes the pane transition for
  // exactly one painted frame so the new level is already in place by the
  // time the panel becomes visible. It needs two frames to be reliable —
  // one to paint the snapped position, one to re-arm the transition — since
  // a single style recalc that both moves the pane and re-enables its
  // transition would still start one.
  let paneSnap = $state(false);

  $effect(() => {
    if (!open) {
      shown = false;
      return;
    }
    mounted = true;
    let inner = 0;
    const outer = requestAnimationFrame(() => {
      inner = requestAnimationFrame(() => {
        paneSnap = false;
        shown = true;
      });
    });
    return () => {
      cancelAnimationFrame(outer);
      cancelAnimationFrame(inner);
    };
  });

  function focusPane() {
    const pane = level === 1 ? projectEl : rootEl;
    pane?.querySelector<HTMLElement>("button, a[href]")?.focus();
  }

  function present(entry: MobileNavEntry | null) {
    const revision = ++presentation;
    const wasOpen = historyOpen;
    const oldLevel = level;
    if (entry && desktop?.matches) { controller?.close(); return; }
    if (entry && !wasOpen) {
      const active = document.activeElement;
      // A restored drawer has no click trigger. Return to the existing
      // mobile header control rather than attempting to focus <body>.
      restoreFocusTo = active instanceof HTMLElement && active !== document.body && active !== document.documentElement
        ? active : document.querySelector<HTMLElement>('button[aria-label="Open navigation"]');
      paneSnap = true;
    }
    if (entry?.project) {
      viewIdentifier = entry.project;
    }
    level = entry?.depth === 2 ? 1 : 0;
    historyOpen = !!entry;
    open = !!entry;
    mobileNavState.open = open;
    if (!open) closeContextMenu();
    void tick().then(() => {
      if (revision !== presentation) return;
      if (open) {
        if (contextMenuState.open) return;
        const row = projectTrigger?.isConnected ? projectTrigger :
          [...(rootEl?.querySelectorAll<HTMLElement>("[data-mobile-project-trigger]") ?? [])]
            .find((element) => element.dataset.mobileProjectTrigger === viewProject?.identifier);
        if (wasOpen && oldLevel === 1 && level === 0 && row && !row.closest("[inert]")) {
          row.focus();
        } else focusPane();
      } else if (restoreFocusTo) {
        const target = restoreFocusTo;
        restoreFocusTo = null;
        if (target.isConnected && !target.closest("[inert]") && target.getClientRects().length) target.focus();
      }
    });
  }

  onMount(() => {
    desktop = matchMedia("(min-width: 768px)");
    controller = createMobileNavHistory(present);
    if (!controller.restore() && open) controller.open(null);
    const resize = () => { if (desktop.matches) close(); };
    desktop.addEventListener("change", resize);
    return () => {
      controller?.destroy();
      mobileNavState.open = false;
      desktop.removeEventListener("change", resize);
    };
  });

  // Keep bind:open compatible, but all closures still unwind owned entries.
  $effect(() => {
    const requested = open;
    untrack(() => {
      if (requested !== historyOpen) {
        if (requested) controller?.open(null);
        else controller?.close();
      }
    });
  });
  $effect(() => {
    const next = route;
    untrack(() => {
      if (lastRoute !== undefined && lastRoute !== next) controller?.routeChanged();
      lastRoute = next;
    });
  });

  // Inert the background, not the context menu mounted beside this dialog.
  // Observe sibling insertion too, since the menu mounts lazily.
  $effect(() => {
    if (!open || !panelEl) return;
    const saved = new Map<HTMLElement, boolean>();
    const modal = panelEl;
    function isolate() {
      let child: HTMLElement = modal;
      while (child.parentElement) {
        for (const sibling of child.parentElement.children) {
          if (!(sibling instanceof HTMLElement) || sibling === child || sibling.matches('[role="menu"], script, style')) continue;
          if (!saved.has(sibling)) saved.set(sibling, sibling.inert);
          sibling.inert = true;
        }
        child = child.parentElement;
        if (child === document.body) break;
      }
    }
    isolate();
    const observer = new MutationObserver(isolate);
    observer.observe(document.body, { childList: true, subtree: true });
    return () => {
      observer.disconnect();
      for (const [element, inert] of saved) element.inert = inert;
    };
  });

  // ── Levels ──────────────────────────────────────────────────
  // 0 = project list, 1 = one project's destinations. `viewProject` is kept
  // populated after popping back so the outgoing pane still has content to
  // animate with; at level 0 it is inert and unreachable.
  let level = $state(0);
  let viewIdentifier = $state<string | null>(null);
  let viewProject = $derived(projects.find((p) => p.identifier === viewIdentifier) ?? null);

  /** Open the nav, optionally landing straight on a project's pane. */
  export function openAt(project: Project | null) {
    projectTrigger = null;
    controller?.open(project?.identifier ?? null);
  }

  export function close() {
    controller?.close();
  }

  /** Use for destinations supplied by parent-owned menus as well. */
  export function navigateTo(path: string) {
    controller?.close(() => { if (route !== path) navigate(path); });
  }

  function push(project: Project) {
    projectTrigger = document.activeElement as HTMLElement | null;
    controller?.open(project.identifier);
  }

  function pop() {
    controller?.back();
  }

  function go(event: MouseEvent, path: string) {
    if (event.button !== 0 || event.metaKey || event.ctrlKey || event.shiftKey || event.altKey) return;
    event.preventDefault();
    navigateTo(path);
  }

  // ── Swipe ───────────────────────────────────────────────────
  // One gesture, two meanings, each matching the direction the surface
  // arrived from: on a project pane a rightward drag pops back, on the root
  // a leftward drag dismisses. Nothing is claimed until horizontal movement
  // clearly beats vertical, so flicking through a long project list still
  // scrolls (the panes are `touch-action: pan-y`, so the browser keeps
  // vertical panning and only hands us the horizontal component).
  let dragging = $state(false);
  let dragProgress = $state(0); // level position while dragging, 0..1
  let dragShift = $state(0); // px the whole panel is pulled left by
  let pending = false;
  let startX = 0;
  let startY = 0;
  let startTime = 0;
  let activePointer: number | null = null;

  const CLAIM_SLOP = 12; // px of travel before a gesture is ours
  const COMMIT_RATIO = 0.28; // fraction of the width that counts as "far enough"
  const COMMIT_VELOCITY = 0.45; // px/ms that counts as a flick

  function onPointerDown(e: PointerEvent) {
    if (e.pointerType === "mouse" || !open) return;
    pending = true;
    activePointer = e.pointerId;
    startX = e.clientX;
    startY = e.clientY;
    startTime = e.timeStamp;
  }

  function onPointerMove(e: PointerEvent) {
    if (activePointer !== e.pointerId) return;
    const dx = e.clientX - startX;
    const dy = e.clientY - startY;

    if (pending) {
      if (Math.abs(dy) > CLAIM_SLOP && Math.abs(dy) > Math.abs(dx)) {
        // Settled into a vertical scroll — leave it to the browser.
        activePointer = null;
        pending = false;
        return;
      }
      if (Math.abs(dx) < CLAIM_SLOP || Math.abs(dx) <= Math.abs(dy) * 1.2) return;
      // Only the direction that means "back" for the current level counts.
      if (level === 1 ? dx <= 0 : dx >= 0) {
        activePointer = null;
        pending = false;
        return;
      }
      pending = false;
      dragging = true;
      dragProgress = level;
      dragShift = 0;
      panelEl?.setPointerCapture(e.pointerId);
    }

    if (!dragging) return;
    const width = panelWidth || window.innerWidth;
    if (level === 1) {
      dragProgress = Math.min(1, Math.max(0, 1 - dx / width));
    } else {
      dragShift = Math.min(width, Math.max(0, -dx));
    }
  }

  function onPointerUp(e: PointerEvent) {
    if (activePointer !== e.pointerId) return;
    activePointer = null;
    pending = false;
    if (!dragging) return;

    const width = panelWidth || window.innerWidth;
    const dx = e.clientX - startX;
    const elapsed = Math.max(1, e.timeStamp - startTime);
    const velocity = Math.abs(dx) / elapsed;
    const travelled = Math.abs(dx) / width;
    const commit = travelled > COMMIT_RATIO || velocity > COMMIT_VELOCITY;

    dragging = false;
    dragProgress = 0;
    dragShift = 0;
    if (!commit) return;
    if (level === 1) pop();
    else close();
  }

  function onPointerCancel(e: PointerEvent) {
    if (activePointer !== e.pointerId) return;
    activePointer = null;
    pending = false;
    dragging = false;
    dragProgress = 0;
    dragShift = 0;
  }

  // ── Derived presentation ────────────────────────────────────
  let progress = $derived(dragging ? dragProgress : level);
  // The root pane drifts a quarter-width left and dims as the project pane
  // covers it; that depth cue is what reads as "pushed" rather than "swapped".
  let rootStyle = $derived(
    `transform: translateX(${(-25 * progress).toFixed(3)}%); opacity: ${(1 - 0.45 * progress).toFixed(3)}`,
  );
  let projectStyle = $derived(
    `transform: translateX(${(100 * (1 - progress)).toFixed(3)}%)`,
  );
  // Open/closed and drag-to-dismiss both live in this one `transform`, in a
  // single calc(). Tailwind's `-translate-x-full` would have written the
  // `translate` property instead, and a swipe that commits to a close would
  // then have snapped the panel back to 0 (transform reset) before animating
  // out (translate 0 → -100%) — a visible bounce right at the moment the
  // gesture is supposed to feel continuous.
  let panelStyle = $derived(
    `transform: translateX(calc(${shown ? "0%" : "-100%"} - ${dragShift}px))`,
  );

  function isActive(path: string): boolean {
    return route === path || route.startsWith(path + "/");
  }

  // Which project the current route belongs to, so its row reads as current
  // in the list. Matched case-insensitively, mirroring the route matcher.
  let activeIdentifier = $derived(
    route.match(/^\/([A-Za-z][A-Za-z0-9_-]*)\//)?.[1]?.toLowerCase() ?? null,
  );

  // Escape steps back one level before it dismisses, so a mis-tapped project
  // costs one key rather than the whole navigation.
  function onKeydown(e: KeyboardEvent) {
    if (!open || contextMenuState.open || e.defaultPrevented) return;
    if (e.key === "Escape") {
      e.preventDefault();
      e.stopPropagation();
      if (editingGroupId != null) cancelEdit();
      else if (level === 1) pop();
      else close();
    } else if (e.key === "Tab") {
      const pane = level === 1 ? projectEl : rootEl;
      const items = [...pane.querySelectorAll<HTMLElement>('button:not(:disabled), a[href], input:not(:disabled), [tabindex="0"]')]
        .filter((el) => !el.closest("[inert]") && el.getClientRects().length);
      const first = items[0], last = items.at(-1);
      if (!first) { e.preventDefault(); panelEl?.focus(); return; }
      if (e.shiftKey ? document.activeElement === first || !pane.contains(document.activeElement) : document.activeElement === last || !pane.contains(document.activeElement)) {
        e.preventDefault();
        (e.shiftKey ? last : first)?.focus();
      }
    }
  }

  function onFocusin(e: FocusEvent) {
    if (!open || contextMenuState.open) return;
    const pane = level === 1 ? projectEl : rootEl;
    if (e.target instanceof Node && !pane?.contains(e.target)) focusPane();
  }

  let editInput = $state<HTMLInputElement>(null!);
  let editReturn: HTMLElement | null = null;
  let editReturnLabel: string | null = null;
  let editReturnGroup: number | null = null;
  let editBusy = $state(false);
  let editError = $state("");
  $effect(() => {
    if (!open || editingGroupId == null) return;
    untrack(() => {
      editError = "";
      editReturn = document.activeElement instanceof HTMLElement && document.activeElement.closest("[data-mobile-nav]")
        ? document.activeElement : editReturn;
      void tick().then(() => { editInput?.focus(); editInput?.select(); });
    });
  });
  function restoreEditFocus() {
    void tick().then(() => {
      if (editReturn?.isConnected && !editReturn.closest("[inert]")) editReturn.focus();
      else {
        const replacement = (editReturnGroup != null ? rootEl?.querySelector<HTMLElement>(`[data-mobile-group-actions="${editReturnGroup}"]`) : null) ?? [...(rootEl?.querySelectorAll<HTMLElement>("button[aria-label]") ?? [])]
          .find((button) => button.getAttribute("aria-label") === editReturnLabel);
        if (replacement) replacement.focus();
        else focusPane();
      }
    });
  }
  function cancelEdit() {
    if (editBusy) return;
    onCancelGroupEdit();
    editError = "";
    restoreEditFocus();
  }
  async function saveEdit() {
    if (editBusy) return;
    if (!draftGroupName?.trim()) { editError = "Enter a group name."; editInput?.focus(); return; }
    editBusy = true;
    editError = "";
    try {
      const result = await onCommitGroupName();
      if (result === false || editingGroupId != null) {
        editError = groupEditError || "Couldn't save the group. Try again.";
        void tick().then(() => editInput?.focus());
      } else restoreEditFocus();
    } catch (error) {
      editError = error instanceof Error ? error.message : "Couldn't save the group. Try again.";
      void tick().then(() => editInput?.focus());
    } finally { editBusy = false; }
  }

  function projectMenu(e: MouseEvent, project: Project) {
    editReturn = e.currentTarget as HTMLElement;
    editReturnLabel = editReturn.getAttribute("aria-label");
    editReturnGroup = null;
    onProjectMenu(e, project);
    anchorMenu(e);
    if (onMoveProject) {
      const group = groups.find((g) => g.project_ids.includes(project.id));
      const ordered = group ? projectsIn(group) : ungrouped;
      contextMenuState.items = [...contextMenuState.items.filter((item) => item.label !== "Move up" && item.label !== "Move down"),
        { label: "Move up", icon: ArrowUp, disabled: ordered[0]?.id === project.id, action: () => void onMoveProject?.(project, "up") },
        { label: "Move down", icon: ArrowDown, disabled: ordered.at(-1)?.id === project.id, action: () => void onMoveProject?.(project, "down") },
      ];
    }
  }
  function groupMenu(e: MouseEvent, group: ProjectGroup) {
    editReturn = e.currentTarget as HTMLElement;
    editReturnLabel = editReturn.getAttribute("aria-label");
    editReturnGroup = group.id;
    onGroupMenu(e, group);
    anchorMenu(e);
    if (onMoveGroup) {
      contextMenuState.items = [...contextMenuState.items.filter((item) => item.label !== "Move up" && item.label !== "Move down"),
        { label: "Move up", icon: ArrowUp, disabled: groups[0]?.id === group.id, action: () => void onMoveGroup?.(group, "up") },
        { label: "Move down", icon: ArrowDown, disabled: groups.at(-1)?.id === group.id, action: () => void onMoveGroup?.(group, "down") },
      ];
    }
  }

  function anchorMenu(e: MouseEvent) {
    if (e.type !== "click") return;
    const rect = (e.currentTarget as HTMLElement).getBoundingClientRect();
    contextMenuState.x = rect.left;
    contextMenuState.y = rect.bottom;
  }

  function createMenu(e: MouseEvent) {
    editReturn = e.currentTarget as HTMLElement;
    editReturnLabel = editReturn.getAttribute("aria-label");
    editReturnGroup = null;
    onOpenCreateMenu(e);
  }

  function initials(name: string): string {
    return name
      .split(/[\s_-]+/)
      .slice(0, 2)
      .map((w) => w[0]?.toUpperCase() ?? "")
      .join("");
  }

  // Destination rows for a project's pane, in the same order the docked
  // sidebar lists them so muscle memory survives the switch between devices.
  const destinations: { slug: string; label: string; icon: typeof List }[] = [
    { slug: "overview", label: "Overview", icon: LayoutDashboard },
    { slug: "issues", label: "Issues", icon: List },
    { slug: "board", label: "Board", icon: LayoutGrid },
    { slug: "graph", label: "Graph", icon: Waypoints },
    { slug: "modules", label: "Modules", icon: Layers },
    { slug: "pages", label: "Pages", icon: FileText },
    { slug: "files", label: "Files", icon: Paperclip },
    { slug: "plans", label: "Plans", icon: ListChecks },
    { slug: "activity", label: "Activity", icon: History },
    { slug: "insights", label: "Insights", icon: TrendingUp },
  ];
</script>

<svelte:window onkeydown={onKeydown} onfocusin={onFocusin} />

{#if mounted}
  <!-- svelte-ignore a11y_no_noninteractive_element_interactions -->
  <div
    bind:this={panelEl}
    bind:clientWidth={panelWidth}
    data-mobile-nav
    class="md:hidden fixed inset-0 z-[60] flex flex-col overflow-hidden bg-[var(--chrome)]
           ease-[var(--ease-out-expo)] focus:outline-none
           {dragging ? 'transition-none' : 'transition-transform duration-300'}
           {open ? '' : 'pointer-events-none'}"
    style={panelStyle}
    role="dialog"
    aria-modal={!contextMenuState.open}
    aria-label="Navigation"
    aria-hidden={!open}
    inert={!open}
    tabindex="-1"
    onpointerdown={onPointerDown}
    onpointermove={onPointerMove}
    onpointerup={onPointerUp}
    onpointercancel={onPointerCancel}
  >
    <!-- ── Level 0: everything you can navigate to ───────────── -->
    <div
      bind:this={rootEl}
      data-mobile-root
      class="absolute inset-0 flex flex-col
             {dragging || paneSnap ? 'transition-none' : 'transition-[transform,opacity] duration-300'}
             ease-[var(--ease-out-expo)]
             {level === 1 && !dragging ? 'pointer-events-none' : ''}"
      style={rootStyle}
      aria-hidden={!open || level === 1}
      inert={!open || level === 1}
    >
      <div
        class="shrink-0 flex items-center gap-2 px-3 pt-[max(0.75rem,env(safe-area-inset-top))] pb-2"
      >
        <img src="/logo.webp" alt="" width="28" height="28" class="rounded-md shrink-0" />
        <span class="font-display text-heading tracking-tight text-[var(--text)] leading-none flex-1">
          Lific
        </span>
        <span
          class="font-mono text-micro tracking-tight text-[var(--text-faint)]
                 px-1.5 py-0.5 rounded-md bg-[var(--bg-subtle)]"
        >
          v{__APP_VERSION__}
        </span>
        <button
          class="size-11 -mr-2 shrink-0 grid place-items-center rounded-lg
                 text-[var(--text-muted)] active:bg-[var(--bg-subtle)] transition-colors"
          aria-label="Close navigation"
          onclick={close}
        >
          <X size={20} />
        </button>
      </div>

      <div class="shrink-0 px-3 pb-2">
        <button
          class="w-full h-11 flex items-center gap-2.5 px-3 rounded-xl
                 bg-[var(--bg)] shadow-[inset_0_1px_2px_rgba(0,0,0,0.08)]
                 text-[var(--text-muted)] active:bg-[var(--bg-subtle)] transition-colors"
          onclick={() => {
            controller?.close(onOpenPalette);
          }}
        >
          <Search size={16} class="shrink-0" />
          <span class="flex-1 text-left text-body">Search issues, pages, projects…</span>
        </button>
      </div>

      <nav class="flex-1 min-h-0 overflow-y-auto overscroll-contain px-2 pb-3">
        <a
          class="w-full min-h-12 flex items-center gap-3 px-3 rounded-xl text-left text-body
                 transition-colors
                 {isActive('/')
            ? 'text-[var(--text)] bg-[var(--bg-subtle)] font-medium'
            : 'text-[var(--text-muted)] active:bg-[var(--bg-subtle)]'}"
          href="#/"
          aria-current={route === "/" ? "page" : undefined}
          onclick={(e) => go(e, "/")}
        >
          <Home size={18} class="shrink-0 {isActive('/') ? 'text-[var(--accent)]' : ''}" />
          Home
        </a>

        <div class="flex items-center justify-between pl-3 pr-1 pt-4 pb-1">
          <span class="text-micro font-semibold uppercase tracking-widest text-[var(--text-faint)]">
            Projects
          </span>
          <button
            class="size-11 -mr-1 grid place-items-center rounded-lg
                   text-[var(--text-faint)] active:bg-[var(--bg-subtle)] transition-colors"
            aria-label="New project or group"
            onclick={createMenu}
          >
            <Plus size={18} />
          </button>
        </div>

        {#if orderError}
          <p role="alert" class="px-3 py-2 text-body-sm text-[var(--error)] break-words">{orderError}</p>
        {/if}

        <!-- One project row. Chevron means "this pushes", matching the
             pane it opens. Long-press still raises the group context menu,
             same as right-click on the desktop sidebar. -->
        {#snippet projectRow(project: Project)}
          <div class="flex items-center">
          <button
            class="flex-1 min-w-0 min-h-[52px] flex items-center gap-3 px-3 rounded-xl text-left
                   transition-colors
                   {project.identifier.toLowerCase() === activeIdentifier
              ? 'bg-[var(--bg-subtle)]'
              : 'active:bg-[var(--bg-subtle)]'}"
            onclick={() => push(project)}
            data-mobile-project-trigger={project.identifier}
            aria-label="Open {project.name} navigation"
            oncontextmenu={(e) => projectMenu(e, project)}
          >
            {#if project.emoji}
              <span class="size-8 rounded-lg bg-[var(--bg-subtle)] grid place-items-center shrink-0">
                <ProjectIcon value={project.emoji} size={18} />
              </span>
            {:else}
              <span
                class="size-8 rounded-lg border border-[var(--border)] bg-[var(--bg-subtle)]
                       grid place-items-center text-caption font-semibold tracking-tight
                       shrink-0 text-[var(--text-muted)]"
              >
                {project.identifier.slice(0, 2)}
              </span>
            {/if}
            <span class="flex-1 min-w-0">
              <span class="block truncate text-body text-[var(--text)]">{project.name}</span>
              <span class="block font-mono text-micro text-[var(--text-faint)]">
                {project.identifier}
              </span>
            </span>
            <ChevronRight size={17} class="shrink-0 text-[var(--text-faint)]" />
          </button>
          <button class="size-11 shrink-0 grid place-items-center rounded-lg text-[var(--text-muted)] active:bg-[var(--bg-subtle)]" aria-label="Actions for {project.name}" aria-haspopup="menu" onclick={(e) => projectMenu(e, project)}><Ellipsis size={18} /></button>
          </div>
        {/snippet}

        {#snippet groupNameInput()}
          <!-- 16px on purpose: anything smaller and iOS Safari zooms the
               viewport on focus (LIF-271). -->
          <input
            bind:this={editInput}
            class="w-full h-11 px-3 my-1 rounded-xl text-[16px] bg-[var(--bg)]
                   border border-[var(--border)] text-[var(--text)]"
            placeholder="Group name"
            aria-label="Group name"
            aria-invalid={!!(editError || groupEditError)}
            aria-describedby={editError || groupEditError ? "mobile-group-error" : undefined}
            disabled={editBusy}
            bind:value={draftGroupName}
            onkeydown={(e) => {
              if (e.key !== "Enter" && e.key !== "Escape") return;
              e.preventDefault();
              e.stopPropagation();
              if (e.key === "Enter") void saveEdit();
              else cancelEdit();
            }}
          />
          {#if editError || groupEditError}<p id="mobile-group-error" role="alert" class="px-3 text-body-sm text-[var(--error)]">{editError || groupEditError}</p>{/if}
          <div class="flex gap-2 px-3 pb-2">
            <button class="min-h-11 px-3 rounded-lg bg-[var(--accent)] text-[var(--accent-text)]" disabled={editBusy} onclick={saveEdit}>{editBusy ? "Saving…" : "Save"}</button>
            <button class="min-h-11 px-3 rounded-lg active:bg-[var(--bg-subtle)]" disabled={editBusy} onclick={cancelEdit}>Cancel</button>
          </div>
        {/snippet}

        {#if editingGroupId === NEW_GROUP}
          {@render groupNameInput()}
        {/if}

        {#each groups as group (group.id)}
          {@const collapsed = collapsedGroups.has(group.id)}
          {#if editingGroupId === group.id}
            {@render groupNameInput()}
          {:else}
            <div class="flex items-center">
            <button
              class="flex-1 min-w-0 min-h-11 flex items-center gap-2 px-3 rounded-xl text-left
                     text-body-sm font-medium uppercase tracking-wide
                     text-[var(--text-muted)] active:bg-[var(--bg-subtle)] transition-colors"
              aria-expanded={!collapsed}
              onclick={() => onToggleGroup(group.id)}
              oncontextmenu={(e) => groupMenu(e, group)}
            >
              <ChevronRight
                size={15}
                class="shrink-0 transition-transform text-[var(--text-faint)]
                       {collapsed ? '' : 'rotate-90'}"
              />
              <Folder size={15} class="shrink-0 text-[var(--text-faint)]" />
              <span class="truncate flex-1 normal-case tracking-normal">{group.name}</span>
            </button>
            <button data-mobile-group-actions={group.id} class="size-11 shrink-0 grid place-items-center rounded-lg text-[var(--text-muted)] active:bg-[var(--bg-subtle)]" aria-label="Actions for {group.name}" aria-haspopup="menu" onclick={(e) => groupMenu(e, group)}><Ellipsis size={18} /></button>
            </div>
          {/if}
          {#if !collapsed}
            <div class="ml-4 pl-1 border-l border-[var(--border)]">
              {#each projectsIn(group) as project (project.id)}
                {@render projectRow(project)}
              {/each}
            </div>
          {/if}
        {/each}

        {#each ungrouped as project (project.id)}
          {@render projectRow(project)}
        {/each}

        {#if projects.length === 0 && groups.length === 0 && editingGroupId !== NEW_GROUP}
          <div class="px-3 py-8">
            <p class="text-body text-[var(--text-faint)] mb-3">No projects yet.</p>
            <a
              class="min-h-11 px-4 rounded-xl bg-[var(--accent)] text-[var(--accent-text)] text-body font-medium"
              href="#/projects/new"
              onclick={(e) => go(e, "/projects/new")}
            >
              Create a project
            </a>
          </div>
        {/if}
      </nav>

      <div
        class="shrink-0 flex items-center gap-1 p-2 pb-[max(0.5rem,env(safe-area-inset-bottom))]
               border-t border-[var(--border)]"
      >
        <a
          class="flex-1 min-w-0 min-h-12 flex items-center gap-3 px-2 rounded-xl text-left
                 transition-colors
                 {isActive('/settings') ? 'bg-[var(--bg-subtle)]' : 'active:bg-[var(--bg-subtle)]'}"
          href="#/settings"
          aria-current={isActive("/settings") ? "page" : undefined}
          onclick={(e) => go(e, "/settings")}
        >
          <div
            class="size-9 rounded-full bg-[var(--accent)] text-[var(--accent-text)]
                   grid place-items-center text-caption font-semibold tracking-wide
                   select-none shrink-0"
          >
            {initials(user.display_name || user.username)}
          </div>
          <div class="flex-1 min-w-0">
            <div class="text-body text-[var(--text)] truncate leading-tight">
              {user.display_name || user.username}
            </div>
            <div class="text-micro text-[var(--text-faint)] flex items-center gap-1 leading-tight mt-0.5">
              <Settings size={10} /> Settings
            </div>
          </div>
        </a>
        <button
          class="size-11 shrink-0 grid place-items-center rounded-xl
                 text-[var(--text-muted)] active:bg-[var(--bg-subtle)] transition-colors"
          onclick={onCycleTheme}
          aria-label="Choose theme, current: {themePref}"
          title="Choose theme, current: {themePref}"
          aria-haspopup="menu"
        >
          {#if themePref === "system"}
            <Monitor size={18} />
          {:else if themeResolved === "dark"}
            <Moon size={18} />
          {:else}
            <Sun size={18} />
          {/if}
        </button>
      </div>
    </div>

    <!-- ── Level 1: one project's destinations ───────────────── -->
    <div
      bind:this={projectEl}
      data-mobile-project
      class="absolute inset-0 flex flex-col bg-[var(--chrome)]
             shadow-[-10px_0_28px_rgba(0,0,0,0.12)]
             {dragging || paneSnap ? 'transition-none' : 'transition-transform duration-300'}
             ease-[var(--ease-out-expo)]
             {level === 0 && !dragging ? 'pointer-events-none' : ''}"
      style={projectStyle}
      aria-hidden={!open || level === 0}
      inert={!open || level === 0}
    >
      {#if viewProject}
        {@const project = viewProject}
        <div
          class="shrink-0 pt-[max(0.5rem,env(safe-area-inset-top))] border-b border-[var(--border)]"
        >
          <div class="flex items-center gap-1 px-1">
            <button
              class="min-h-11 pl-1 pr-3 flex items-center gap-0.5 rounded-lg
                     text-body text-[var(--accent)] active:bg-[var(--bg-subtle)] transition-colors"
              onclick={pop}
            >
              <ChevronLeft size={20} class="shrink-0" />
              Projects
            </button>
            <div class="flex-1"></div>
            <button
              class="size-11 shrink-0 grid place-items-center rounded-lg
                     text-[var(--text-muted)] active:bg-[var(--bg-subtle)] transition-colors"
              aria-label="Close navigation"
              onclick={close}
            >
              <X size={20} />
            </button>
          </div>
          <div class="flex items-center gap-3 px-4 pt-1 pb-4">
            {#if project.emoji}
              <span class="size-11 rounded-xl bg-[var(--bg-subtle)] grid place-items-center shrink-0">
                <ProjectIcon value={project.emoji} size={24} />
              </span>
            {:else}
              <span
                class="size-11 rounded-xl border border-[var(--border)] bg-[var(--bg-subtle)]
                       grid place-items-center text-body font-semibold tracking-tight
                       shrink-0 text-[var(--text-muted)]"
              >
                {project.identifier.slice(0, 2)}
              </span>
            {/if}
            <div class="min-w-0 flex-1">
              <h2 class="font-display text-title tracking-tight text-[var(--text)] truncate leading-tight">
                {project.name}
              </h2>
              <p class="font-mono text-caption text-[var(--text-faint)] leading-tight mt-0.5">
                {project.identifier}
              </p>
            </div>
          </div>
        </div>

        <nav
          class="flex-1 min-h-0 overflow-y-auto overscroll-contain px-2
                 pb-[max(0.75rem,env(safe-area-inset-bottom))]"
        >
          {#each destinations as dest (dest.slug)}
            {@const href = `/${project.identifier}/${dest.slug}`}
            {@const active = isActive(href)}
            <a
              class="w-full min-h-[52px] flex items-center gap-3 px-3 rounded-xl text-left text-body
                     transition-colors
                     {active
                ? 'text-[var(--text)] bg-[var(--bg-subtle)] font-medium'
                : 'text-[var(--text-muted)] active:bg-[var(--bg-subtle)]'}"
              href={"#" + href}
              aria-current={active ? "page" : undefined}
              onclick={(e) => go(e, href)}
            >
              <dest.icon size={18} class="shrink-0 {active ? 'text-[var(--accent)]' : ''}" />
              <span class="flex-1">{dest.label}</span>
              {#if active}
                <span class="size-1.5 rounded-full bg-[var(--accent)] shrink-0"></span>
              {/if}
            </a>
          {/each}
        </nav>
      {:else if level === 1}
        <div class="shrink-0 flex items-center justify-between px-2 pt-[max(0.5rem,env(safe-area-inset-top))] border-b border-[var(--border)]">
          <button class="min-h-11 flex items-center gap-0.5 px-2 rounded-lg text-body text-[var(--accent)] active:bg-[var(--bg-subtle)]" onclick={pop}>
            <ChevronLeft size={20} /> Projects
          </button>
          <button class="size-11 grid place-items-center rounded-lg text-[var(--text-muted)] active:bg-[var(--bg-subtle)]" aria-label="Close navigation" onclick={close}><X size={20} /></button>
        </div>
        <div class="p-4">
          <h2 class="font-display text-title text-[var(--text)]">Project unavailable</h2>
          <p class="mt-2 text-body text-[var(--text-muted)]">This project is no longer in your project list.</p>
        </div>
      {/if}
    </div>
  </div>
{/if}

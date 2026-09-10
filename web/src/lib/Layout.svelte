<script lang="ts">
  import {
    me,
    clearSession,
    listProjects,
    reorderProjects,
    reorderProjectGroups,
    listIssues,
    listModules,
    listPages,
    listPlans,
    listProjectGroups,
    createProjectGroup,
    renameProjectGroup,
    deleteProjectGroup,
    assignProjectGroup,
    type Project,
    type ProjectGroup,
    type Issue,
    type Module,
    type Page,
    type Plan,
  } from "./api";
  import { loadCollapsedGroups, saveCollapsedGroups, NEW_GROUP } from "./projectGroups";
  import ProjectIcon from "./ProjectIcon.svelte";
  import MobileNav from "./MobileNav.svelte";
  import CommandPalette from "./CommandPalette.svelte";
  import ShortcutHelp from "./ShortcutHelp.svelte";
  import { dndzone, type DndEvent } from "svelte-dnd-action";
  import { flip } from "svelte/animate";
  import { themePreference, resolvedTheme, setPreference, motionReduced } from "./theme";
  import { currentUser, getUserRevision, publishUser } from "./userState";
  import { mobileNavState } from "./mobileNavState.svelte";
  import { navLink } from "./navLink";
  import { Settings, List, LayoutGrid, FileText, Plus, Layers, History, ListChecks, LayoutDashboard, Search, ChevronRight, Sun, Moon, Monitor, Menu, Home, TrendingUp, HelpCircle, Folder, FolderPlus, FolderMinus, Pencil, Trash2, PanelLeftClose, PanelLeftOpen, Waypoints, Paperclip } from "lucide-svelte";
  import { onDestroy, setContext, untrack, tick } from "svelte";
  import { Ellipsis, ArrowUp, ArrowDown } from "lucide-svelte";
  import { peekState } from "./issues/peek.svelte";
  import PeekPanel from "./issues/PeekPanel.svelte"; // LIF-248: hoisted here so it's available on every route
  import PagePeekPanel from "./pages/PagePeekPanel.svelte"; // pages sibling of PeekPanel, same reasoning
  import { contextMenuState, openContextMenu } from "./contextMenuState.svelte";
  import { toast } from "./toast/toast.svelte";
  import ContextMenu from "./ContextMenu.svelte"; // LIF-248
  import { commandPaletteState } from "./commandPaletteState.svelte";
  import { toggleShortcutHelp } from "./shortcutHelpState.svelte";
  import { isTypingContext } from "./shortcuts";
  import { loadProjectRole } from "./projectRole.svelte"; // LIF-234
  import { startAutoRefresh } from "./autoRefresh.svelte";
  import {
    clampSidebarWidth,
    loadSidebarWidth,
    saveSidebarWidth,
    loadSidebarCollapsed,
    saveSidebarCollapsed,
    SIDEBAR_DEFAULT_WIDTH,
    SIDEBAR_MAX_WIDTH,
    SIDEBAR_MIN_WIDTH,
  } from "./sidebarWidth";

  // Ref to the command palette so the sidebar's "Jump to…" affordance can
  // summon it (LIF-192).
  let palette = $state<{ openPalette: () => void } | null>(null);

  // LIF-349: below md, navigation is a separate full-screen surface
  // (MobileNav) rather than this sidebar squeezed into a 230px drawer.
  // Layout still owns the open flag so route changes can dismiss it, and
  // holds the instance so the header can open it already drilled into the
  // current project.
  let navOpen = $state(false);
  let mobileNav = $state<{ openAt: (p: Project | null) => void; navigateTo: (path: string) => void } | null>(null);

  // LIF-309: only the md+ docked sidebar is resizable; the mobile drawer
  // always remains 230px. Width changes stay in memory until a drag ends.
  let sidebarWidth = $state(loadSidebarWidth());
  let sidebarResizing = $state(false);
  let sidebarPointerId: number | null = null;
  let sidebarDragStartX = 0;
  let sidebarDragStartWidth = SIDEBAR_DEFAULT_WIDTH;
  let previousBodyCursor = "";
  let previousBodyUserSelect = "";
  let sidebarBodyStylesApplied = false;

  function restoreSidebarResizeStyles() {
    if (!sidebarBodyStylesApplied || typeof document === "undefined") return;
    document.body.style.cursor = previousBodyCursor;
    document.body.style.userSelect = previousBodyUserSelect;
    sidebarBodyStylesApplied = false;
  }

  function handleSidebarPointerDown(event: PointerEvent) {
    if (event.button !== 0) return;

    sidebarResizing = true;
    sidebarPointerId = event.pointerId;
    sidebarDragStartX = event.clientX;
    sidebarDragStartWidth = sidebarWidth;
    previousBodyCursor = document.body.style.cursor;
    previousBodyUserSelect = document.body.style.userSelect;
    document.body.style.cursor = "col-resize";
    document.body.style.userSelect = "none";
    sidebarBodyStylesApplied = true;
    (event.currentTarget as HTMLElement).setPointerCapture(event.pointerId);
    event.preventDefault();
  }

  function handleSidebarPointerMove(event: PointerEvent) {
    if (!sidebarResizing || event.pointerId !== sidebarPointerId) return;
    sidebarWidth = clampSidebarWidth(
      sidebarDragStartWidth + event.clientX - sidebarDragStartX,
    );
  }

  function finishSidebarResize(event: PointerEvent) {
    if (!sidebarResizing || event.pointerId !== sidebarPointerId) return;

    const handle = event.currentTarget as HTMLElement;
    if (handle.hasPointerCapture(event.pointerId)) {
      handle.releasePointerCapture(event.pointerId);
    }
    sidebarResizing = false;
    sidebarPointerId = null;
    restoreSidebarResizeStyles();
    saveSidebarWidth(sidebarWidth);
  }

  function resetSidebarWidth() {
    sidebarWidth = SIDEBAR_DEFAULT_WIDTH;
    saveSidebarWidth(sidebarWidth);
  }

  function handleSidebarResizeKeydown(event: KeyboardEvent) {
    const delta = event.key === "ArrowLeft" ? -10 : event.key === "ArrowRight" ? 10 : 0;
    if (delta === 0) return;

    event.preventDefault();
    sidebarWidth = clampSidebarWidth(sidebarWidth + delta);
    saveSidebarWidth(sidebarWidth);
  }

  function sidebarResizeHandle(node: HTMLElement) {
    node.addEventListener("pointerdown", handleSidebarPointerDown);
    node.addEventListener("pointermove", handleSidebarPointerMove);
    node.addEventListener("pointerup", finishSidebarResize);
    node.addEventListener("pointercancel", finishSidebarResize);
    node.addEventListener("dblclick", resetSidebarWidth);
    node.addEventListener("keydown", handleSidebarResizeKeydown);

    return {
      destroy() {
        node.removeEventListener("pointerdown", handleSidebarPointerDown);
        node.removeEventListener("pointermove", handleSidebarPointerMove);
        node.removeEventListener("pointerup", finishSidebarResize);
        node.removeEventListener("pointercancel", finishSidebarResize);
        node.removeEventListener("dblclick", resetSidebarWidth);
        node.removeEventListener("keydown", handleSidebarResizeKeydown);
      },
    };
  }

  onDestroy(restoreSidebarResizeStyles);

  // LIF-360: the docked sidebar can be folded away on desktop. It stays
  // mounted (display:none) rather than being torn down, so the project tree,
  // its scroll position, and the collapsed-group state all survive a toggle.
  // Below md this flag is inert: navigation there is MobileNav, which the
  // sidebar's `hidden` base class already defers to.
  let sidebarCollapsed = $state(loadSidebarCollapsed());
  function toggleSidebar() {
    sidebarCollapsed = !sidebarCollapsed;
    saveSidebarCollapsed(sidebarCollapsed);
  }

  // "?" summons the Shortcut Help overlay from anywhere in the app
  // (LIF-245) — registered as a window listener via effect because
  // <svelte:window> may only appear at the component's top level, and our
  // markup is gated behind {#if user}. (Escape for the mobile nav is
  // handled inside MobileNav, which needs to pop a level before closing.)
  //
  // The "?" guard deliberately checks typing/peek/palette directly rather
  // than calling `shortcutsSuppressed()` — that helper also folds in
  // "the shortcut help overlay itself is open", which would make a second
  // "?" press unable to close it. Esc still closes it (ShortcutHelp owns
  // that), and this toggle works both ways.
  $effect(() => {
    function onKey(e: KeyboardEvent) {
      if (mobileNavState.open || e.defaultPrevented) return;
      if (
        e.key === "?" &&
        !isTypingContext() &&
        !peekState.open &&
        !commandPaletteState.open &&
        !contextMenuState.open
      ) {
        e.preventDefault();
        toggleShortcutHelp();
        return;
      }

      // LIF-360: ⌘/Ctrl + \ folds the desktop sidebar away. It's a modifier
      // chord, so unlike the bare "?" it stays live while typing. That's the
      // contract every editor gives its sidebar toggle.
      if (e.key === "\\" && (e.metaKey || e.ctrlKey) && !e.altKey && !e.shiftKey) {
        e.preventDefault();
        toggleSidebar();
      }
    }
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  });

  let themePref = $derived($themePreference);
  let themeResolved = $derived($resolvedTheme);
  function themeMenu(e?: MouseEvent) {
    e?.stopPropagation();
    const trigger = (e?.currentTarget ?? document.activeElement) as HTMLElement;
    const rect = trigger.getBoundingClientRect();
    openContextMenu(rect.left, rect.top, [
      { label: "Light", icon: Sun, action: () => setPreference("light") },
      { label: "Dark", icon: Moon, action: () => setPreference("dark") },
      { label: "System", icon: Monitor, action: () => setPreference("system") },
    ], trigger);
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

  let user = $derived($currentUser);
  let projects = $state<Project[]>([]);
  let loading = $state(true);

  // ── Per-user sidebar project groups ────────────────────────
  let groups = $state<ProjectGroup[]>([]);
  let groupsLoaded = $state(false);
  let collapsedGroups = $state<Set<number>>(loadCollapsedGroups());

  // Membership is the server's answer, so a project missing from every group
  // is ungrouped by definition — there's no "ungrouped" row to keep in sync.
  let groupedIds = $derived(new Set(groups.flatMap((g) => g.project_ids)));
  let ungrouped = $derived(projects.filter((p) => !groupedIds.has(p.id)));

  // `ungrouped` is derived, so svelte-dnd-action can't own it during a drag.
  // The in-flight order lives here and wins while the drag is live.
  let ungroupedDuringDrag = $state<Project[] | null>(null);
  let ungroupedItems = $derived(ungroupedDuringDrag ?? ungrouped);

  // `projects` is already in sidebar order, so filtering preserves it inside
  // a group too.
  function projectsIn(group: ProjectGroup): Project[] {
    return projects.filter((p) => group.project_ids.includes(p.id));
  }

  function toggleGroup(id: number) {
    const next = new Set(collapsedGroups);
    if (!next.delete(id)) next.add(id);
    collapsedGroups = next;
    saveCollapsedGroups(next);
  }

  // ── Managing groups ────────────────────────────────────────
  // The id being renamed, or NEW_GROUP while creating one.
  let editingGroupId = $state<number | null>(null);
  let draftGroupName = $state("");
  let groupEditError = $state("");
  let groupSaving = $state(false);
  let orderError = $state("");
  let orderSaving = $state(false);
  let refreshRequest = 0;
  let groupInput = $state<HTMLInputElement | null>(null);
  let groupEditTrigger: HTMLElement | null = null;

  function menuNavigate(path: string) {
    if (navOpen) mobileNav?.navigateTo(path);
    else navigate(path);
  }

  async function moveProject(project: Project, direction: "up" | "down") {
    if (orderSaving) return;
    const group = groups.find((g) => g.project_ids.includes(project.id));
    const siblings = group ? projectsIn(group) : ungrouped;
    const index = siblings.findIndex((p) => p.id === project.id);
    const neighbor = siblings[index + (direction === "up" ? -1 : 1)];
    if (!neighbor) return;
    const next = [...projects];
    const a = next.findIndex((p) => p.id === project.id);
    const b = next.findIndex((p) => p.id === neighbor.id);
    [next[a], next[b]] = [next[b], next[a]];
    await persistProjectOrder(next);
  }

  async function persistProjectOrder(next: Project[]) {
    refreshRequest++;
    const previous = projects;
    orderSaving = true;
    orderError = "";
    projects = next;
    const res = await reorderProjects(next.map((p) => p.id));
    projects = res.ok ? res.data : previous;
    if (!res.ok) orderError = `Project order wasn't saved: ${res.error}`;
    orderSaving = false;
  }

  async function moveGroup(group: ProjectGroup, direction: "up" | "down") {
    if (orderSaving) return;
    const index = groups.findIndex((g) => g.id === group.id);
    const to = index + (direction === "up" ? -1 : 1);
    if (to < 0 || to >= groups.length) return;
    refreshRequest++;
    const previous = groups;
    const next = [...groups];
    [next[index], next[to]] = [next[to], next[index]];
    groups = next;
    orderSaving = true;
    orderError = "";
    const res = await reorderProjectGroups(next.map((g) => g.id));
    groups = res.ok ? res.data : previous;
    if (!res.ok) orderError = `Group order wasn't saved: ${res.error}`;
    orderSaving = false;
  }
  // Set when "New group…" came from a project's menu: the project to file
  // into the group as soon as it exists.
  let pendingGroupProjectId = $state<number | null>(null);

  function openProjectMenu(e: MouseEvent, project: Project) {
    e.preventDefault();
    e.stopPropagation();
    const current = groups.find((g) => g.project_ids.includes(project.id));
    const siblings = current ? projectsIn(current) : ungrouped;
    const index = siblings.findIndex((p) => p.id === project.id);
    const rect = (e.currentTarget as HTMLElement).getBoundingClientRect();
    openContextMenu(e.type === "contextmenu" ? e.clientX : rect.right, e.type === "contextmenu" ? e.clientY : rect.bottom, [
      { label: "Move up", icon: ArrowUp, disabled: orderSaving || index === 0, action: () => void moveProject(project, "up") },
      { label: "Move down", icon: ArrowDown, disabled: orderSaving || index === siblings.length - 1, action: () => void moveProject(project, "down") },
      ...groups
        .filter((g) => g.id !== current?.id)
        .map((g) => ({
          label: `Move to ${g.name}`,
          icon: Folder,
          action: () => void assignProject(project.id, g.id),
        })),
      ...(current
        ? [
            {
              label: "Remove from group",
              icon: FolderMinus,
              action: () => void assignProject(project.id, null),
            },
          ]
        : []),
      {
        label: "New group…",
        icon: FolderPlus,
        action: () => startCreatingGroup(project.id),
      },
    ], e.currentTarget as HTMLElement);
  }

  function openGroupMenu(e: MouseEvent, group: ProjectGroup) {
    e.preventDefault();
    e.stopPropagation();
    const index = groups.findIndex((g) => g.id === group.id);
    const rect = (e.currentTarget as HTMLElement).getBoundingClientRect();
    openContextMenu(e.type === "contextmenu" ? e.clientX : rect.right, e.type === "contextmenu" ? e.clientY : rect.bottom, [
      { label: "Move up", icon: ArrowUp, disabled: orderSaving || index === 0, action: () => void moveGroup(group, "up") },
      { label: "Move down", icon: ArrowDown, disabled: orderSaving || index === groups.length - 1, action: () => void moveGroup(group, "down") },
      { label: "Rename", icon: Pencil, action: () => startRenamingGroup(group) },
      { label: "Delete group", icon: Trash2, action: () => void removeGroup(group) },
    ], e.currentTarget as HTMLElement);
  }

  function openCreateMenu(e: MouseEvent) {
    // Without this the click keeps bubbling to ContextMenu's window listener,
    // which closes the menu this very call just opened.
    e.stopPropagation();
    // Anchored to the button's own box, not the cursor, so the menu lines up
    // under the + rather than wherever the pointer happened to be.
    const rect = (e.currentTarget as HTMLElement).getBoundingClientRect();
    openContextMenu(rect.left, rect.bottom, [
      { label: "New project", icon: Plus, href: "#/projects/new", action: () => menuNavigate("/projects/new") },
      { label: "New group", icon: FolderPlus, action: () => startCreatingGroup() },
    ], e.currentTarget as HTMLElement);
  }

  function startRenamingGroup(group: ProjectGroup) {
    groupEditTrigger = document.activeElement as HTMLElement | null;
    groupEditError = "";
    editingGroupId = group.id;
    draftGroupName = group.name;
  }

  function startCreatingGroup(projectId: number | null = null) {
    groupEditTrigger = document.activeElement as HTMLElement | null;
    groupEditError = "";
    editingGroupId = NEW_GROUP;
    draftGroupName = "";
    pendingGroupProjectId = projectId;
  }

  function cancelGroupEdit() {
    if (groupSaving) return;
    const id = editingGroupId;
    groupEditError = "";
    editingGroupId = null;
    pendingGroupProjectId = null;
    if (!navOpen) void tick().then(() => {
      const target = groupEditTrigger?.isConnected ? groupEditTrigger
        : document.querySelector<HTMLElement>(`[data-sidebar-group-actions="${id}"]`);
      target?.focus({ preventScroll: true });
    });
  }

  async function assignProject(projectId: number, groupId: number | null) {
    const res = await assignProjectGroup(projectId, groupId);
    if (res.ok) {
      await refreshProjects();
    } else {
      toast(res.error, { kind: "error" });
    }
  }

  async function commitGroupName() {
    if (groupSaving) return false;
    const name = draftGroupName.trim();
    const editing = editingGroupId;
    const pending = pendingGroupProjectId;
    if (!name || editing === null) {
      groupEditError = "Enter a group name.";
      return false;
    }
    groupSaving = true;
    groupEditError = "";

    const res =
      editing === NEW_GROUP
        ? await createProjectGroup(name)
        : await renameProjectGroup(editing, name);
    if (!res.ok) {
      groupSaving = false;
      groupEditError = res.error;
      await tick();
      groupInput?.focus();
      return false;
    }
    groupSaving = false;
    cancelGroupEdit();
    // The group exists now even if the follow-up assignment fails, so report
    // that separately: the user's next move is to file the project by hand,
    // not to create the group again.
    if (editing === NEW_GROUP && pending !== null) {
      const assigned = await assignProjectGroup(pending, res.data.id);
      if (!assigned.ok) {
        toast(`Group created, but the project wasn't moved into it: ${assigned.error}`, {
          kind: "error",
        });
      }
    }
    await refreshProjects();
    return true;
  }

  // Deleting a group never touches the projects inside it — they reappear in
  // the ungrouped list below, so there is nothing to confirm.
  async function removeGroup(group: ProjectGroup) {
    const res = await deleteProjectGroup(group.id);
    if (res.ok) {
      await refreshProjects();
    } else {
      toast(res.error, { kind: "error" });
    }
  }

  // Load user once on mount
  $effect(() => {
    loadUser();
  });

  // Re-fetch projects whenever route changes (catches new/deleted projects).
  // Also dismiss the mobile nav on navigation so it never lingers over the
  // newly-loaded route (LIF-223).
  $effect(() => {
    route; // track route changes
    refreshProjects();
  });

  $effect(() =>
    startAutoRefresh({
      refresh: refreshProjects,
      isBusy: () => dragActive || orderSaving,
      // `project_groups.changed` uses an underscore, so the `project.` prefix
      // test below does not cover it — it needs its own clause.
      shouldRefresh: (event) =>
        event.type === "resync.required" ||
        event.type === "projects.reordered" ||
        event.type === "project_groups.changed" ||
        event.type.startsWith("project."),
    }),
  );

  async function loadUser() {
    const revision = getUserRevision();
    const session = localStorage.getItem("lific_token");
    const res = await me();
    if (session !== localStorage.getItem("lific_token")) return;
    if (res.ok) {
      publishUser(res.data, revision);
    } else {
      clearSession();
      currentUser.set(null);
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
    if (dragActive || orderSaving) return;
    const request = ++refreshRequest;
    groupsLoaded = false;
    const [projectsRes, groupsRes] = await Promise.all([
      listProjects(),
      listProjectGroups(),
    ]);
    if (request !== refreshRequest || dragActive || orderSaving) return;
    if (projectsRes.ok) {
      projects = projectsRes.data;
    }
    if (groupsRes.ok) {
      groups = groupsRes.data;
      groupsLoaded = true;
    }
  }

  // ── LIF-233: drag-to-reorder projects in the sidebar ────────
  // The dndzone owns `projects` during a drag. We veto auto-refresh while
  // dragActive, then persist the new order on finalize (server reindexes
  // sort_order and returns the canonical list).
  let dragActive = $state(false);
  // LIF-246: checked fresh at each drag/flip (not memoized) so a live
  // toggle of the motion preference takes effect on the next reorder —
  // same pattern as IssueList's flipMs().
  function flipMs(): number {
    return motionReduced() ? 0 : 150;
  }

  function handleProjectConsider(e: CustomEvent<DndEvent<Project>>) {
    dragActive = true;
    ungroupedDuringDrag = e.detail.items;
  }

  async function handleProjectFinalize(e: CustomEvent<DndEvent<Project>>) {
    ungroupedDuringDrag = e.detail.items;
    const ids = reorderPayload(e.detail);
    await persistProjectOrder(ids.map((id) => projects.find((p) => p.id === id)!));
    ungroupedDuringDrag = null;
    dragActive = false;
  }

  // Relocate the dragged project against its siblings while preserving the
  // personal order of projects outside this zone.
  function reorderPayload(detail: DndEvent<Project>): number[] {
    const movedId = Number(detail.info.id);
    const moved = projects.find((p) => p.id === movedId);
    if (!moved) return projects.map((p) => p.id);

    const items = detail.items;
    const pos = items.findIndex((p) => p.id === movedId);
    const after = items[pos + 1];
    const before = items[pos - 1];

    const rest = projects.filter((p) => p.id !== movedId);
    // Anchor to whichever ungrouped neighbour the drop landed against; with
    // no neighbour on either side the zone held only this project, so its
    // position relative to everything else is unchanged.
    let at: number;
    if (after) {
      at = rest.findIndex((p) => p.id === after.id);
    } else if (before) {
      at = rest.findIndex((p) => p.id === before.id) + 1;
    } else {
      at = rest.length;
    }
    rest.splice(at, 0, moved);
    return rest.map((p) => p.id);
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
    const match = route.match(/^\/([A-Z][A-Z0-9_-]*)\//i);
    return match ? match[1].toUpperCase() : null;
  }

  let activeProject = $derived(projectFromRoute());

  type RecentSection = "issues" | "modules" | "pages" | "plans";
  let recentIssues = $state<Issue[]>([]);
  let recentModules = $state<Module[]>([]);
  let recentPages = $state<Page[]>([]);
  let recentPlans = $state<Plan[]>([]);
  let recentLoading = $state<RecentSection | null>(null);
  let recentRequest = 0;
  let recentProjectId: number | null = null;
  let recentOpen = $state(false);

  let activeRecentProjectId = $derived(
    projects.find((project) => project.identifier === activeProject)?.id ?? null,
  );
  let activeRecentSection = $derived.by<RecentSection | null>(() => {
    if (!activeProject) return null;
    const prefix = `/${activeProject}`;
    if (isActive(`${prefix}/issues`)) return "issues";
    if (isActive(`${prefix}/modules`)) return "modules";
    if (isActive(`${prefix}/pages`)) return "pages";
    if (isActive(`${prefix}/plans`)) return "plans";
    return null;
  });

  // LIF-307: refresh the active resource's five most-recent items on each
  // route entry. This deliberately has no auto-refresh loop.
  $effect(() => {
    route; // track re-entry to the same section, including detail routes
    const projectId = activeRecentProjectId;
    const section = activeRecentSection;
    if (projectId === null || section === null) {
      recentRequest++;
      recentLoading = null;
      return;
    }
    void loadRecents(projectId, section);
  });

  async function loadRecents(projectId: number, section: RecentSection) {
    const requestId = ++recentRequest;
    recentLoading = section;
    // Preserve the last successful rows on refresh, but never display another
    // project's cached data while the new project is loading.
    if (recentProjectId !== projectId) {
      recentProjectId = projectId;
      recentIssues = [];
      recentModules = [];
      recentPages = [];
      recentPlans = [];
    }

    if (section === "issues") {
      const res = await listIssues({
        project_id: projectId,
        order_by: "updated",
        order: "desc",
        limit: 5,
      });
      if (requestId !== recentRequest) return;
      if (res.ok) recentIssues = res.data;
    } else if (section === "modules") {
      const res = await listModules(projectId);
      if (requestId !== recentRequest) return;
      recentModules = res.ok
        ? res.data.sort((a, b) => b.updated_at.localeCompare(a.updated_at)).slice(0, 5)
        : recentModules;
    } else if (section === "pages") {
      // Page statuses cannot be negated server-side, so fetch a bounded recent
      // slice for each visible lifecycle state, then combine the candidates.
      // This avoids loading every page (and its content) just to omit archived
      // pages from the five-item sidebar list.
      const results = await Promise.all(
        ["draft", "active", "complete"].map((status) =>
          listPages(projectId, undefined, undefined, status, {
            order_by: "updated",
            order: "desc",
            limit: 5,
          }),
        ),
      );
      if (requestId !== recentRequest) return;
      recentPages = results.every((res) => res.ok)
        ? results
            .flatMap((res) => (res.ok ? res.data : []))
            .sort((a, b) => b.updated_at.localeCompare(a.updated_at) || b.id - a.id)
            .slice(0, 5)
        : recentPages;
    } else {
      // Over-fetch so filtering archived plans out can still yield 5 rows.
      const res = await listPlans(projectId, undefined, 10);
      if (requestId !== recentRequest) return;
      recentPlans = res.ok
        ? res.data.filter((p) => p.status !== "archived").slice(0, 5)
        : recentPlans;
    }

    if (requestId === recentRequest) recentLoading = null;
  }

  // LIF-234: the single point that primes the shared project-role store on
  // each project switch. Resolves the route identifier to a numeric id from
  // the already-loaded projects list, then loads (once, cached) the caller's
  // effective role so every route/component can gate mutate affordances
  // without its own fetch. Runs off `activeProject` + `projects` so it fires
  // as soon as both are known (projects arrive async after the first route
  // render). Case-insensitive match mirrors the route matcher.
  $effect(() => {
    const ident = activeProject;
    if (!ident) return;
    const proj = projects.find(
      (p) => p.identifier.toLowerCase() === ident.toLowerCase(),
    );
    if (proj) loadProjectRole(proj.id);
  });

  // Expansion is independent of navigation. Reveal only on project entry,
  // never on refresh or a route change within the same project.
  let expandedProjects = $state(new Set<number>());
  let revealedProjectId: number | null = null;
  $effect(() => {
    const project = projects.find((p) => p.identifier === activeProject);
    if (!project) { revealedProjectId = null; return; }
    if (!groupsLoaded) return;
    // Track asynchronous group arrival so a direct link is revealed once
    // both lists have loaded, but do not track deliberate disclosure edits.
    const containing = groups.find((g) => g.project_ids.includes(project.id));
    if (revealedProjectId === project.id) return;
    revealedProjectId = project.id;
    untrack(() => {
      expandedProjects = new Set([...expandedProjects, project.id]);
      if (containing && collapsedGroups.has(containing.id)) toggleGroup(containing.id);
    });
    void tick().then(() => {
      if (revealedProjectId !== project.id || !matchMedia("(min-width: 768px)").matches) return;
      document.querySelector<HTMLElement>(`[data-sidebar-project="${project.id}"]`)?.scrollIntoView({ block: "nearest" });
    });
  });
  function subnavOpen(project: Project): boolean {
    return !dragActive && expandedProjects.has(project.id);
  }
  function toggleProject(project: Project) {
    const next = new Set(expandedProjects);
    if (!next.delete(project.id)) next.add(project.id);
    expandedProjects = next;
  }

  // ── Mobile header context (LIF-349) ─────────────────────────
  // The phone header states where you are rather than repeating the
  // wordmark, and doubles as a second way into the nav — tapping it opens
  // MobileNav already showing this project's destinations.
  let activeProjectRecord = $derived(
    projects.find(
      (p) => p.identifier.toLowerCase() === (activeProject ?? "").toLowerCase(),
    ) ?? null,
  );
  let mobileSection = $derived.by(() => {
    if (route === "/") return "Home";
    if (isActive("/settings")) return "Settings";
    if (route === "/projects/new") return "New project";
    if (route === "/projects/import") return "Import archive";
    if (!activeProject) return null;
    const rest = route.slice(activeProject.length + 1);
    const slug = rest.split("/")[1] ?? "";
    const labels: Record<string, string> = {
      overview: "Overview",
      settings: "Overview",
      issues: "Issues",
      board: "Board",
      modules: "Modules",
      pages: "Pages",
      files: "Files",
      plans: "Plans",
      activity: "Activity",
      insights: "Insights",
    };
    return labels[slug] ?? null;
  });
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
    <!-- ── SIDEBAR (LIF-192 redesign) ──────────────────────────
         Desktop only. Below md, navigation is MobileNav — a full-screen
         drilldown surface with its own structure, not this tree at a
         narrower width (LIF-349). -->
    <aside
      class="{sidebarCollapsed ? 'hidden' : 'hidden md:flex'} w-[var(--sidebar-w)]
             shrink-0 relative flex-col bg-[var(--chrome)] select-none"
      style={`--sidebar-w: ${sidebarWidth}px`}
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
        <!-- LIF-360: collapse control. Sits beside the brand rather than in
             the footer so its position mirrors the expand control that takes
             its place in the topbar. -->
        <button
          class="size-7 shrink-0 grid place-items-center rounded-md
                 text-[var(--text-faint)] hover:text-[var(--text)]
                 hover:bg-[var(--bg-subtle)] transition-colors"
          onclick={toggleSidebar}
          title="Collapse sidebar  ·  ⌘\\"
          aria-label="Collapse sidebar"
        >
          <PanelLeftClose size={15} />
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
        <!-- LIF-237: Home — "My Work" landing dashboard. Sits above the
             project list as its own top-level entry, mirroring the sub-nav
             pill's shape (icon + label) but unindented and un-chevroned
             since it isn't a disclosure. -->
        <a href="#/" use:navLink={navigate} aria-current={route === "/" ? "page" : undefined}
          class="w-full flex items-center gap-2 px-2.5 py-1.5 mb-1 rounded-md
                 text-left text-body-sm transition-colors
                 {isActive('/')
            ? 'text-[var(--text)] bg-[var(--bg-subtle)] font-medium'
            : 'text-[var(--text-muted)] hover:text-[var(--text)] hover:bg-[var(--bg-subtle)]'}"
        >
          <Home size={14} class="shrink-0 {isActive('/') ? 'text-[var(--accent)]' : ''}" />
          Home
        </a>

        <!-- One project entry: the pill plus its sub-nav. Shared verbatim by
             the grouped lists and the ungrouped drag zone below, so a project
             looks and behaves identically wherever it is filed. -->
        {#snippet projectEntry(project: Project)}
            {@const isProjectActive = activeProject === project.identifier}
            {@const open = subnavOpen(project)}
            <div
              class="sidebar-row group w-full flex items-center rounded-md
                     text-left text-body-sm transition
                     {isProjectActive
                ? 'text-[var(--text)] bg-[var(--bg-subtle)] font-medium'
                : 'text-[var(--text-muted)] hover:text-[var(--text)] hover:bg-[var(--bg-subtle)]'}"
            >
              <button class="size-7 shrink-0 grid place-items-center rounded-md hover:bg-[var(--bg-subtle)]"
                aria-label={`${open ? 'Collapse' : 'Expand'} ${project.name}`}
                aria-expanded={open} aria-controls={`project-nav-${project.id}`}
                onclick={() => toggleProject(project)}>
              <ChevronRight
                size={13}
                class="shrink-0 transition-transform
                       {open ? 'rotate-90' : ''}
                       {isProjectActive ? 'text-[var(--text-muted)]' : 'text-[var(--text-faint)] group-hover:text-[var(--text-muted)]'}"
              />
              </button>
              <a href={`#/${project.identifier}/overview`} use:navLink={navigate}
                data-sidebar-project={project.id}
                aria-current={route === `/${project.identifier}/overview` ? "page" : undefined}
                title={project.name} class="min-w-0 flex-1 flex items-center gap-1.5 py-1.5"
                oncontextmenu={(e) => openProjectMenu(e, project)}>
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
              </a>
              <button class="sidebar-overflow size-7 shrink-0 grid place-items-center rounded-md hover:bg-[var(--bg-subtle)]"
                aria-label={`Actions for ${project.name}`} aria-haspopup="menu"
                onclick={(e) => openProjectMenu(e, project)}><Ellipsis size={15} /></button>
            </div>

              <!-- Sub-nav: indented under the project with a vertical guide
                   line, matching the tree language used in Pages. -->
              <div id={`project-nav-${project.id}`} hidden={!open} class="project-subnav ml-3 pl-1 mt-0.5 mb-1.5 border-l border-[var(--border)] flex flex-col gap-px">
                {#snippet subItem(href: string, label: string, Icon: typeof List)}
                  {@const active = isActive(href)}
                  <a href={`#${href}`} use:navLink={navigate} aria-current={active ? "page" : undefined}
                    class="w-full flex items-center gap-2 px-2 py-1 rounded-md
                           text-left text-body-sm transition-colors
                           {active
                      ? 'text-[var(--text)] bg-[var(--bg-subtle)] font-medium'
                      : 'text-[var(--text-muted)] hover:text-[var(--text)] hover:bg-[var(--bg-subtle)]'}"
                  >
                    <Icon size={14} class="shrink-0 {active ? 'text-[var(--accent)]' : ''}" />
                    {label}
                  </a>
                {/snippet}
                {#snippet recentItem(href: string, label: string, identifier: string | null)}
                  <a href={`#${href}`} use:navLink={navigate} aria-current={isActive(href) ? "page" : undefined}
                    title={identifier ? `${identifier}: ${label}` : label}
                    class="recent-link relative w-full flex items-center gap-1 px-2 py-1 rounded-md
                           text-left text-caption transition-colors
                           {isActive(href)
                      ? 'text-[var(--text)] bg-[var(--bg-subtle)] font-medium'
                      : 'text-[var(--text-muted)] hover:text-[var(--text)] hover:bg-[var(--bg-subtle)]'}"
                  >
                    {#if identifier}
                      <span class="font-mono text-[var(--text-faint)] shrink-0">#{identifier.split('-').at(-1)}</span>
                    {/if}
                    <span class="flex-1 min-w-0 truncate">{label}</span>
                    <span class="focus-title">{label}</span>
                  </a>
                {/snippet}
                {#snippet recentItems(section: RecentSection, project: Project)}
                    {#if section === "issues"}
                      {#each recentIssues as issue (issue.id)}
                        {@render recentItem(`/${project.identifier}/issues/${issue.identifier}`, issue.title, issue.identifier)}
                      {/each}
                    {:else if section === "modules"}
                      {#each recentModules as module (module.id)}
                        {@render recentItem(`/${project.identifier}/modules/${module.id}`, module.name, null)}
                      {/each}
                    {:else if section === "pages"}
                      {#each recentPages as page (page.id)}
                        {@render recentItem(`/${project.identifier}/pages/${page.id}`, page.title, null)}
                      {/each}
                    {:else}
                      {#each recentPlans as plan (plan.id)}
                        {@render recentItem(`/${project.identifier}/plans/${plan.id}`, plan.title, null)}
                      {/each}
                    {/if}
                {/snippet}
                {@render subItem(`/${project.identifier}/overview`, "Overview", LayoutDashboard)}
                {@render subItem(`/${project.identifier}/issues`, "Issues", List)}
                {@render subItem(`/${project.identifier}/board`, "Board", LayoutGrid)}
                {@render subItem(`/${project.identifier}/graph`, "Graph", Waypoints)}
                {@render subItem(`/${project.identifier}/modules`, "Modules", Layers)}
                {@render subItem(`/${project.identifier}/pages`, "Pages", FileText)}
                {@render subItem(`/${project.identifier}/files`, "Files", Paperclip)}
                {@render subItem(`/${project.identifier}/plans`, "Plans", ListChecks)}
                {@render subItem(`/${project.identifier}/activity`, "Activity", History)}
                {@render subItem(`/${project.identifier}/insights`, "Insights", TrendingUp)}
                {#if isProjectActive && activeRecentSection}
                  <button class="flex items-center gap-1 px-2 py-1 text-caption text-[var(--text-faint)] rounded-md hover:bg-[var(--bg-subtle)]"
                    aria-expanded={recentOpen} aria-controls={`recent-${project.id}`} onclick={() => recentOpen = !recentOpen}>
                    <ChevronRight size={12} class={recentOpen ? "rotate-90" : ""} /> Recent {activeRecentSection}
                  </button>
                  <div id={`recent-${project.id}`} hidden={!recentOpen} aria-busy={recentLoading !== null}>
                    {@render recentItems(activeRecentSection, project)}
                  </div>
                {/if}
              </div>
        {/snippet}

        <!-- The header renders unconditionally: it carries the only affordance
             for creating a group, so gating it on having projects would make
             the first group unreachable on a brand-new instance. -->
        <div class="flex items-center justify-between px-2 pt-1.5 pb-1">
          <span class="text-micro font-semibold uppercase tracking-widest text-[var(--text-faint)]">
            Projects
          </span>
          <button
            class="size-8 flex items-center justify-center rounded
                   text-[var(--text-faint)] hover:text-[var(--accent)]
                   hover:bg-[var(--bg-subtle)] transition-colors"
            title="New project or group"
            aria-label="New project or group" aria-haspopup="menu"
            onclick={openCreateMenu}
          >
            <Plus size={13} />
          </button>
        </div>

        <!-- Outside the guard below for the same reason as the header: on an
             empty instance this input is the whole first-group flow. -->
        {#snippet groupNameInput()}
          <!-- Escape cancels from the input or either form button. -->
          <!-- svelte-ignore a11y_no_noninteractive_element_interactions -->
          <form class="px-1 py-1" onsubmit={(e) => { e.preventDefault(); void commitGroupName(); }}
            onkeydown={(e) => { if (e.key === "Escape") { e.stopPropagation(); e.preventDefault(); cancelGroupEdit(); } }}>
          <input
            class="w-full h-7 px-2 mb-0.5 rounded-md text-body-sm bg-[var(--bg)]
                   border border-[var(--border)] text-[var(--text)]"
            placeholder="Group name"
            aria-label="Group name" aria-invalid={!!groupEditError} aria-describedby={groupEditError ? "group-edit-error" : undefined}
            bind:this={groupInput}
            bind:value={draftGroupName}
            disabled={groupSaving}
            autofocus
          />
          {#if groupEditError}<p id="group-edit-error" role="alert" class="text-caption text-[var(--error)] break-words">{groupEditError}</p>{/if}
          <div class="flex gap-2 text-caption">
            <button type="submit" disabled={groupSaving} class="px-2 py-1 rounded hover:bg-[var(--bg-subtle)]">{groupSaving ? "Saving…" : "Save"}</button>
            <button type="button" disabled={groupSaving} class="px-2 py-1 rounded hover:bg-[var(--bg-subtle)]" onclick={cancelGroupEdit}>Cancel</button>
          </div>
          </form>
        {/snippet}

        {#if editingGroupId === NEW_GROUP}
          {@render groupNameInput()}
        {/if}
        {#if orderError}<p role="alert" class="px-2 py-1 text-caption text-[var(--error)] break-words">{orderError}</p>{/if}

        <!-- Groups render above the ungrouped list. The guard covers groups as
             well as projects so a group whose last project left stays around
             to be renamed or deleted instead of vanishing. -->
        {#if projects.length > 0 || groups.length > 0}
          {#each groups as group (group.id)}
            {@const collapsed = collapsedGroups.has(group.id)}
            {#if editingGroupId === group.id}
              {@render groupNameInput()}
            {:else}
            <div class="sidebar-row group flex items-center">
            <button
              class="min-w-0 flex-1 flex items-center gap-1.5 pl-1.5 pr-1 py-1.5 rounded-md
                     text-left text-body-sm transition text-[var(--text-muted)]
                     hover:text-[var(--text)] hover:bg-[var(--bg-subtle)]"
              aria-expanded={!collapsed}
              aria-controls={`group-${group.id}`} title={group.name}
              onclick={() => toggleGroup(group.id)}
              oncontextmenu={(e) => openGroupMenu(e, group)}
            >
              <ChevronRight
                size={13}
                class="shrink-0 transition-transform {collapsed ? '' : 'rotate-90'}
                       text-[var(--text-faint)] group-hover:text-[var(--text-muted)]"
              />
              <Folder size={14} class="shrink-0 text-[var(--text-faint)]" />
              <span class="truncate flex-1">{group.name}</span>
            </button>
            <button class="sidebar-overflow size-7 shrink-0 grid place-items-center rounded-md hover:bg-[var(--bg-subtle)]"
              data-sidebar-group-actions={group.id}
              aria-label={`Actions for group ${group.name}`} aria-haspopup="menu" onclick={(e) => openGroupMenu(e, group)}><Ellipsis size={15} /></button>
            </div>
            {/if}
            {#if collapsed && projectsIn(group).some((p) => p.identifier === activeProject)}
              <button class="ml-3 max-w-[calc(100%-0.75rem)] truncate text-caption text-[var(--accent)] px-2 py-1"
                title={`Show ${activeProjectRecord?.name}`} onclick={() => toggleGroup(group.id)}>Current: {activeProjectRecord?.name}</button>
            {/if}
              <!-- Same indent and guide line as a project's sub-nav, so the
                   sidebar reads as one tree rather than two conventions. -->
              <div id={`group-${group.id}`} hidden={collapsed} class="ml-2 pl-1 border-l border-[var(--border)]">
                {#each projectsIn(group) as project (project.id)}
                  {@render projectEntry(project)}
                {/each}
              </div>
          {/each}

          <!-- LIF-233: drag-to-reorder zone, now holding only the ungrouped
               projects. Each is a SINGLE direct child of the zone (pill + its
               sub-nav wrapped together), so svelte-dnd-action's
               one-item-per-child model stays 1:1 — the active project's
               expanded sub-nav must NOT become its own draggable item. The
               header/+button and the groups above sit OUTSIDE the zone. -->
          <div
            use:dndzone={{
              items: ungroupedItems,
              flipDurationMs: flipMs(),
              type: "lific-projects",
              dropTargetStyle: {},
              dragDisabled: orderSaving || ungroupedItems.length < 2,
            }}
            onconsider={handleProjectConsider}
            onfinalize={handleProjectFinalize}
          >
          {#each ungroupedItems as project (project.id)}
            <!-- animate:flip gives the reorder its slide; the wrapper holds
                 both the pill and (when open) the sub-nav so they move as a
                 unit. -->
            <div animate:flip={{ duration: flipMs() }}>
              {@render projectEntry(project)}
            </div>
          {/each}
          </div>
        {:else if editingGroupId !== NEW_GROUP}
          <div class="px-3 py-6">
            <p class="text-body-sm text-[var(--text-faint)] mb-2">No projects yet.</p>
            <div class="flex flex-col items-start gap-1">
              <a href="#/projects/new" use:navLink={navigate}
                class="text-body-sm text-[var(--accent)] hover:underline"
              >
                Create a project
              </a>
              <button
                class="text-body-sm text-[var(--accent)] hover:underline"
                onclick={() => startCreatingGroup()}
              >
                Create a group
              </button>
            </div>
          </div>
        {/if}
      </nav>

      <!-- Footer: the user identity IS the Settings entry (logout now lives
           inside Settings → Security). A compact theme toggle sits beside it. -->
      <div class="p-2 flex items-center gap-1">
        <a href="#/settings" use:navLink={navigate} aria-current={isActive('/settings') ? 'page' : undefined}
          class="flex-1 min-w-0 flex items-center gap-2.5 px-2 py-1.5 rounded-md text-left transition-colors
                 {isActive('/settings')
            ? 'bg-[var(--bg-subtle)]'
            : 'hover:bg-[var(--bg-subtle)]'}"
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
        </a>
        <button
          class="size-8 shrink-0 grid place-items-center rounded-md
                 text-[var(--text-muted)] hover:text-[var(--text)] hover:bg-[var(--bg-subtle)] transition-colors"
          onclick={themeMenu}
          title="Theme: {themePref}"
          aria-label="Choose theme, current: {themePref}" aria-haspopup="menu"
        >
          {#if themePref === "system"}
            <Monitor size={15} />
          {:else if themeResolved === "dark"}
            <Moon size={15} />
          {:else}
            <Sun size={15} />
          {/if}
        </button>
        <!-- LIF-245: small, unobtrusive entry point to the Shortcut Help
             overlay — mirrors the theme toggle beside it. The "?" key does
             the same thing from anywhere; this is for anyone who doesn't
             know the key exists yet. -->
        <button
          class="size-8 shrink-0 grid place-items-center rounded-md
                 text-[var(--text-muted)] hover:text-[var(--text)] hover:bg-[var(--bg-subtle)] transition-colors"
          onclick={() => toggleShortcutHelp()}
          title="Keyboard shortcuts  ·  ?"
          aria-label="Keyboard shortcuts"
        >
          <HelpCircle size={15} />
        </button>
      </div>
      <!-- LIF-309: an 8px hit area keeps the 3px resize indicator easy to grab. -->
      <!-- svelte-ignore a11y_no_noninteractive_tabindex -->
      <div
        class="group absolute inset-y-0 -right-1 z-20 hidden w-2 cursor-col-resize touch-none md:block focus:outline-none"
        role="separator"
        aria-orientation="vertical"
        aria-label="Resize sidebar"
        aria-valuemin={SIDEBAR_MIN_WIDTH}
        aria-valuemax={SIDEBAR_MAX_WIDTH}
        aria-valuenow={sidebarWidth}
        tabindex="0"
        use:sidebarResizeHandle
      >
        <span
          class="pointer-events-none absolute inset-y-0 left-1/2 w-[3px] -translate-x-1/2
                 opacity-40 transition-[background-color,opacity]
                 {sidebarResizing
            ? 'bg-[var(--accent)] opacity-60'
            : 'bg-transparent group-hover:bg-[var(--border)] group-focus-visible:bg-[var(--accent)]'}"
        ></span>
      </div>
    </aside>

    <!-- Right column: chrome topbar (continuous with sidebar) + inset panel -->
    <div class="flex-1 min-w-0 flex flex-col">
      <!-- Mobile header (below md only). The hamburger opens the nav at the
           project list; the title beside it states where you currently are
           and opens the nav already showing THAT project's destinations, so
           moving between a project's sections is one tap plus one, not a
           trip back through the root (LIF-349). -->
      <header
        class="md:hidden shrink-0 flex items-center gap-1 h-12 px-1
               pt-[env(safe-area-inset-top)] box-content bg-[var(--chrome)]"
      >
        <button
          class="size-11 shrink-0 grid place-items-center rounded-lg
                 text-[var(--text-muted)] active:bg-[var(--bg-subtle)] transition-colors"
          aria-label="Open navigation"
          aria-expanded={navOpen}
          onclick={() => mobileNav?.openAt(null)}
        >
          <Menu size={20} />
        </button>
        {#if activeProjectRecord}
          <button
            class="min-w-0 flex-1 h-11 flex items-center gap-2 px-1.5 rounded-lg text-left
                   active:bg-[var(--bg-subtle)] transition-colors"
            onclick={() => mobileNav?.openAt(activeProjectRecord)}
          >
            {#if activeProjectRecord.emoji}
              <span class="size-6 grid place-items-center shrink-0">
                <ProjectIcon value={activeProjectRecord.emoji} size={18} />
              </span>
            {:else}
              <span
                class="size-6 rounded-md border border-[var(--border)] bg-[var(--bg-subtle)]
                       grid place-items-center text-micro font-semibold tracking-tight
                       shrink-0 text-[var(--text-muted)]"
              >
                {activeProjectRecord.identifier.slice(0, 2)}
              </span>
            {/if}
            <span class="min-w-0 flex items-baseline gap-1.5">
              <span class="truncate font-display text-body-lg tracking-tight text-[var(--text)]">
                {activeProjectRecord.name}
              </span>
              {#if mobileSection}
                <span class="shrink-0 text-body-sm text-[var(--text-faint)]">
                  {mobileSection}
                </span>
              {/if}
            </span>
            <ChevronRight size={14} class="shrink-0 text-[var(--text-faint)]" />
          </button>
        {:else}
          <span class="flex-1 min-w-0 px-1.5 flex items-center gap-2">
            <img src="/logo.webp" alt="" width="22" height="22" class="rounded-md shrink-0" />
            <span
              class="truncate font-display text-heading tracking-tight text-[var(--text)] leading-none"
            >
              {mobileSection ?? "Lific"}
            </span>
          </span>
        {/if}
      </header>

      <!-- Chrome topbar slot. Routes pass a `topbar` snippet for breadcrumb,
           filters, search, etc. Background matches the sidebar so the L is
           visually seamless. -->
      {#if topbarSnippet || sidebarCollapsed}
        <!-- The topbar deliberately uses muted text/icon colors so it
             reads as quieter than the content panel below. We avoid
             `opacity` for the dimming effect because it creates a CSS
             stacking context that traps absolutely-positioned dropdowns
             (filters, display, help popovers) BEHIND the content panel. -->
        <div class="shrink-0 flex items-stretch min-h-0 bg-[var(--chrome)]">
          {#if sidebarCollapsed}
            <!-- LIF-360: with the sidebar folded away this is the only way
                 back to it besides the shortcut, so it leads the topbar. It
                 renders even on routes that supply no topbar snippet, which
                 is why the row above is no longer gated on the snippet
                 alone. -->
            <div class="hidden md:flex items-center shrink-0 pl-2 pr-0.5 py-2">
              <button
                class="size-7 grid place-items-center rounded-md
                       text-[var(--text-faint)] hover:text-[var(--text)]
                       hover:bg-[var(--bg-subtle)] transition-colors"
                onclick={toggleSidebar}
                title="Expand sidebar  ·  ⌘\\"
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
             docked to cast the shadow; on mobile there's nothing to its left,
             and neither is there once the sidebar is collapsed (LIF-360), so
             the gradient would read as a vignette against the viewport edge. -->
        {#if !sidebarCollapsed}
          <div
            class="hidden md:block pointer-events-none absolute top-0 left-0 bottom-0 w-6 z-10
                   bg-gradient-to-r from-[var(--shadow-recess)] to-transparent"
          ></div>
        {/if}
      </div>
    </div>
  </div>

  <!-- LIF-349: the phone's navigation surface. Mounted as a sibling of the
       app shell (not inside it) because it covers the whole viewport rather
       than occupying a column, and it lazily mounts itself on first open so
       desktop sessions never build a second project tree. -->
  <MobileNav
    bind:this={mobileNav}
    bind:open={navOpen}
    bind:editingGroupId
    bind:draftGroupName
    {route}
    {navigate}
    {user}
    {projects}
    {groups}
    {projectsIn}
    ungrouped={ungroupedItems}
    {collapsedGroups}
    onToggleGroup={toggleGroup}
    onOpenPalette={() => palette?.openPalette()}
    onOpenCreateMenu={openCreateMenu}
    onProjectMenu={openProjectMenu}
    onGroupMenu={openGroupMenu}
    onCommitGroupName={commitGroupName}
    onCancelGroupEdit={cancelGroupEdit}
    {groupEditError}
    {orderError}
    {themePref}
    {themeResolved}
    onCycleTheme={themeMenu}
  />

  <!-- LIF-159: cmd+k / ctrl+p jump-anywhere. Mounted here (once, above
       routes) so its session catalog cache survives navigation. -->
  <!-- `route` is passed because the palette outlives every route: it needs
       the authoritative current path to know which project's read model to
       search, and to invalidate in-flight results when that project
       changes underneath it (LIF-445). -->
  <CommandPalette bind:this={palette} {navigate} {route} actions={paletteActions} />
  <!-- LIF-245: shortcut help overlay, mounted once so "?" works from any
       route. -->
  <ShortcutHelp />
  <!-- LIF-248: issue peek panel + right-click context menu, mounted once
       here (not per-route) so shift-click-to-peek and right-click work on
       every authenticated route — issue detail, plans, pages, activity,
       home — not just the issue list/board. Both are `fixed`-positioned
       singletons driven by module stores, so mounting them here vs. deep
       inside a route makes no visual difference; it just makes them
       reachable from everywhere. -->
  <PeekPanel {navigate} />
  <PagePeekPanel {navigate} />
  <ContextMenu />
{/if}

<style>
  aside a:hover { text-decoration: none; }
  [data-sidebar-project] { color: inherit; }
  .project-subnav[hidden] { display: none; }
  .focus-title { display: none; }
  .recent-link:focus-visible .focus-title {
    display: block;
    pointer-events: none;
    position: absolute;
    inset-inline: 0;
    top: 100%;
    z-index: 30;
    padding: 0.4rem;
    white-space: normal;
    overflow-wrap: anywhere;
    background: var(--surface);
    border: 1px solid var(--border);
    border-radius: 0.25rem;
  }
  @media (hover: hover) and (pointer: fine) {
    .sidebar-overflow { opacity: 0; }
    .sidebar-row:hover .sidebar-overflow,
    .sidebar-row:focus-within .sidebar-overflow { opacity: 1; }
  }
  @media (pointer: coarse) {
    .sidebar-overflow { min-width: 44px; min-height: 44px; }
  }
</style>

<script lang="ts">
  import {
    me,
    clearSession,
    sessionKey,
    listProjects,
    listIssues,
    listModules,
    listPages,
    listPlans,
    getSidebarLayout,
    moveSidebarItem,
    createProjectGroup,
    renameProjectGroup,
    deleteProjectGroup,
    getAttention,
    type Project,
    type ProjectGroup,
    type SidebarItem,
    type SidebarLayout,
    type SidebarMove,
    type Issue,
    type Module,
    type Page,
    type Plan,
  } from "./api";
  import { loadCollapsedGroups, saveCollapsedGroups, NEW_GROUP } from "./projectGroups";
  import {
    DEFAULT_MAX_DEPTH, applyMove, ancestors, buildTree, canHoldSubfolder, checkMove, flatProjects,
    folderChoices, hidingFolder, indexTree, itemOf, keyOf, moveOut, newFolderSlot, resolveDrop,
    sameItem, stepMove, wrapFits, zoneAt, type DropResult, type DropZone, type FolderNode, type TreeNode,
  } from "./sidebarTree";
  import ProjectIcon from "./ProjectIcon.svelte";
  import MobileNav from "./MobileNav.svelte";
  import CommandPalette from "./CommandPalette.svelte";
  import ShortcutHelp from "./ShortcutHelp.svelte";
  import { themePreference, resolvedTheme, setPreference } from "./theme";
  import { currentUser, getUserRevision, publishUser } from "./userState";
  import { mobileNavState } from "./mobileNavState.svelte";
  import { navLink } from "./navLink";
  import { List, LayoutGrid, FileText, Plus, Layers, History, ListChecks, LayoutDashboard, Search, ChevronRight, Sun, Moon, Monitor, Menu, Home, TrendingUp, HelpCircle, Folder, FolderOpen, FolderPlus, FolderMinus, FolderInput, Pencil, Trash2, PanelLeftClose, PanelLeftOpen, Waypoints, Paperclip } from "lucide-svelte";
  import { onDestroy, setContext, untrack, tick } from "svelte";
  import { Ellipsis, ArrowUp, ArrowDown, Inbox, UserCheck, Bot, Settings2, LogIn } from "lucide-svelte";
  import { peekState } from "./issues/peek.svelte";
  import PeekPanel from "./issues/PeekPanel.svelte"; // LIF-248: hoisted here so it's available on every route
  import PagePeekPanel from "./pages/PagePeekPanel.svelte"; // pages sibling of PeekPanel, same reasoning
  import { contextMenuState, openContextMenu, closeContextMenu } from "./contextMenuState.svelte";
  import { toast } from "./toast/toast.svelte";
  import ContextMenu from "./ContextMenu.svelte"; // LIF-248
  import { commandPaletteState } from "./commandPaletteState.svelte";
  import { toggleShortcutHelp } from "./shortcutHelpState.svelte";
  import { isTypingContext } from "./shortcuts";
  import { loadProjectRole } from "./projectRole.svelte"; // LIF-234
  import { startAutoRefresh } from "./autoRefresh.svelte";
  import { loadActiveAgentCount } from "./agents/activeCount"; // LIF-544
  import { loginRouteFor } from "./loginRedirect";
  import { isSignedOut } from "./signedOut"; // LIF-583
  import SignInButton from "./SignInButton.svelte";
  import { buildEntries } from "./inbox/inbox";
  import { ensureServerClock, serverDay } from "./serverClock.svelte";
  import {
    clampSidebarWidth,
    loadSidebarWidthPreference,
    sidebarSizing,
    observeSidebarFontSize,
    saveSidebarWidth,
    loadSidebarCollapsed,
    saveSidebarCollapsed,
    SIDEBAR_DEFAULT_WIDTH,
  } from "./sidebarWidth";

  // LIF-583: a signed-out reader gets this same shell around a published
  // project, minus everything that belongs to an account: the workspace
  // views, groups, creation, reordering, the palette and the user footer.
  // Read once: App remounts Layout when the answer changes.
  const signedOut = isSignedOut();

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

  // Saved widths remain physical CSS pixels. Only the default and minimum
  // follow text size, and a temporary clamp never overwrites the preference.
  let preferredSidebarWidth = $state(loadSidebarWidthPreference());
  let sidebarFontSize = $state(16);
  let sidebarMetrics = $derived(sidebarSizing(preferredSidebarWidth, sidebarFontSize));
  let sidebarWidth = $derived(sidebarMetrics.width);
  let sidebarResizing = $state(false);
  let sidebarPointerId: number | null = null;
  let sidebarDragStartX = 0;
  let sidebarDragStartWidth = SIDEBAR_DEFAULT_WIDTH;
  let sidebarDragChanged = false;
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
    sidebarDragChanged = false;
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
    const next = clampSidebarWidth(
      sidebarDragStartWidth + event.clientX - sidebarDragStartX,
      sidebarMetrics.min, sidebarMetrics.max,
    );
    if (next !== sidebarWidth) {
      preferredSidebarWidth = next;
      sidebarDragChanged = true;
    }
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
    if (sidebarDragChanged) saveSidebarWidth(preferredSidebarWidth);
  }

  function resetSidebarWidth() {
    preferredSidebarWidth = null;
    saveSidebarWidth(null);
  }

  function handleSidebarResizeKeydown(event: KeyboardEvent) {
    const delta = event.key === "ArrowLeft" ? -10 : event.key === "ArrowRight" ? 10 : 0;
    if (delta === 0) return;

    event.preventDefault();
    preferredSidebarWidth = clampSidebarWidth(sidebarWidth + delta, sidebarMetrics.min, sidebarMetrics.max);
    saveSidebarWidth(preferredSidebarWidth);
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
  // LIF-537: collapsed no longer means hidden. The sidebar folds into an icon
  // rail inside the same chrome, so every destination stays one click away.
  let sidebarCollapsed = $state(loadSidebarCollapsed());
  function toggleSidebar() {
    sidebarCollapsed = !sidebarCollapsed;
    saveSidebarCollapsed(sidebarCollapsed);
    if (railTip !== null) railTip = null;
  }

  // The rail's nav scrolls, which would clip a tooltip positioned inside it,
  // so a single fixed tooltip follows whichever rail control is hovered.
  let railTip = $state<{ text: string; top: number; left: number } | null>(null);
  function tip(node: HTMLElement, text: string) {
    let label = text;
    const show = () => {
      const r = node.getBoundingClientRect();
      railTip = { text: label, top: r.top + r.height / 2, left: r.right + 8 };
    };
    // A focused control blurs while the rail unmounts, inside Svelte's
    // teardown, where writing state is forbidden even when it is a no-op.
    const hide = () => { if (railTip !== null) railTip = null; };
    node.addEventListener("pointerenter", show);
    node.addEventListener("pointerleave", hide);
    node.addEventListener("focus", show);
    node.addEventListener("blur", hide);
    node.addEventListener("click", hide);
    return {
      update(next: string) { label = next; },
      destroy() {
        node.removeEventListener("pointerenter", show);
        node.removeEventListener("pointerleave", hide);
        node.removeEventListener("focus", show);
        node.removeEventListener("blur", hide);
        node.removeEventListener("click", hide);
      },
    };
  }

  const modKey = typeof navigator !== "undefined" && /Mac|iPhone|iPad/.test(navigator.platform) ? "⌘" : "Ctrl";

  // ── Workspace counts (LIF-538) ──────────────────────────────
  // One attention read feeds both badges: Inbox counts every issue waiting on
  // the caller, Assigned to me counts the ones that name them. The endpoint
  // caps each group at 50, hence the "50+" display.
  let inboxCount = $state<number | null>(null);
  let assignedCount = $state<number | null>(null);
  // LIF-544: agents with an action in the last 10 minutes.
  let activeAgents = $state<number | null>(null);
  let countsAt = 0;
  let countsRequest = 0;
  async function refreshCounts(force = false) {
    if (signedOut) return;
    if (!force && Date.now() - countsAt < 15_000) return;
    countsAt = Date.now();
    const request = ++countsRequest;
    const [res, agents] = await Promise.all([getAttention(), loadActiveAgentCount()]);
    if (request !== countsRequest) return;
    activeAgents = agents;
    if (!res.ok) return;
    const { assigned, human, waiting } = res.data;
    // Only date waits need the server's day; skip the clock read otherwise.
    if ([...assigned, ...human, ...waiting].some((i) => i.waits?.some((w) => w.kind === "date"))) {
      ensureServerClock();
    }
    // Issues put off until a later day are not counted until they come back.
    inboxCount = buildEntries(res.data, serverDay(Date.now())).filter((e) => !e.later).length;
    assignedCount = assigned.length;
  }
  function countLabel(n: number): string {
    return n >= 50 ? "50+" : String(n);
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

  // ── Per-user sidebar folders ────────────────────────────────
  // The server's layout is the tree: folders nest, and each level orders its
  // folders and projects together. `tree` joins it with the project list, so
  // a project the layout doesn't place still shows, last at the top level.
  let layout = $state<SidebarLayout | null>(null);
  let groupsLoaded = $state(false);
  let collapsedGroups = $state<Set<number>>(loadCollapsedGroups());
  let tree = $derived(buildTree(projects, layout));
  let treeIndex = $derived(indexTree(tree));
  let groups = $derived<ProjectGroup[]>(layout?.groups ?? []);
  let maxDepth = $derived(layout?.max_depth ?? DEFAULT_MAX_DEPTH);
  let railProjects = $derived(flatProjects(tree));

  function folderName(id: number | null): string {
    return groups.find((g) => g.id === id)?.name ?? "";
  }
  function itemName(item: SidebarItem): string {
    return item.kind === "folder" ? folderName(item.id) : projects.find((p) => p.id === item.id)?.name ?? "";
  }

  function toggleGroup(id: number) {
    const next = new Set(collapsedGroups);
    if (!next.delete(id)) next.add(id);
    collapsedGroups = next;
    saveCollapsedGroups(next);
  }
  function expandFolders(ids: number[]) {
    if (!ids.some((id) => collapsedGroups.has(id))) return;
    const next = new Set(collapsedGroups);
    for (const id of ids) next.delete(id);
    collapsedGroups = next;
    saveCollapsedGroups(next);
  }

  // ── Managing folders ───────────────────────────────────────
  // The id being renamed, or NEW_GROUP while creating one, which then goes
  // in `newFolderParent` and, when it came from "Move to folder", wraps
  // `newFolderContains` in the same write.
  let editingGroupId = $state<number | null>(null);
  let newFolderParent = $state<number | null>(null);
  let newFolderContains = $state<SidebarItem | null>(null);
  let draftGroupName = $state("");
  let groupEditError = $state("");
  let groupSaving = $state(false);
  let orderError = $state("");
  let orderSaving = $state(false);
  let refreshRequest = 0;
  let groupInput = $state<HTMLInputElement | null>(null);
  let groupEditTrigger: HTMLElement | null = null;
  // Screen readers hear where a move put things; the tree itself says it
  // visually.
  let liveMessage = $state("");

  function menuNavigate(path: string) {
    if (navOpen) mobileNav?.navigateTo(path);
    else navigate(path);
  }

  /** The one way the sidebar changes its arrangement: check, paint, save,
   *  and on failure put the previous tree back with a visible message. */
  async function runMove(move: SidebarMove, reveal = false): Promise<boolean> {
    if (!layout || orderSaving) return false;
    const check = checkMove(treeIndex, move, maxDepth);
    if (check.kind === "refused") {
      toast(check.reason, { kind: "error" });
      return false;
    }
    if (check.kind === "noop") return true;
    const name = itemName(move.item);
    refreshRequest++;
    const previous = layout;
    orderSaving = true;
    orderError = "";
    layout = applyMove(layout, move);
    if (reveal && move.parent_id !== null) {
      expandFolders([...ancestors(treeIndex, { kind: "folder", id: move.parent_id }), move.parent_id]);
    }
    const res = await moveSidebarItem(move);
    orderSaving = false;
    if (res.ok) {
      layout = res.data;
      liveMessage = `Moved ${name} ${move.parent_id === null ? "to the top level" : `into ${folderName(move.parent_id)}`}.`;
      return true;
    }
    layout = previous;
    orderError = `${name || "That item"} wasn't moved: ${res.error}`;
    return false;
  }

  function moveItems(item: SidebarItem) {
    const up = stepMove(treeIndex, item, "up");
    const down = stepMove(treeIndex, item, "down");
    const out = moveOut(treeIndex, item);
    const busy = orderSaving || !groupsLoaded;
    return [
      { label: "Move up", icon: ArrowUp, disabled: busy || !up, action: () => { if (up) void runMove(up); } },
      { label: "Move down", icon: ArrowDown, disabled: busy || !down, action: () => { if (down) void runMove(down); } },
      { label: "Move to folder…", icon: FolderInput, disabled: busy, action: () => openMoveMenu(item) },
      ...(out
        ? [{
            label: `Move out of ${folderName(treeIndex.get(keyOf(item))?.parent ?? null)}`,
            icon: FolderMinus,
            disabled: busy,
            action: () => void runMove(out),
          }]
        : []),
    ];
  }

  // The menu that opened "Move to folder…", so its replacement opens in the
  // same place and returns focus to the same row.
  let menuAnchor: { x: number; y: number; trigger: HTMLElement | null } = { x: 0, y: 0, trigger: null };

  function anchorFrom(e: MouseEvent) {
    const trigger = e.currentTarget as HTMLElement;
    const rect = trigger.getBoundingClientRect();
    menuAnchor = {
      x: e.type === "contextmenu" ? e.clientX : rect.right,
      y: e.type === "contextmenu" ? e.clientY : rect.bottom,
      trigger,
    };
    return menuAnchor;
  }

  /** Every destination, with the reason on the ones that can't be picked;
   *  the keyboard and touch way to do what dragging does. */
  function openMoveMenu(item: SidebarItem) {
    const parent = treeIndex.get(keyOf(item))?.parent ?? null;
    const fits = wrapFits(treeIndex, item, maxDepth);
    openContextMenu(menuAnchor.x, menuAnchor.y, [
      ...folderChoices(tree, treeIndex, item, maxDepth).map((choice) => ({
        label: choice.name,
        icon: choice.id === null ? Home : Folder,
        indent: choice.level,
        hint: choice.disabled ?? undefined,
        disabled: !!choice.disabled,
        action: () => void runMove({ item, parent_id: choice.id, before: null }, true),
      })),
      {
        label: "New folder…",
        icon: FolderPlus,
        hint: fits ? undefined : `${maxDepth} levels max`,
        disabled: !fits,
        action: () => startCreatingGroup(parent, item),
      },
    ], menuAnchor.trigger);
  }

  function openProjectMenu(e: MouseEvent, project: Project) {
    e.preventDefault();
    e.stopPropagation();
    const { x, y, trigger } = anchorFrom(e);
    openContextMenu(x, y, [
      ...moveItems({ kind: "project", id: project.id }),
      {
        label: "Project settings",
        icon: Settings2,
        href: `#/${project.identifier}/settings`,
        action: () => menuNavigate(`/${project.identifier}/settings`),
      },
    ], trigger);
  }

  const projectViews = [
    ["overview", "Overview", LayoutDashboard],
    ["issues", "Issues", List],
    ["board", "Board", LayoutGrid],
    ["modules", "Modules", Layers],
    ["pages", "Pages", FileText],
    ["plans", "Plans", ListChecks],
    ["graph", "Graph", Waypoints],
    ["files", "Files", Paperclip],
    ["activity", "Activity", History],
    ["insights", "Insights", TrendingUp],
  ] as const;

  function openRailProjectMenu(e: MouseEvent, project: Project) {
    e.stopPropagation();
    const trigger = e.currentTarget as HTMLElement;
    const rect = trigger.getBoundingClientRect();
    openContextMenu(rect.right + 6, rect.top, projectViews.map(([slug, label, icon]) => {
      const path = `/${project.identifier}/${slug}`;
      return { label, icon, href: `#${path}`, action: () => navigate(path) };
    }), trigger);
  }

  function openGroupMenu(e: MouseEvent, group: ProjectGroup) {
    e.preventDefault();
    e.stopPropagation();
    const { x, y, trigger } = anchorFrom(e);
    const roomy = canHoldSubfolder(treeIndex, group.id, maxDepth);
    openContextMenu(x, y, [
      ...moveItems({ kind: "folder", id: group.id }),
      {
        label: "New subfolder",
        icon: FolderPlus,
        hint: roomy ? undefined : `${maxDepth} levels max`,
        disabled: !roomy,
        action: () => startCreatingGroup(group.id),
      },
      { label: "Rename", icon: Pencil, action: () => startRenamingGroup(group) },
      { label: "Delete folder", icon: Trash2, action: () => void removeGroup(group) },
    ], trigger);
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
      { label: "New folder", icon: FolderPlus, action: () => startCreatingGroup() },
    ], e.currentTarget as HTMLElement);
  }

  function startRenamingGroup(group: ProjectGroup) {
    groupEditTrigger = document.activeElement as HTMLElement | null;
    groupEditError = "";
    editingGroupId = group.id;
    draftGroupName = group.name;
  }

  function startCreatingGroup(parent: number | null = null, contains: SidebarItem | null = null) {
    groupEditTrigger = document.activeElement as HTMLElement | null;
    groupEditError = "";
    editingGroupId = NEW_GROUP;
    newFolderParent = parent;
    newFolderContains = contains;
    draftGroupName = "";
    // The name field shows where the folder will be, so open the way there.
    if (parent !== null) expandFolders([...ancestors(treeIndex, { kind: "folder", id: parent }), parent]);
    // The rail has no room for the field.
    if (sidebarCollapsed && !navOpen) toggleSidebar();
  }

  function cancelGroupEdit() {
    if (groupSaving) return;
    const id = editingGroupId;
    groupEditError = "";
    editingGroupId = null;
    newFolderParent = null;
    newFolderContains = null;
    if (!navOpen) void tick().then(() => {
      const target = groupEditTrigger?.isConnected ? groupEditTrigger
        : document.querySelector<HTMLElement>(`[data-sidebar-group-actions="${id}"]`);
      target?.focus({ preventScroll: true });
    });
  }

  async function commitGroupName() {
    if (groupSaving) return false;
    const name = draftGroupName.trim();
    const editing = editingGroupId;
    if (!name || editing === null) {
      groupEditError = "Enter a folder name.";
      return false;
    }
    groupSaving = true;
    groupEditError = "";
    const res =
      editing === NEW_GROUP
        ? await createProjectGroup(name, newFolderParent, newFolderContains)
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
    liveMessage = editing === NEW_GROUP ? `Created folder ${res.data.name}.` : `Renamed to ${res.data.name}.`;
    await refreshProjects();
    return true;
  }

  // Deleting a folder never touches the projects inside it: they and any
  // subfolders take its place in its parent, so there is nothing to confirm.
  async function removeGroup(group: ProjectGroup) {
    const res = await deleteProjectGroup(group.id);
    if (res.ok) {
      liveMessage = `Deleted folder ${group.name}. What it held moved up a level.`;
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
  let routeEffectMounted = false;
  $effect(() => {
    route; // track route changes
    // loadUser already fetches projects on mount.
    if (!routeEffectMounted) {
      routeEffectMounted = true;
      return;
    }
    refreshProjects();
    void refreshCounts();
  });

  $effect(() =>
    startAutoRefresh({
      refresh: () => refreshCounts(true),
      // Agent status decays with time alone, and reads emit no events.
      intervalMs: 60_000,
      realtimeDebounceMs: 1_000,
      realtimeMaxWaitMs: 5_000,
      shouldRefresh: (event) =>
        event.type === "resync.required" || event.type.startsWith("issue."),
    }),
  );

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
    if (signedOut) {
      await refreshProjects();
      loading = false;
      return;
    }
    const revision = getUserRevision();
    const session = sessionKey();
    const res = await me();
    if (session !== sessionKey()) return;
    if (res.ok) {
      publishUser(res.data, revision);
    } else {
      clearSession();
      currentUser.set(null);
      navigate(loginRouteFor(route));
      return;
    }
    void refreshCounts(true);
    await refreshProjects();
    loading = false;
  }

  async function refreshProjects() {
    // Never swap the tree out from under a drag or a save in flight: the
    // drop and the save each settle from the server's own answer.
    if (dragActive || orderSaving) return;
    const request = ++refreshRequest;
    groupsLoaded = false;
    // Folders are an account's own filing; a signed-out reader has none.
    const [projectsRes, layoutRes] = await Promise.all([
      listProjects(),
      signedOut
        ? Promise.resolve({ ok: true as const, data: { root: [], groups: [], max_depth: DEFAULT_MAX_DEPTH } as SidebarLayout })
        : getSidebarLayout(),
    ]);
    if (request !== refreshRequest || dragActive || orderSaving) return;
    if (projectsRes.ok) {
      projects = projectsRes.data;
    }
    if (layoutRes.ok) {
      layout = layoutRes.data;
      groupsLoaded = true;
    }
  }

  // ── Dragging in the tree ────────────────────────────────────
  // A pointer drag, not HTML5 drag and drop, so the dragged item travels as
  // one compact row whatever it holds, and the drop can mean before, after or
  // inside. Only the dragged item folds its own contents away; the rest of
  // the tree keeps its shape so nothing jumps under the cursor. Mouse and pen
  // only: touch scrolls, and every move is also in the item's menu.
  let dragActive = $state(false);
  interface Drag {
    item: SidebarItem;
    pointerId: number;
    startX: number;
    startY: number;
    x: number;
    y: number;
    result: DropResult | null;
    /** Whether the pointer has been over anything but the dragged row. */
    leftOrigin: boolean;
    indicator: { top: number; left: number; width: number; height: number; box: boolean; refused: boolean } | null;
  }
  let drag = $state<Drag | null>(null);
  // Folders opened by hovering during this drag; closed again after the drop
  // unless the dropped item landed inside one of them.
  let springOpen = $state(new Set<number>());
  let springTimer: ReturnType<typeof setTimeout> | null = null;
  let springTarget: number | null = null;
  let scrollEl = $state<HTMLElement | null>(null);
  let scrollFrame = 0;
  const SPRING_MS = 600;
  const DRAG_SLOP = 5;

  function folderOpen(id: number): boolean {
    if (dragActive && drag && sameItem(drag.item, { kind: "folder", id })) return false;
    return springOpen.has(id) || !collapsedGroups.has(id);
  }

  function dragSource(node: HTMLElement, item: SidebarItem) {
    let current = item;
    const down = (e: PointerEvent) => {
      if (e.button !== 0 || e.pointerType === "touch" || signedOut || !groupsLoaded || orderSaving || drag) return;
      if ((e.target as HTMLElement).closest("input, form")) return;
      drag = { item: current, pointerId: e.pointerId, startX: e.clientX, startY: e.clientY, x: e.clientX, y: e.clientY, result: null, leftOrigin: false, indicator: null };
      window.addEventListener("pointermove", onDragMove);
      window.addEventListener("pointerup", onDragEnd);
      window.addEventListener("pointercancel", cancelDrag);
      window.addEventListener("keydown", onDragKey, true);
      window.addEventListener("blur", cancelDrag);
    };
    // Links and the native image drag would start HTML5 drag and drop.
    const native = (e: DragEvent) => e.preventDefault();
    node.addEventListener("pointerdown", down);
    node.addEventListener("dragstart", native);
    return {
      update(next: SidebarItem) { current = next; },
      destroy() {
        node.removeEventListener("pointerdown", down);
        node.removeEventListener("dragstart", native);
      },
    };
  }

  function onDragMove(e: PointerEvent) {
    if (!drag || e.pointerId !== drag.pointerId) return;
    drag.x = e.clientX;
    drag.y = e.clientY;
    if (!dragActive) {
      if (Math.hypot(e.clientX - drag.startX, e.clientY - drag.startY) < DRAG_SLOP) return;
      dragActive = true;
      closeContextMenu(false);
      document.body.classList.add("sidebar-dragging");
      scrollFrame = requestAnimationFrame(autoScroll);
    }
    e.preventDefault();
    updateDropTarget();
  }

  function onDragKey(e: KeyboardEvent) {
    if (e.key !== "Escape" || !drag || !dragActive) return;
    e.preventDefault();
    e.stopPropagation();
    finishDrag([], true);
  }

  // Near the top or bottom edge, scroll the tree, and re-aim since the rows
  // moved under a still pointer.
  function autoScroll() {
    if (!dragActive || !drag || !scrollEl) return;
    const rect = scrollEl.getBoundingClientRect();
    const edge = 32;
    const inside = drag.x >= rect.left && drag.x <= rect.right;
    const step = !inside ? 0
      : drag.y < rect.top + edge ? -Math.ceil((rect.top + edge - drag.y) / 4)
      : drag.y > rect.bottom - edge ? Math.ceil((drag.y - rect.bottom + edge) / 4) : 0;
    if (step) {
      const before = scrollEl.scrollTop;
      scrollEl.scrollTop += step;
      if (scrollEl.scrollTop !== before) updateDropTarget();
    }
    scrollFrame = requestAnimationFrame(autoScroll);
  }

  /** Aim at the row under the pointer. Gaps between rows belong to the row
   *  below (before it); a project's open sub-tabs belong to the project
   *  (after it); space under the last row is the end of the top level. */
  function updateDropTarget() {
    if (!drag || !scrollEl) return;
    const { x, y } = drag;
    const box = scrollEl.getBoundingClientRect();
    let target: SidebarItem | null | undefined;
    let zone: DropZone = "after";
    let row: HTMLElement | null = null;
    if (x >= box.left && x <= box.right && y >= box.top && y <= box.bottom) {
      target = null;
      for (const el of scrollEl.querySelectorAll<HTMLElement>("[data-drop-row]")) {
        if (!el.getClientRects().length) continue;
        const rect = el.getBoundingClientRect();
        const kind = el.dataset.dropKind as SidebarItem["kind"] | "into";
        const extent = kind === "project" ? el.closest<HTMLElement>("[data-drop-entry]")?.getBoundingClientRect().bottom ?? rect.bottom : rect.bottom;
        if (y >= extent) continue;
        row = el;
        const id = Number(el.dataset.dropId);
        if (kind === "into") { target = { kind: "folder", id }; zone = "inside"; }
        else {
          target = { kind, id };
          zone = y < rect.top ? "before" : y <= rect.bottom ? zoneAt(kind, y - rect.top, rect.height) : "after";
        }
        break;
      }
    }
    if (target === undefined) {
      drag.result = null;
      drag.indicator = null;
      armSpring(null);
      return;
    }
    // A drag starts on its own row; it only reads as "inside itself" once
    // the pointer has gone elsewhere and come back.
    if (!drag.leftOrigin) {
      if (target && sameItem(target, drag.item)) {
        drag.result = { kind: "noop" };
        drag.indicator = null;
        armSpring(null);
        return;
      }
      drag.leftOrigin = true;
    }
    const result = resolveDrop(treeIndex, tree, drag.item, target, zone, folderOpen, maxDepth);
    drag.result = result;
    armSpring(target?.kind === "folder" && zone === "inside" && !folderOpen(target.id) && !sameItem(target, drag.item) ? target.id : null);
    if (result.kind === "noop") { drag.indicator = null; return; }
    const refused = result.kind === "refused";
    const at = (top: number, left: number, width: number, height = 2, boxed = false) => ({
      top: top - box.top + scrollEl!.scrollTop, left: left - box.left, width, height, box: boxed, refused,
    });
    if (!row || target === null) {
      const list = scrollEl.querySelector<HTMLElement>("[data-tree-root]");
      const r = (list ?? scrollEl).getBoundingClientRect();
      drag.indicator = at(r.bottom, r.left, r.width);
      return;
    }
    const rect = row.getBoundingClientRect();
    if (zone === "inside") drag.indicator = at(rect.top, rect.left, rect.width, rect.height, true);
    else if (zone === "before") drag.indicator = at(rect.top - 1, rect.left, rect.width);
    else if (result.kind === "move" && target.kind === "folder" && result.move.parent_id === target.id) {
      // "After" an open folder's heading is its first slot: indent to show it.
      const indent = 1.4375 * sidebarFontSize;
      drag.indicator = at(rect.bottom, rect.left + indent, rect.width - indent);
    } else {
      const bottom = row.dataset.dropKind === "project"
        ? row.closest<HTMLElement>("[data-drop-entry]")?.getBoundingClientRect().bottom ?? rect.bottom
        : rect.bottom;
      drag.indicator = at(bottom, rect.left, rect.width);
    }
  }

  function armSpring(folder: number | null) {
    if (folder === springTarget) return;
    if (springTimer) clearTimeout(springTimer);
    springTimer = null;
    springTarget = folder;
    if (folder === null) return;
    springTimer = setTimeout(() => {
      springTimer = null;
      if (!dragActive || springTarget !== folder) return;
      springOpen = new Set([...springOpen, folder]);
      void tick().then(updateDropTarget);
    }, SPRING_MS);
  }

  function stopDragListeners() {
    window.removeEventListener("pointermove", onDragMove);
    window.removeEventListener("pointerup", onDragEnd);
    window.removeEventListener("pointercancel", cancelDrag);
    window.removeEventListener("keydown", onDragKey, true);
    window.removeEventListener("blur", cancelDrag);
    cancelAnimationFrame(scrollFrame);
    armSpring(null);
    document.body.classList.remove("sidebar-dragging");
  }

  /** `held`: the button is still down (Escape), so the click that would
   *  follow its release is still to come. */
  function finishDrag(keepOpen: number[] = [], held = false) {
    stopDragListeners();
    const started = dragActive;
    drag = null;
    dragActive = false;
    // Spring-opened folders close again, except the way to the dropped item.
    if (keepOpen.length) expandFolders(keepOpen.filter((id) => springOpen.has(id) || !collapsedGroups.has(id)));
    springOpen = new Set();
    if (started) {
      // The release would otherwise click the row it ended on.
      const swallow = (e: MouseEvent) => { e.preventDefault(); e.stopPropagation(); };
      const stop = () => setTimeout(() => window.removeEventListener("click", swallow, { capture: true }), 0);
      window.addEventListener("click", swallow, { capture: true, once: true });
      if (held) window.addEventListener("pointerup", stop, { capture: true, once: true });
      else stop();
    }
  }

  function cancelDrag() {
    finishDrag();
  }

  async function onDragEnd(e: PointerEvent) {
    if (!drag || e.pointerId !== drag.pointerId) return;
    if (!dragActive) { finishDrag(); return; }
    updateDropTarget();
    const result = drag.result;
    const name = itemName(drag.item);
    if (result?.kind === "refused") {
      finishDrag();
      toast(`Can't move ${name || "that"} there. ${result.reason}`, { kind: "error" });
      return;
    }
    if (result?.kind !== "move") { finishDrag(); return; }
    const parent = result.move.parent_id;
    const keep = parent === null ? [] : [...ancestors(treeIndex, { kind: "folder", id: parent }), parent].filter((id) => springOpen.has(id));
    finishDrag(keep);
    await runMove(result.move);
  }

  onDestroy(stopDragListeners);

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
    const path = ancestors(treeIndex, { kind: "project", id: project.id });
    if (revealedProjectId === project.id) return;
    revealedProjectId = project.id;
    untrack(() => {
      expandedProjects = new Set([...expandedProjects, project.id]);
      expandFolders(path);
    });
    void tick().then(() => {
      if (revealedProjectId !== project.id || !matchMedia("(min-width: 768px)").matches) return;
      document.querySelector<HTMLElement>(`[data-sidebar-project="${project.id}"]`)?.scrollIntoView({ block: "nearest" });
    });
  });
  // While dragging, only the dragged project folds its sub-tabs away; it
  // opens again on drop because `expandedProjects` is never touched.
  function subnavOpen(project: Project): boolean {
    if (dragActive && drag && sameItem(drag.item, { kind: "project", id: project.id })) return false;
    return expandedProjects.has(project.id);
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
  // The collapsed folder, outermost first, that hides the current project.
  let hiddenActiveFolder = $derived(
    activeProjectRecord ? hidingFolder(treeIndex, { kind: "project", id: activeProjectRecord.id }, collapsedGroups) : null,
  );
  let mobileSection = $derived.by(() => {
    if (route === "/") return "Home";
    if (route === "/inbox") return "Inbox";
    if (route === "/assigned") return "Assigned to me";
    if (route === "/agents") return "Agent activity";
    if (isActive("/settings")) return "Settings";
    if (route === "/projects/new") return "New project";
    if (route === "/projects/import") return "Import archive";
    if (!activeProject) return null;
    const rest = route.slice(activeProject.length + 1);
    const slug = rest.split("/")[1] ?? "";
    const labels: Record<string, string> = {
      overview: "Overview",
      settings: "Settings",
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
{:else if user || signedOut}
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
      class="sidebar-theme desktop-sidebar hidden md:flex
             {sidebarCollapsed ? 'sidebar-rail w-[3.5rem]' : 'w-[var(--sidebar-w)]'}
             shrink-0 relative flex-col bg-[var(--chrome)] select-none"
      style={`--sidebar-w: ${sidebarWidth}px`}
    >
      <span aria-hidden="true" class="absolute invisible h-0 pointer-events-none" style="width: 1rem"
        use:observeSidebarFontSize={(size) => sidebarFontSize = size}></span>
      {#if sidebarCollapsed}
        {@render rail()}
      {:else}
      <!-- Brand header -->
      <div class="px-3 pt-3 pb-2 flex items-center gap-1.5">
        <a
          href="https://github.com/VoidNullable/lific"
          target="_blank"
          rel="noopener noreferrer"
          title={`Lific v${__APP_VERSION__} on GitHub`}
          class="sidebar-brand flex flex-1 min-w-0 items-center gap-2.5 px-1 py-1 transition-colors"
        >
          <img src="/logo.webp" alt="" width="26" height="26" class="rounded-md shrink-0" />
          <span class="font-display text-heading tracking-tight text-[var(--text)] leading-none flex-1">
            Lific
          </span>
        </a>
        <!-- LIF-360: collapse control. Sits beside the brand rather than in
             the footer so its position mirrors the expand control that takes
             its place in the topbar. -->
        <button
          class="size-7 shrink-0 grid place-items-center rounded-md
                 text-[var(--text-faint)] hover:text-[var(--text)]
                 hover:bg-[var(--sidebar-hover)] transition-colors"
          onclick={toggleSidebar}
          title="Collapse sidebar  ·  ⌘\\"
          aria-label="Collapse sidebar"
        >
          <PanelLeftClose size={15} />
        </button>
      </div>

      <!-- Jump-to / command palette trigger -->
      {#if !signedOut}
      <div class="sidebar-launcher-wrap px-2">
        <button
          class="sidebar-launcher w-full h-10 flex items-center gap-3 pl-[0.6875rem] pr-2 rounded-lg transition-colors"
          onclick={() => palette?.openPalette()}
        >
          <Search size={17} class="shrink-0" />
          <span class="flex-1 min-w-0 truncate text-left text-body">Search…</span>
          <span class="flex items-center gap-0.5 shrink-0" aria-hidden="true">
            <kbd class="sidebar-key">{modKey}</kbd><kbd class="sidebar-key">K</kbd>
          </span>
        </button>
      </div>
      {/if}

      <!-- Navigation. The workspace views stay put; only the project tree
           below the rule scrolls. -->
      <nav class="flex-1 min-h-0 flex flex-col">
        <!-- LIF-538: cross-project views for the signed-in person. Home keeps
             its LIF-237 role as the landing dashboard. -->
        {#snippet workspaceItem(href: string, label: string, Icon: typeof Home, count: number | null, tone: "accent" | "plain", key: string)}
          <a href={`#${href}`} use:navLink={navigate} aria-current={route === href ? "page" : undefined}
            data-workspace={key}
            class="sidebar-destination sidebar-workspace-item w-full h-10 flex items-center gap-3 pl-[0.6875rem] pr-2 rounded-md
                   text-left text-body transition-colors"
          >
            <Icon size={18} class="shrink-0" />
            <span class="flex-1 min-w-0 truncate">{label}</span>
            {#if count}
              <span class="sidebar-count" data-tone={tone}>{countLabel(count)}</span>
            {/if}
          </a>
        {/snippet}
        {#if signedOut}
          <span class="sidebar-section-label block px-4 pt-2 pb-1 text-micro font-semibold uppercase text-[var(--text-faint)]">
            Projects
          </span>
        {:else}
        <div class="sidebar-workspace shrink-0 px-2 pt-1">
          <span class="sidebar-section-label block px-2 pb-1 text-micro font-semibold uppercase text-[var(--text-faint)]">
            Workspace
          </span>
          {@render workspaceItem("/", "Home", Home, null, "plain", "home")}
          {@render workspaceItem("/inbox", "Inbox", Inbox, inboxCount, "accent", "inbox")}
          {@render workspaceItem("/assigned", "Assigned to me", UserCheck, assignedCount, "plain", "assigned")}
          {@render workspaceItem("/agents", "Agent activity", Bot, activeAgents, "plain", "agents")}
        </div>
        <hr class="sidebar-section-rule shrink-0" />

        <!-- The header renders unconditionally: on a brand-new instance it is
             the way in. Folders are also made from a folder's or a project's
             menu. -->
        <div class="sidebar-new-project shrink-0 px-2 flex items-center gap-0.5">
          <a href="#/projects/new" use:navLink={navigate}
            aria-current={route === "/projects/new" ? "page" : undefined}
            class="sidebar-destination min-w-0 flex-1 h-9 flex items-center gap-2.5 pl-2 pr-2 rounded-md text-left text-body transition-colors"
          >
            <span class="sidebar-project-icon sidebar-new-project-icon rounded-md"><Plus size={15} /></span>
            <span class="flex-1 min-w-0 truncate">New project</span>
          </a>
          <button class="sidebar-new-folder size-8 shrink-0 grid place-items-center rounded-md text-[var(--text-faint)]
                         hover:text-[var(--text)] hover:bg-[var(--sidebar-hover)] transition-colors"
            title="New folder" aria-label="New folder" data-sidebar-new-folder
            onclick={() => startCreatingGroup()}><FolderPlus size={16} /></button>
        </div>
        {/if}

        <div class="sidebar-scroll relative flex-1 min-h-0 overflow-y-auto px-2 pb-1" bind:this={scrollEl}>

        <!-- One project entry: the pill plus its sub-nav, in one wrapper so
             a drag over its open sub-tabs still means "after this project".
             Identical wherever the project is filed. -->
        {#snippet projectEntry(project: Project)}
            {@const isProjectActive = activeProject === project.identifier}
            {@const open = subnavOpen(project)}
            {@const dragging = dragActive && sameItem(drag?.item, { kind: "project", id: project.id })}
            <div data-drop-entry data-dragging={dragging || undefined}>
            <div
              class="sidebar-row sidebar-project group w-full flex items-center rounded-md
                     text-left text-body font-medium text-[var(--text)] transition-colors"
              data-drop-row data-drop-kind="project" data-drop-id={project.id}
              use:dragSource={{ kind: "project", id: project.id }}
            >
              <!-- The icon leads the row so it sits where the collapsed rail
                   draws the same project; the disclosure moves to the end. -->
              <a href={`#/${project.identifier}/overview`} use:navLink={navigate}
                draggable="false"
                data-sidebar-project={project.id}
                aria-current={route === `/${project.identifier}/overview` ? "page" : undefined}
                title={project.name} class="sidebar-project-link min-w-0 flex-1 flex items-center gap-2.5 pl-2"
                oncontextmenu={signedOut ? undefined : (e) => openProjectMenu(e, project)}>
              {#if project.emoji}
                <span class="sidebar-project-icon">
                  <ProjectIcon value={project.emoji} size={18} />
                </span>
              {:else}
                <span
                  class="sidebar-project-icon sidebar-initials rounded-md text-caption font-medium tracking-tight"
                >
                  {project.identifier.slice(0, 2)}
                </span>
              {/if}
              <span class="truncate flex-1">{project.name}</span>
              </a>
              {#if !signedOut}
              <button class="sidebar-overflow size-7 shrink-0 grid place-items-center rounded-md text-[var(--text-faint)] hover:bg-[var(--sidebar-hover)]"
                aria-label={`Actions for ${project.name}`} aria-haspopup="menu"
                onclick={(e) => openProjectMenu(e, project)}><Ellipsis size={16} /></button>
              {/if}
              <button class="size-7 shrink-0 grid place-items-center rounded-md hover:bg-[var(--sidebar-hover)]"
                aria-label={`${open ? 'Collapse' : 'Expand'} ${project.name}`}
                aria-expanded={open} aria-controls={`project-nav-${project.id}`}
                onclick={() => toggleProject(project)}>
              <ChevronRight
                size={14}
                class="shrink-0 transition-transform
                       {open ? 'rotate-90' : ''}
                       {isProjectActive ? 'text-[var(--text-muted)]' : 'text-[var(--text-faint)] group-hover:text-[var(--text-muted)]'}"
              />
              </button>
            </div>

              <!-- Sub-nav: indented under the project with a vertical guide
                   line, matching the tree language used in Pages. -->
              <div id={`project-nav-${project.id}`} hidden={!open} class="project-subnav flex flex-col">
                {#snippet subItem(href: string, label: string, Icon: typeof List, alsoActiveOn?: string)}
                  {@const active = isActive(href) || (alsoActiveOn !== undefined && isActive(alsoActiveOn))}
                  <a href={`#${href}`} use:navLink={navigate} aria-current={active ? "page" : undefined}
                    class="sidebar-destination w-full h-8 flex items-center gap-2.5 px-2 rounded-md
                           text-left text-body-sm transition-colors"
                  >
                    <Icon size={16} class="shrink-0" />
                    {label}
                  </a>
                {/snippet}
                {#snippet recentItem(href: string, label: string, identifier: string | null)}
                  <a href={`#${href}`} use:navLink={navigate} aria-current={isActive(href) ? "page" : undefined}
                    title={identifier ? `${identifier}: ${label}` : label}
                    class="sidebar-destination recent-link relative w-full flex items-center gap-1 px-2 py-1 rounded-md
                           text-left text-caption transition-colors"
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
                {@render subItem(`/${project.identifier}/overview`, "Overview", LayoutDashboard, `/${project.identifier}/settings`)}
                {@render subItem(`/${project.identifier}/issues`, "Issues", List)}
                {@render subItem(`/${project.identifier}/board`, "Board", LayoutGrid)}
                {@render subItem(`/${project.identifier}/modules`, "Modules", Layers)}
                {@render subItem(`/${project.identifier}/pages`, "Pages", FileText)}
                {@render subItem(`/${project.identifier}/plans`, "Plans", ListChecks)}
                <!-- Daily views above the rule, reference views below it. -->
                <hr class="sidebar-subnav-rule" />
                {@render subItem(`/${project.identifier}/graph`, "Graph", Waypoints)}
                {@render subItem(`/${project.identifier}/files`, "Files", Paperclip)}
                {@render subItem(`/${project.identifier}/activity`, "Activity", History)}
                {@render subItem(`/${project.identifier}/insights`, "Insights", TrendingUp)}
                {#if isProjectActive && activeRecentSection}
                  <button class="sidebar-recents-heading flex items-center gap-1 px-2 py-1 text-caption text-[var(--text-faint)] rounded-md hover:bg-[var(--sidebar-hover)]"
                    aria-expanded={recentOpen} aria-controls={`recent-${project.id}`} onclick={() => recentOpen = !recentOpen}>
                    <ChevronRight size={12} class={recentOpen ? "rotate-90" : ""} /> Recent {activeRecentSection}
                  </button>
                  <div id={`recent-${project.id}`} hidden={!recentOpen} aria-busy={recentLoading !== null}>
                    {@render recentItems(activeRecentSection, project)}
                  </div>
                {/if}
              </div>
            </div>
        {/snippet}

        <!-- Escape cancels from the input or either form button. Rendered
             where the folder will appear, or in place of the one renamed. -->
        {#snippet groupNameInput()}
          <!-- svelte-ignore a11y_no_noninteractive_element_interactions -->
          <form class="px-1 py-1" onsubmit={(e) => { e.preventDefault(); void commitGroupName(); }}
            onkeydown={(e) => { if (e.key === "Escape") { e.stopPropagation(); e.preventDefault(); cancelGroupEdit(); } }}>
          <input
            class="w-full h-7 px-2 mb-0.5 rounded-md text-body-sm bg-[var(--bg)]
                   border border-[var(--border)] text-[var(--text)]"
            placeholder="Folder name"
            aria-label="Folder name" aria-invalid={!!groupEditError} aria-describedby={groupEditError ? "group-edit-error" : undefined}
            maxlength="80"
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

        {#if orderError}<p role="alert" class="px-2 py-1 text-caption text-[var(--error)] break-words">{orderError}</p>{/if}

        <!-- A folder: folder glyph where a project draws its icon, open or
             closed, and a count of every project inside it. -->
        {#snippet folderRow(node: FolderNode)}
          {@const group = node.folder}
          {@const open = folderOpen(group.id)}
          {@const dragging = dragActive && sameItem(drag?.item, { kind: "folder", id: group.id })}
          {#if editingGroupId === group.id}
            {@render groupNameInput()}
          {:else}
          <div class="sidebar-row sidebar-group-heading group flex items-center rounded-md"
            data-collapsed={!open || undefined} data-dragging={dragging || undefined}
            data-drop-row data-drop-kind="folder" data-drop-id={group.id}
            use:dragSource={{ kind: "folder", id: group.id }}>
          <button
            class="sidebar-folder min-w-0 flex-1 flex items-center gap-2.5 pl-2 pr-1 rounded-md
                   text-left text-body font-medium transition-colors"
            aria-expanded={open}
            aria-controls={`group-${group.id}`} title={group.name}
            onclick={() => toggleGroup(group.id)}
            oncontextmenu={signedOut ? undefined : (e) => openGroupMenu(e, group)}
          >
            <span class="sidebar-project-icon sidebar-folder-icon">
              {#if open}
                <FolderOpen size={18} />
              {:else}
                <Folder size={18} />
              {/if}
            </span>
            <span class="truncate flex-1">{group.name}</span>
            <span class="sidebar-folder-count text-micro tabular-nums" aria-hidden="true">{node.projectCount}</span>
          </button>
          <button class="sidebar-overflow size-7 shrink-0 grid place-items-center rounded-md text-[var(--text-faint)] hover:bg-[var(--sidebar-hover)]"
            data-sidebar-group-actions={group.id}
            aria-label={`Actions for folder ${group.name}`} aria-haspopup="menu" onclick={(e) => openGroupMenu(e, group)}><Ellipsis size={15} /></button>
          </div>
          {/if}
          {#if !open && !dragActive && hiddenActiveFolder === group.id && activeProjectRecord}
            <button class="ml-3 max-w-[calc(100%-0.75rem)] truncate text-caption text-[var(--accent)] px-2 py-1"
              title={`Show ${activeProjectRecord.name}`}
              onclick={() => expandFolders(ancestors(treeIndex, { kind: "project", id: activeProjectRecord!.id }))}>Current: {activeProjectRecord.name}</button>
          {/if}
          <!-- The folder's contents hang off a guide under its icon. -->
          <div id={`group-${group.id}`} hidden={!open} class="sidebar-group-projects">
            {@render treeNodes(node.children, group.id)}
            {#if node.children.length === 0 && !(editingGroupId === NEW_GROUP && newFolderParent === group.id)}
              <p class="sidebar-folder-empty px-2 py-1 text-caption text-[var(--text-faint)]"
                data-drop-row data-drop-kind="into" data-drop-id={group.id}>Empty</p>
            {/if}
          </div>
        {/snippet}

        {#snippet treeNodes(nodes: TreeNode[], parent: number | null)}
          {@const slot = editingGroupId === NEW_GROUP && newFolderParent === parent ? newFolderSlot(nodes) : -1}
          {#each nodes as node, i (keyOf(node))}
            {#if i === slot}{@render groupNameInput()}{/if}
            {#if node.kind === "folder"}
              {@render folderRow(node)}
            {:else}
              {@render projectEntry(node.project)}
            {/if}
          {/each}
          {#if slot === nodes.length}{@render groupNameInput()}{/if}
        {/snippet}

        <!-- The guard covers folders as well as projects so an empty folder
             stays around to be filled, renamed or deleted. -->
        {#if projects.length > 0 || groups.length > 0 || editingGroupId === NEW_GROUP}
          <div data-tree-root data-dragging={dragActive || undefined}>
            {@render treeNodes(tree, null)}
          </div>
          {#if dragActive && drag?.indicator}
            {@const at = drag.indicator}
            <div class="sidebar-drop-indicator" aria-hidden="true"
              data-box={at.box || undefined} data-refused={at.refused || undefined}
              style={`top: ${at.top}px; left: ${at.left}px; width: ${at.width}px; height: ${at.height}px`}></div>
          {/if}
        {:else if signedOut}
          <p class="px-3 py-6 text-body-sm text-[var(--text-faint)]">No published projects.</p>
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
                Create a folder
              </button>
            </div>
          </div>
        {/if}
        </div>
      </nav>

      <!-- Footer: the user identity IS the Settings entry (logout now lives
           inside Settings → Security). A compact theme toggle sits beside it. -->
      <div class="sidebar-footer p-1.5">
        {#if signedOut || !user}
        <!-- LIF-583: no account to show, so the way in takes its place. -->
        <div class="sidebar-account-card flex items-center gap-0.5 p-1 rounded-lg">
          <SignInButton class="flex-1 min-w-0 justify-center" />
          {@render themeButton()}
        </div>
        {:else}
        <div class="sidebar-account-card flex items-center gap-0.5 p-1 rounded-lg">
        <a href="#/settings" use:navLink={navigate} aria-current={isActive('/settings') ? 'page' : undefined}
          class="sidebar-destination sidebar-account flex-1 min-w-0 flex items-center gap-3 pl-px pr-1.5 py-1 rounded-md text-left transition-colors"
          title="Account settings"
        >
          <div
            class="sidebar-avatar size-8 rounded-full
                   flex items-center justify-center text-caption font-semibold
                   tracking-wide select-none shrink-0"
          >
            {initials(user.display_name || user.username)}
          </div>
          <div class="flex-1 min-w-0">
            <div class="text-body-sm font-medium text-[var(--text)] truncate leading-tight">
              {user.display_name || user.username}
            </div>
            <div class="text-micro text-[var(--text-faint)] truncate leading-tight mt-1">
              @{user.username}{user.is_admin ? " · Admin" : ""}
            </div>
          </div>
        </a>
        {@render themeButton()}
        <!-- LIF-245: small, unobtrusive entry point to the Shortcut Help
             overlay — mirrors the theme toggle beside it. The "?" key does
             the same thing from anywhere; this is for anyone who doesn't
             know the key exists yet. -->
        <button
          class="size-7 shrink-0 grid place-items-center rounded-md
                 text-[var(--text-faint)] hover:text-[var(--text)] hover:bg-[var(--sidebar-hover)] transition-colors"
          onclick={() => toggleShortcutHelp()}
          title="Keyboard shortcuts  ·  ?"
          aria-label="Keyboard shortcuts"
        >
          <HelpCircle size={15} />
        </button>
        </div>
        {/if}
      </div>
      <!-- LIF-309: an 8px hit area keeps the 3px resize indicator easy to grab. -->
      <!-- svelte-ignore a11y_no_noninteractive_tabindex -->
      <div
        class="group absolute inset-y-0 -right-1 z-20 hidden w-2 cursor-col-resize touch-none md:block focus:outline-none"
        role="separator"
        aria-orientation="vertical"
        aria-label="Resize sidebar"
        aria-valuemin={sidebarMetrics.min}
        aria-valuemax={sidebarMetrics.max}
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
      {/if}

      <!-- LIF-537: the collapsed sidebar. Same chrome, same destinations, as
           icons: workspace views are links, and a project opens a menu of its
           views so any page is still one choice away. -->
      {#snippet railLink(href: string, label: string, Icon: typeof Home, active: boolean, count: number | null = null)}
        <a href={`#${href}`} use:navLink={navigate} use:tip={count ? `${label} · ${countLabel(count)}` : label}
          aria-label={count ? `${label}, ${countLabel(count)}` : label}
          aria-current={active ? "page" : undefined}
          class="sidebar-destination rail-item relative size-10 grid place-items-center rounded-md transition-colors"
        >
          <Icon size={18} />
          {#if count}<span class="rail-dot" aria-hidden="true"></span>{/if}
        </a>
      {/snippet}
      {#snippet themeButton()}
        <button
          class="size-7 shrink-0 grid place-items-center rounded-md
                 text-[var(--text-faint)] hover:text-[var(--text)] hover:bg-[var(--sidebar-hover)] transition-colors"
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
      {/snippet}
      {#snippet rail()}
        <div class="flex flex-col items-center gap-1 pt-3 pb-2 px-1.5">
          <img src="/logo.webp" alt="Lific" width="26" height="26" class="rounded-md mb-1" />
          <button
            class="rail-item size-10 grid place-items-center rounded-md text-[var(--text-faint)]
                   hover:text-[var(--text)] hover:bg-[var(--sidebar-hover)] transition-colors"
            onclick={toggleSidebar}
            use:tip={"Expand sidebar  ·  " + modKey + " \\"}
            aria-label="Expand sidebar"
          >
            <PanelLeftOpen size={18} />
          </button>
          {#if !signedOut}
          <button
            class="sidebar-launcher rail-item size-10 grid place-items-center rounded-lg transition-colors"
            onclick={() => palette?.openPalette()}
            use:tip={`Search  ·  ${modKey} K`}
            aria-label="Search or jump to"
          >
            <Search size={17} />
          </button>
          {/if}
        </div>
        <nav class="rail-nav flex-1 min-h-0 overflow-y-auto overflow-x-hidden flex flex-col items-center gap-1 px-1.5 pb-2" aria-label="Navigation">
          {#if !signedOut}
          {@render railLink("/", "Home", Home, route === "/")}
          {@render railLink("/inbox", "Inbox", Inbox, route === "/inbox", inboxCount)}
          {@render railLink("/assigned", "Assigned to me", UserCheck, route === "/assigned")}
          {@render railLink("/agents", "Agent activity", Bot, route === "/agents", activeAgents)}
          <hr class="rail-rule" />
          {/if}
          {#each railProjects as project (project.id)}
            {@const current = activeProject?.toLowerCase() === project.identifier.toLowerCase()}
            <button
              class="sidebar-destination rail-item size-10 grid place-items-center rounded-md transition-colors"
              data-rail-project={project.id}
              aria-current={current ? "true" : undefined}
              aria-haspopup="menu"
              aria-label={`${project.name} views`}
              use:tip={project.name}
              onclick={(e) => openRailProjectMenu(e, project)}
              oncontextmenu={signedOut ? undefined : (e) => openProjectMenu(e, project)}
            >
              {#if project.emoji}
                <ProjectIcon value={project.emoji} size={18} />
              {:else}
                <span class="sidebar-initials size-6 grid place-items-center rounded-md text-caption font-medium tracking-tight">
                  {project.identifier.slice(0, 2)}
                </span>
              {/if}
            </button>
          {/each}
          {#if !signedOut}
          <button
            class="rail-item size-10 grid place-items-center rounded-md text-[var(--text-faint)]
                   hover:text-[var(--text)] hover:bg-[var(--sidebar-hover)] transition-colors"
            aria-label="New project or folder" aria-haspopup="menu"
            use:tip={"New project or folder"}
            onclick={openCreateMenu}
          >
            <Plus size={17} />
          </button>
          {/if}
        </nav>
        <div class="flex flex-col items-center gap-1 px-1.5 pt-2 pb-2.5 border-t border-[var(--sidebar-guide)]">
          <button
            class="rail-item size-10 grid place-items-center rounded-md text-[var(--text-faint)]
                   hover:text-[var(--text)] hover:bg-[var(--sidebar-hover)] transition-colors"
            onclick={themeMenu}
            use:tip={`Theme: ${themePref}`}
            aria-label="Choose theme, current: {themePref}" aria-haspopup="menu"
          >
            {#if themePref === "system"}
              <Monitor size={17} />
            {:else if themeResolved === "dark"}
              <Moon size={17} />
            {:else}
              <Sun size={17} />
            {/if}
          </button>
          {#if signedOut}
          <a href={`#${loginRouteFor(route)}`} use:navLink={navigate} use:tip={"Sign in"}
            aria-label="Sign in" data-sign-in
            class="sidebar-destination rail-item size-10 grid place-items-center rounded-md transition-colors"
          >
            <LogIn size={17} />
          </a>
          {:else}
          <a href="#/settings" use:navLink={navigate} aria-current={isActive('/settings') ? 'page' : undefined}
            use:tip={user ? `${user.display_name || user.username} · Settings` : "Settings"}
            aria-label="Account settings"
            class="sidebar-destination rail-item size-10 grid place-items-center rounded-full transition-colors"
          >
            <span class="sidebar-avatar size-8 rounded-full grid place-items-center text-caption font-semibold tracking-wide">
              {initials(user?.display_name || user?.username || "")}
            </span>
          </a>
          {/if}
        </div>
      {/snippet}
    </aside>
    {#if dragActive && drag}
      {@const item = drag.item}
      {@const node = treeIndex.get(keyOf(item))?.node}
      <!-- The dragged item as one compact row: never its sub-tabs or contents. -->
      <div class="sidebar-drag-ghost fixed z-[130] pointer-events-none max-w-64 px-2 py-1.5 rounded-md text-body-sm"
        data-sidebar-drag-ghost data-refused={drag.result?.kind === "refused" || undefined}
        style={`top: ${drag.y + 12}px; left: ${drag.x + 14}px`}>
        <div class="flex items-center gap-2 min-w-0">
          {#if node?.kind === "folder"}
            <Folder size={16} class="shrink-0 text-[var(--sidebar-accent)]" />
            <span class="truncate font-medium">{node.folder.name}</span>
            <span class="sidebar-folder-count text-micro tabular-nums">{node.projectCount}</span>
          {:else if node?.kind === "project"}
            {#if node.project.emoji}<ProjectIcon value={node.project.emoji} size={16} />
            {:else}<span class="sidebar-initials size-5 grid place-items-center rounded text-micro font-medium">{node.project.identifier.slice(0, 2)}</span>{/if}
            <span class="truncate font-medium">{node.project.name}</span>
          {/if}
        </div>
        {#if drag.result?.kind === "refused"}
          <p class="mt-1 text-caption text-[var(--error)] whitespace-normal">{drag.result.reason}</p>
        {/if}
      </div>
    {/if}
    <p class="sr-only" role="status" aria-live="polite">{liveMessage}</p>
    {#if railTip && sidebarCollapsed}
      <div role="tooltip" class="rail-tip fixed z-50 -translate-y-1/2 pointer-events-none whitespace-nowrap
                                  px-2 py-1 rounded-md text-caption"
        style={`top: ${railTip.top}px; left: ${railTip.left}px`}>
        {railTip.text}
      </div>
    {/if}

    <!-- Right column: chrome topbar (continuous with sidebar) + inset panel -->
    <div class="flex-1 min-w-0 flex flex-col">
      <!-- Mobile header (below md only). The hamburger opens the nav at the
           project list; the title beside it states where you currently are
           and opens the nav already showing THAT project's destinations, so
           moving between a project's sections is one tap plus one, not a
           trip back through the root (LIF-349). -->
      <header
        class="sidebar-theme md:hidden shrink-0 flex items-center gap-1 h-12 px-1
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
                class="sidebar-initials size-6 rounded
                       grid place-items-center text-micro font-medium tracking-tight shrink-0"
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
      {#if topbarSnippet}
        <!-- The topbar deliberately uses muted text/icon colors so it
             reads as quieter than the content panel below. We avoid
             `opacity` for the dimming effect because it creates a CSS
             stacking context that traps absolutely-positioned dropdowns
             (filters, display, help popovers) BEHIND the content panel. -->
        <div class="shrink-0 flex items-stretch min-h-0 bg-[var(--chrome)]">
          <div class="flex-1 min-w-0 flex items-stretch">
            {@render topbarSnippet()}
          </div>
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
        <!-- The scroller carries the radius too: Chromium sometimes leaves a
             composited scroller (and its sticky rows) out of an ancestor's
             rounded overflow clip, painting a square corner. -->
        <main class="absolute inset-0 bg-[var(--bg)] overflow-y-auto md:rounded-tl-xl">
          {@render children()}
        </main>
        <!-- Top edge: TL → TR. -->
        <div
          class="pointer-events-none absolute top-0 left-0 right-0 h-6 z-10
                 bg-gradient-to-b from-[var(--shadow-recess)] to-transparent"
        ></div>
        <!-- Left edge: TL → BL. Only meaningful at md+ where the sidebar (or
             its LIF-537 rail) is docked to cast the shadow; on mobile there's
             nothing to its left. -->
        <div
          class="hidden md:block pointer-events-none absolute top-0 left-0 bottom-0 w-6 z-10
                 bg-gradient-to-r from-[var(--shadow-recess)] to-transparent"
        ></div>
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
    {signedOut}
    {projects}
    {tree}
    {newFolderParent}
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
    {inboxCount}
    {assignedCount}
  />

  <!-- LIF-159: cmd+k / ctrl+p jump-anywhere. Mounted here (once, above
       routes) so its session catalog cache survives navigation. -->
  <!-- `route` is passed because the palette outlives every route: it needs
       the authoritative current path to know which project's read model to
       search, and to invalidate in-flight results when that project
       changes underneath it (LIF-445). -->
  <!-- LIF-583: not for a signed-out reader; it carries create and update
       commands as well as search. -->
  {#if !signedOut}
    <CommandPalette bind:this={palette} {navigate} {route} actions={paletteActions} />
  {/if}
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
    /* In subfolders, where names have the least room, the hidden button
       gives its width back until the row is hovered or focused. */
    .sidebar-group-projects .sidebar-group-projects .sidebar-row:not(:hover):not(:focus-within) .sidebar-overflow {
      width: 0;
      overflow: hidden;
    }
  }
  @media (pointer: coarse) {
    .sidebar-overflow { min-width: 44px; min-height: 44px; }
  }
  /* The dragged item stays in place, faded, so its origin is never lost. */
  [data-drop-entry][data-dragging] > .sidebar-row,
  .sidebar-group-heading[data-dragging] { opacity: 0.4; }
  [data-tree-root][data-dragging] .sidebar-row:hover { background: transparent; }
  .sidebar-drop-indicator {
    position: absolute;
    z-index: 5;
    pointer-events: none;
    border-radius: 2px;
    background: var(--sidebar-accent);
  }
  .sidebar-drop-indicator::before {
    content: "";
    position: absolute;
    left: -3px;
    top: -2px;
    width: 6px;
    height: 6px;
    border-radius: 999px;
    background: inherit;
  }
  .sidebar-drop-indicator[data-box] {
    background: color-mix(in srgb, var(--sidebar-accent) 14%, transparent);
    box-shadow: inset 0 0 0 1.5px var(--sidebar-accent);
    border-radius: 0.375rem;
  }
  .sidebar-drop-indicator[data-box]::before { display: none; }
  .sidebar-drop-indicator[data-refused] { background: var(--error); }
  .sidebar-drop-indicator[data-box][data-refused] {
    background: color-mix(in srgb, var(--error) 12%, transparent);
    box-shadow: inset 0 0 0 1.5px var(--error);
  }
  .sidebar-drag-ghost {
    background: var(--surface);
    color: var(--text);
    border: 1px solid var(--border);
    box-shadow: 0 6px 18px rgb(0 0 0 / 0.18);
  }
  .sidebar-drag-ghost[data-refused] { border-color: var(--error); }
  :global(body.sidebar-dragging) { cursor: grabbing; user-select: none; }
  :global(body.sidebar-dragging *) { cursor: grabbing !important; }
</style>

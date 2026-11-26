<script lang="ts">
  import Login from "./routes/Login.svelte";
  import Signup from "./routes/Signup.svelte";
  import Home from "./routes/Home.svelte";
  import Settings from "./routes/Settings.svelte";
  import InstanceSettings from "./routes/InstanceSettings.svelte";
  import IssueList from "./routes/IssueList.svelte";
  import IssueDetail from "./routes/IssueDetail.svelte";
  import IssueNew from "./routes/IssueNew.svelte";
  import ProjectNew from "./routes/ProjectNew.svelte";
  import ProjectImport from "./routes/ProjectImport.svelte";
  import ProjectOverview from "./routes/ProjectOverview.svelte";
  import ProjectSettings from "./routes/ProjectSettings.svelte";
  import PageList from "./routes/PageList.svelte";
  import Files from "./routes/Files.svelte";
  import PageDetail from "./routes/PageDetail.svelte";
  import ModuleList from "./routes/ModuleList.svelte";
  import ModuleDetail from "./routes/ModuleDetail.svelte";
  import PlanList from "./routes/PlanList.svelte";
  import PlanDetail from "./routes/PlanDetail.svelte";
  import ProjectActivity from "./routes/ProjectActivity.svelte";
  import Insights from "./routes/Insights.svelte";
  import DependencyGraph from "./routes/DependencyGraph.svelte";
  import Inbox from "./routes/Inbox.svelte";
  import Assigned from "./routes/Assigned.svelte";
  import AgentActivity from "./routes/AgentActivity.svelte";
  import PublishedProjects from "./routes/PublishedProjects.svelte"; // LIF-583
  import Layout from "./lib/Layout.svelte";
  import ErrorState from "./lib/ErrorState.svelte";
  import Toaster from "./lib/toast/Toaster.svelte"; // LIF-243
  import {
    hasSession,
    getInstance,
    autoLogin,
    saveSession,
    clearSession,
    me,
    listProjects,
    onSessionChange,
    type Project,
  } from "./lib/api";
  import { REALTIME_INVALIDATE_EVENT, type RealtimeEvent } from "./lib/autoRefresh.svelte";
  import {
    handleRealtimeEvent,
    realtimeClosed,
    realtimeOpened,
  } from "./lib/sync/readModel.svelte"; // LIF-442
  import { createSyncClient } from "./lib/sync/leader"; // LIF-443
  import {
    commentTargetFromHash,
    routeForCommentHash,
    routeWithCommentTarget,
    splitResourcePath,
  } from "./lib/commentLinks";
  import { loadLayout } from "./lib/issues/persistence"; // LIF-434
  import {
    createActivityRateCounter,
    isActivityRealtimeEvent,
    parseActivityBaseline,
  } from "./lib/activityRate";
  import { motionReduced } from "./lib/theme";
  import {
    SIGNED_OUT_PAGES,
    isSignedOut,
    onSignedOutChange,
    rewritePublicRoute,
    setSignedOut,
  } from "./lib/signedOut"; // LIF-583
  import { fade } from "svelte/transition";
  import { onDestroy, onMount, setContext, untrack } from "svelte";
  import { routePageTitle, PAGE_TITLE_CONTEXT } from "./lib/pageTitle";
  import { loginRouteFor, nextRouteFrom } from "./lib/loginRedirect";

  // Path-style deep links (LIF-247): external tools (e.g. the Dashboard)
  // link to plain paths like /LIF/overview or /LIF/issues/LIF-42. The server
  // SPA-fallbacks those to index.html, but this app is hash-routed. Keep the
  // path as the initial route when it carries a comment fragment; otherwise
  // translate it into the hash route once at boot. A fragment already uses
  // the browser's hash, so replacing it would make the router see only
  // `comment-123` and render its 404 page.
  const resourcePath = splitResourcePath(window.location.pathname);
  const pathRoute =
    window.location.pathname !== "/" && !window.location.hash.startsWith("#/")
      ? (resourcePath?.route ?? window.location.pathname) +
        window.location.search
      : null;
  const appBasePath =
    resourcePath?.basePath ??
    (window.location.hash.startsWith("#/")
      ? window.location.pathname.replace(/\/$/, "")
      : "");
  if (pathRoute && !window.location.hash) {
    history.replaceState(
      null,
      "",
      appBasePath + "/#" + pathRoute,
    );
  }

  // LIF-583: the old public view's routes (`#/public/LIF/...`) now live at
  // the project's normal routes. A path-style `/public/...` link is redirected
  // by the server; a hash route never reaches it, so it is rewritten here.
  const bootRoute = pathRoute ?? (window.location.hash.slice(1) || "/");
  const legacyBootRoute = rewritePublicRoute(bootRoute);
  if (legacyBootRoute) {
    history.replaceState(null, "", appBasePath + "/#" + legacyBootRoute);
  }
  const initialRoute = legacyBootRoute ?? bootRoute;

  let route = $state(initialRoute);
  function applyRoute(next: string) {
    route = next;
  }

  // LIF-583: mirrors lib/signedOut for this component's reactive reads. Set
  // once bootstrap (and single-user auto-login) has settled, then on every
  // session change. Signed out, a published project's pages render in the
  // normal Layout, read-only; everything else goes to login.
  let signedOut = $state(isSignedOut());
  $effect(() => onSignedOutChange((next) => {
    signedOut = next;
    published = null;
    publishedError = null;
    publishedRetriedFor = null;
  }));
  $effect(() => onSessionChange(() => {
    if (!bootstrapping) setSignedOut(!hasSession());
  }));

  // The projects a signed-out reader may open: `listProjects()` answers an
  // anonymous caller with the published ones only. null until loaded.
  let published = $state<Project[] | null>(null);
  let publishedError = $state<string | null>(null);
  let publishedLoading = false;
  // A route naming a project missing from the list refetches once (it may
  // have just been published) before going to login.
  let publishedRetriedFor: string | null = null;

  async function loadPublished() {
    if (publishedLoading) return;
    publishedLoading = true;
    publishedError = null;
    const res = await listProjects();
    publishedLoading = false;
    if (!isSignedOut()) return;
    if (res.ok) published = res.data;
    // Nothing published, or nothing this caller may see: login, as before.
    else if (res.status === 401 || res.status === 404) published = [];
    // Rate limited, busy or offline: say so rather than bounce to login.
    else publishedError = res.error;
  }

  function isPublished(project: string): boolean {
    const want = project.toLowerCase();
    return !!published?.some((p) => p.identifier.toLowerCase() === want);
  }

  // LIF-434: a detail view opened as the app's entry point (an issue link
  // tapped in a chat, a new tab, a PWA launch) has no in-app history under
  // it, so the system back button leaves the app instead of going "up" to
  // the list the view belongs to, the thing a phone user actually expects.
  // Synthesize that parent entry once at boot: replace the lone entry with
  // the parent list route, then push the current URL back on top. Back then
  // lands on the list (via the hashchange listener below); a second back
  // exits as before. Guarded on "nothing to go back to", so a same-tab
  // navigation from elsewhere keeps its real history untouched.
  function parentListRoute(input: string): string | null {
    const p = parseRoute(input);
    if (p.type !== "app") return null;
    switch (p.page) {
      case "issue-detail":
        // Mirror IssueDetail's back arrow: board when that's the layout
        // the user last had this project's list in.
        return loadLayout(p.project) === "board"
          ? `/${p.project}/board`
          : `/${p.project}/issues`;
      case "page-detail":
        return `/${p.project}/pages`;
      case "module-detail":
        return `/${p.project}/modules`;
      case "plan-detail":
        return `/${p.project}/plans`;
      default:
        return null;
    }
  }
  {
    // Chrome exposes the Navigation API's canGoBack; elsewhere fall back to
    // history.length, which over-counts across prior sites but is only ever
    // wrong toward leaving history alone.
    const nav = (window as { navigation?: { canGoBack?: boolean } }).navigation;
    const canGoBack =
      typeof nav?.canGoBack === "boolean" ? nav.canGoBack : history.length > 1;
    const parentRoute = canGoBack ? null : parentListRoute(route);
    if (parentRoute) {
      const entry = window.location.href;
      history.replaceState(null, "", appBasePath + "/#" + parentRoute);
      history.pushState(null, "", entry);
    }
  }

  // LIF-215: single-user mode. On a cold load with no session, ask the
  // instance whether web auto-login is enabled; if so, silently mint an admin
  // session before the redirect logic can bounce us to /login. We start
  // "bootstrapping" only when there's no session, so the logged-in common case
  // never shows a spinner.
  let bootstrapping = $state(!hasSession());
  // LIF-443: the socket itself lives in the sync client, which shares one
  // connection across every tab of this browser (and falls back to a
  // per-tab socket where Web Locks are unavailable). Everything around it —
  // backoff, reauth, the activity counter, the resync signal — stays here.
  const realtime = createSyncClient({
    url: socketUrl,
    onEvent: handleRealtimeMessage,
    onOpen: handleRealtimeOpen,
    onClose: handleRealtimeDown,
    onLeaderDisconnect: handleLeaderDisconnect,
  });
  let realtimeReconnect: ReturnType<typeof setTimeout> | null = null;
  let realtimeDelayMs = 1000;
  let realtimeNeedsResync = false;
  let realtimeDisposed = false;
  const realtimeActivity = createActivityRateCounter();
  let realtimeActivityReady = $state(false);
  let realtimeActivityRevision = $state(0);
  let realtimeActivityRefresh: ReturnType<typeof setTimeout> | null = null;
  let realtimeActivityBaselineRefresh: ReturnType<typeof setInterval> | null = null;
  let realtimeHeartbeat: ReturnType<typeof setInterval> | null = null;
  const REALTIME_ACTIVITY_BASELINE_REFRESH_MS = 3_600_000;
  const REALTIME_HEARTBEAT_MS = 30_000;

  function refreshRealtimeActivity() {
    realtimeActivityRefresh = null;
    realtimeActivityRevision += 1;
  }

  function scheduleRealtimeActivityRefresh() {
    if (realtimeActivityRefresh) clearTimeout(realtimeActivityRefresh);
    realtimeActivityRefresh = setTimeout(refreshRealtimeActivity, 100);
  }

  function resetRealtimeActivity() {
    if (realtimeActivityRefresh) {
      clearTimeout(realtimeActivityRefresh);
      realtimeActivityRefresh = null;
    }
    if (realtimeActivityBaselineRefresh) {
      clearInterval(realtimeActivityBaselineRefresh);
      realtimeActivityBaselineRefresh = null;
    }
    realtimeActivity.reset();
    realtimeActivityReady = false;
    realtimeActivityRevision += 1;
  }

  onMount(async () => {
    // Probe the instance once; its auto-login flag decides single-user mode.
    const inst = await getInstance();

    // LIF-215 follow-up: trust a stored session only if it still validates.
    // `hasSession()` only checks localStorage — an expired token (server
    // rejects with 401) would otherwise look valid, skip auto-login, and leave
    // the user stuck with a dead session. Clear it so the flow below can self-
    // heal in single-user mode (or fall through to /login otherwise).
    if (hasSession()) {
      const probe = await me();
      if (!probe.ok && probe.status === 401) {
        clearSession();
      }
    }

    // Single-user mode: no valid session → silently mint a fresh admin session
    // (covers both a cold load with no token and a just-expired one).
    if (!hasSession() && inst.ok && inst.data.web_auto_login) {
      const res = await autoLogin();
      if (res.ok) saveSession(res.data.token);
    }

    setSignedOut(!hasSession());
    bootstrapping = false;
    if (!realtimeDisposed) syncRealtimeSocket();
  });

  onDestroy(() => {
    realtimeDisposed = true;
    closeRealtimeSocket();
  });

  function navigate(path: string) {
    window.location.hash = path;
    applyRoute(path);
    syncRealtimeSocket();
  }

  // An auth bounce replaces the entry it bounces from, so Back from the login
  // screen does not land on a page that only sends it to login again.
  function replaceRoute(path: string) {
    history.replaceState(null, "", appBasePath + "/#" + path);
    applyRoute(path);
    syncRealtimeSocket();
  }

  // The route to come back to after signing in. A path-style deep link with a
  // comment fragment keeps the fragment in the browser hash rather than in
  // `route` (see the boot code above), so fold it back in.
  function routeToResume(): string {
    const hash = window.location.hash;
    const target = hash.startsWith("#/") ? null : commentTargetFromHash(hash);
    return target ? routeWithCommentTarget(route, target) : route;
  }

  $effect(() => {
    function onHash(event: HashChangeEvent) {
      const hash = new URL(event.newURL).hash;
      const commentTarget = commentTargetFromHash(hash);
      if (commentTarget) {
        const nextRoute = routeForCommentHash(hash, route);
        if (hash.startsWith("#/")) applyRoute(nextRoute);
        history.replaceState(
          null,
          "",
          appBasePath + "/#" + routeWithCommentTarget(nextRoute, commentTarget),
        );
      } else if (hash === "" || hash.startsWith("#/")) {
        const next = hash.slice(1) || "/";
        const legacy = rewritePublicRoute(next);
        if (legacy) replaceRoute(legacy);
        else applyRoute(next);
      }
    }
    window.addEventListener("hashchange", onHash);
    return () => window.removeEventListener("hashchange", onHash);
  });

  // Redirect logic
  $effect(() => {
    // Hold off until the single-user auto-login probe resolves, so we don't
    // flash /login and then bounce into the app once the session lands.
    if (!bootstrapping) {
      if (!signedOut) {
        // LIF-237: "/" is now a real route (Home) rather than a redirect
        // target. Only /login and /signup bounce once a session exists,
        // to the page they were opened for (`?next=`) or else Home.
        if (parsed.type === "auth") {
          replaceRoute(nextRouteFrom(route));
        }
      } else if (parsed.type !== "auth") {
        redirectSignedOut();
      }
    }
  });

  // LIF-583: where a signed-out reader may be. A published project's
  // readable pages stay put; `/` opens the one published project or lists
  // several; everything else is the login screen, as it always was.
  function redirectSignedOut() {
    const page = parsed.type === "app" ? parsed.page : null;
    const project = parsed.type === "app" && "project" in parsed ? parsed.project : null;
    if (page !== "home" && (project === null || !SIGNED_OUT_PAGES.has(page ?? ""))) {
      replaceRoute(loginRouteFor(routeToResume()));
      return;
    }
    if (publishedError !== null) return;
    if (published === null) {
      untrack(() => void loadPublished());
      return;
    }
    if (page === "home") {
      if (published.length === 1) replaceRoute(`/${published[0].identifier}/overview`);
      else if (published.length === 0) toLogin("/");
      return;
    }
    if (project === null || isPublished(project)) return;
    if (publishedRetriedFor !== project.toLowerCase()) {
      publishedRetriedFor = project.toLowerCase();
      published = null;
      return;
    }
    // Unpublished, missing, or never there: the same answer for all three.
    toLogin(routeToResume());
  }

  // The next readable route asks again: the list may change while the
  // reader is on the login screen (a project published, or unpublished).
  function toLogin(resume: string) {
    published = null;
    publishedRetriedFor = null;
    replaceRoute(loginRouteFor(resume));
  }

  // True when the current route may render for this reader. Signed in,
  // always; signed out, only once the published list confirms it.
  let signedOutReady = $derived.by(() => {
    if (!signedOut) return true;
    if (parsed.type !== "app" || published === null) return false;
    if (parsed.page === "home") return published.length > 1;
    return "project" in parsed && SIGNED_OUT_PAGES.has(parsed.page) && isPublished(parsed.project);
  });

  function retryPublished() {
    publishedError = null;
    published = null;
  }

  function socketUrl(): string {
    const url = new URL("/api/events/ws", window.location.origin);
    url.protocol = window.location.protocol === "https:" ? "wss:" : "ws:";
    return url.toString();
  }

  function closeRealtimeSocket() {
    if (realtimeReconnect) {
      clearTimeout(realtimeReconnect);
      realtimeReconnect = null;
    }
    realtimeDelayMs = 1000;
    realtimeNeedsResync = false;
    if (realtimeHeartbeat) {
      clearInterval(realtimeHeartbeat);
      realtimeHeartbeat = null;
    }
    resetRealtimeActivity();
    // Leader: closes the socket and releases the lock, so a surviving tab
    // takes over; also tells followers the connection is gone. Follower:
    // just stops listening.
    realtime.disconnect();
  }

  // A session always gets the socket. LIF-583: a signed-out reader on a
  // project route gets it too; the server sends anonymous sockets the events
  // of published projects only.
  function realtimeWanted(): boolean {
    if (realtimeDisposed || bootstrapping) return false;
    if (hasSession()) return true;
    return isSignedOut() && parsed.type === "app" && "project" in parsed;
  }

  function scheduleRealtimeReconnect() {
    if (!realtimeReconnect && realtimeWanted() && !realtime.hasLiveConnection()) {
      realtimeReconnect = window.setTimeout(() => {
        realtimeReconnect = null;
        syncRealtimeSocket();
      }, realtimeDelayMs);
      realtimeDelayMs = Math.min(realtimeDelayMs * 2, 10_000);
    }
  }

  function syncRealtimeSocket() {
    if (realtimeWanted()) {
      // Idempotent: joins the existing connection, wins the election, or
      // reopens the leader's socket, whichever applies.
      realtime.connect();
    } else {
      closeRealtimeSocket();
    }
  }

  function dispatchRealtimeEvent(event: RealtimeEvent) {
    // LIF-442: the delta read models consume events directly — they need the
    // envelope's `seq`, which the DOM CustomEvent path treats as opaque. The
    // broadcast below still drives every view that has not been converted to
    // the read model (detail views, plans, activity, files).
    handleRealtimeEvent(event);
    window.dispatchEvent(
      new CustomEvent<RealtimeEvent>(REALTIME_INVALIDATE_EVENT, { detail: event }),
    );
  }

  function requestRealtimeActivityBaseline() {
    // Followers ask through the leader: the reply fans back out over the
    // channel, so every tab seeds its counter from one server round trip.
    realtime.send({ type: "activity.baseline.request" });
  }

  function startRealtimeActivityBaselineRefresh() {
    if (realtimeActivityBaselineRefresh) {
      clearInterval(realtimeActivityBaselineRefresh);
    }
    requestRealtimeActivityBaseline();
    realtimeActivityBaselineRefresh = setInterval(
      requestRealtimeActivityBaseline,
      REALTIME_ACTIVITY_BASELINE_REFRESH_MS,
    );
  }

  // Keepalive for the socket itself, so only the tab that owns one runs it.
  function startRealtimeHeartbeat() {
    if (realtimeHeartbeat) clearInterval(realtimeHeartbeat);
    realtimeHeartbeat = setInterval(() => {
      realtime.send({ type: "heartbeat" });
    }, REALTIME_HEARTBEAT_MS);
  }

  async function reconnectAfterFailedRealtimeAttempt(sessionToken: string | null) {
    const session = await me();

    // A stale probe must not clear a session established after this socket
    // attempt ended. Network failures have no HTTP status and must keep
    // reconnecting so a temporarily unavailable server can recover.
    if (realtimeDisposed || localStorage.getItem("lific_token") !== sessionToken) return;

    if (!session.ok && session.status === 401) {
      clearSession();
      navigate(loginRouteFor(routeToResume()));
      return;
    }

    scheduleRealtimeReconnect();
  }

  // LIF-443: the shared connection came up — in this tab because it won the
  // election, or in the leader tab, which said so over the channel. Either
  // way this tab now has a live path to the server and treats it as a fresh
  // connection: resync what it missed, reseed the activity counter, and
  // resume the active project. A promotion lands here too, which is what
  // makes every tab (not just the new leader) catch up after a leader dies.
  function handleRealtimeOpen() {
    if (realtimeDisposed) return;
    realtimeDelayMs = 1000;
    if (realtimeNeedsResync) {
      realtimeNeedsResync = false;
      resetRealtimeActivity();
      dispatchRealtimeEvent({ type: "resync.required" });
    }
    startRealtimeActivityBaselineRefresh();
    if (realtime.ownsSocket()) startRealtimeHeartbeat();
    // LIF-442: hand the read models a way to send frames, so the active
    // project can ask the server to replay what it missed while the socket
    // was down (or be told to backfill over HTTP instead). In a follower
    // the frame travels to the leader and goes out on the shared socket;
    // the replay it triggers fans back out to every tab, where duplicates
    // are dropped by cursor discipline.
    realtimeOpened((frame) => {
      realtime.send(frame);
    });
  }

  // The shared connection went away, whether this tab owned it or not.
  function handleRealtimeDown() {
    realtimeClosed(); // LIF-442
    if (realtimeHeartbeat) {
      clearInterval(realtimeHeartbeat);
      realtimeHeartbeat = null;
    }
    resetRealtimeActivity();
    realtimeNeedsResync = true;
  }

  // Leader-only: our socket is gone and nothing reopens it on its own.
  // Backoff and the "is the session still valid" probe live here, not in
  // the sync client. Followers never reach this: they wait for whichever
  // tab holds the lock to reconnect, or for the lock to come to them.
  function handleLeaderDisconnect(opened: boolean) {
    if (realtimeDisposed) return;
    // LIF-583: signed out there is no session to probe or expire; back off
    // and retry.
    if (opened || isSignedOut()) {
      scheduleRealtimeReconnect();
    } else {
      void reconnectAfterFailedRealtimeAttempt(localStorage.getItem("lific_token"));
    }
  }

  function handleRealtimeMessage(event: RealtimeEvent) {
    if (realtimeDisposed) return;
    if (event.type === "activity.baseline") {
      const baseline = parseActivityBaseline(event.day_count);
      if (!baseline) return;
      realtimeActivity.seed(baseline, Date.now());
      realtimeActivityReady = true;
      scheduleRealtimeActivityRefresh();
    } else if (event.type === "resync.required") {
      resetRealtimeActivity();
      dispatchRealtimeEvent(event);
      startRealtimeActivityBaselineRefresh();
    } else {
      if (realtimeActivityReady && isActivityRealtimeEvent(event.type)) {
        realtimeActivity.record(Date.now());
        scheduleRealtimeActivityRefresh();
      }
      dispatchRealtimeEvent(event);
    }
  }

  type ParsedRoute =
    | { type: "auth"; page: "login" | "signup" }
    | { type: "app"; page: "home" }
    | { type: "app"; page: "inbox" }
    | { type: "app"; page: "assigned" }
    | { type: "app"; page: "agents" }
    | { type: "app"; page: "settings" }
    | { type: "app"; page: "instance-settings" }
    | { type: "app"; page: "project-new" }
    | { type: "app"; page: "project-import" }
    | { type: "app"; page: "project-overview"; project: string }
    | { type: "app"; page: "project-settings"; project: string; section: string | null }
    | { type: "app"; page: "issues"; project: string }
    | { type: "app"; page: "board"; project: string }
    | { type: "app"; page: "graph"; project: string }
    | {
        type: "app";
        page: "issue-new";
        project: string;
        defaultModuleId: number | null;
        defaultStatus: string | null;
      }
    | { type: "app"; page: "issue-detail"; project: string; identifier: string }
    | { type: "app"; page: "pages"; project: string }
    | { type: "app"; page: "files"; project: string }
    | { type: "app"; page: "page-detail"; project: string; pageId: number }
    | { type: "app"; page: "modules"; project: string }
    | { type: "app"; page: "module-detail"; project: string; moduleId: number }
    | { type: "app"; page: "plans"; project: string }
    | { type: "app"; page: "plan-detail"; project: string; planId: number }
    | { type: "app"; page: "activity"; project: string }
    | { type: "app"; page: "insights"; project: string }
    | { type: "loading" }
    | { type: "not-found" };

  function parseRoute(input: string): ParsedRoute {
    // Strip a "?key=value" query string from the route before pattern-
    // matching. The path portion drives the page selection; the query
    // is parsed separately for routes that opt into it (currently
    // issue-new for `?module={id}` prefill — LIF-121).
    const [r, queryString] = input.split("?");
    const query = new URLSearchParams(queryString ?? "");

    if (r === "/login" || r === "/signup") {
      return { type: "auth", page: r.slice(1) as "login" | "signup" };
    }

    // LIF-237: bare root — the "My Work" home dashboard.
    if (r === "/") {
      return { type: "app", page: "home" };
    }
    // Workspace views: lowercase and single-segment, so they can never
    // collide with a project route, which always has a second segment.
    if (r === "/inbox") {
      return { type: "app", page: "inbox" };
    }
    if (r === "/assigned") {
      return { type: "app", page: "assigned" };
    }
    if (r === "/agents") {
      return { type: "app", page: "agents" };
    }
    if (r === "/settings") {
      return { type: "app", page: "settings" };
    }
    if (r === "/settings/instance") {
      return { type: "app", page: "instance-settings" };
    }
    if (r === "/projects/new") {
      return { type: "app", page: "project-new" };
    }
    if (r === "/projects/import") {
      return { type: "app", page: "project-import" };
    }

    // Project-scoped: /{IDENTIFIER}/overview (the project dashboard) and
    // /{IDENTIFIER}/settings, optionally ?section=labels to open at one.
    const projectOverviewMatch = r.match(/^\/([A-Za-z][A-Za-z0-9_-]*)\/overview$/i);
    if (projectOverviewMatch) {
      return { type: "app", page: "project-overview", project: projectOverviewMatch[1] };
    }
    const projectSettingsMatch = r.match(/^\/([A-Za-z][A-Za-z0-9_-]*)\/settings$/i);
    if (projectSettingsMatch) {
      return {
        type: "app",
        page: "project-settings",
        project: projectSettingsMatch[1],
        section: query.get("section"),
      };
    }

    // Project-scoped: /{IDENTIFIER}/issues
    const issueListMatch = r.match(/^\/([A-Za-z][A-Za-z0-9_-]*)\/issues$/i);
    if (issueListMatch) {
      return { type: "app", page: "issues", project: issueListMatch[1] };
    }

    // Project-scoped: /{IDENTIFIER}/board
    const boardMatch = r.match(/^\/([A-Za-z][A-Za-z0-9_-]*)\/board$/i);
    if (boardMatch) {
      return { type: "app", page: "board", project: boardMatch[1] };
    }

    // Project-scoped: /{IDENTIFIER}/graph (dependency graph — LIF-363)
    const graphMatch = r.match(/^\/([A-Za-z][A-Za-z0-9_-]*)\/graph$/i);
    if (graphMatch) {
      return { type: "app", page: "graph", project: graphMatch[1] };
    }

    // Project-scoped: /{IDENTIFIER}/issues/new
    // Optional prefills: ?module={id} (LIF-121) and ?status={status}
    // (board column "+" creates an issue in that column's status).
    const issueNewMatch = r.match(/^\/([A-Za-z][A-Za-z0-9_-]*)\/issues\/new$/i);
    if (issueNewMatch) {
      const moduleParam = query.get("module");
      const defaultModuleId = moduleParam && /^\d+$/.test(moduleParam)
        ? parseInt(moduleParam)
        : null;
      const statusParam = query.get("status");
      const defaultStatus =
        statusParam &&
        ["backlog", "todo", "active", "done", "cancelled"].includes(statusParam)
          ? statusParam
          : null;
      return {
        type: "app",
        page: "issue-new",
        project: issueNewMatch[1],
        defaultModuleId,
        defaultStatus,
      };
    }

    // Project-scoped: /{IDENTIFIER}/issues/{ISSUE-ID}
    const issueDetailMatch = r.match(
      /^\/([A-Za-z][A-Za-z0-9_-]*)\/issues\/([A-Za-z][A-Za-z0-9_-]*-\d+)$/i
    );
    if (issueDetailMatch) {
      return {
        type: "app",
        page: "issue-detail",
        project: issueDetailMatch[1],
        identifier: issueDetailMatch[2],
      };
    }

    // Project-scoped: /{IDENTIFIER}/pages
    const pageListMatch = r.match(/^\/([A-Za-z][A-Za-z0-9_-]*)\/pages$/i);
    if (pageListMatch) {
      return { type: "app", page: "pages", project: pageListMatch[1] };
    }

    // Project-scoped: /{IDENTIFIER}/files (the files manager, LIF-418)
    const filesMatch = r.match(/^\/([A-Za-z][A-Za-z0-9_-]*)\/files$/i);
    if (filesMatch) {
      return { type: "app", page: "files", project: filesMatch[1] };
    }

    // Project-scoped: /{IDENTIFIER}/pages/{ID}
    const pageDetailMatch = r.match(/^\/([A-Za-z][A-Za-z0-9_-]*)\/pages\/(\d+)$/i);
    if (pageDetailMatch) {
      return {
        type: "app",
        page: "page-detail",
        project: pageDetailMatch[1],
        pageId: parseInt(pageDetailMatch[2]),
      };
    }

    // Project-scoped: /{IDENTIFIER}/plans
    const planListMatch = r.match(/^\/([A-Za-z][A-Za-z0-9_-]*)\/plans$/i);
    if (planListMatch) {
      return { type: "app", page: "plans", project: planListMatch[1] };
    }

    // Project-scoped: /{IDENTIFIER}/plans/{ID}
    const planDetailMatch = r.match(/^\/([A-Za-z][A-Za-z0-9_-]*)\/plans\/(\d+)$/i);
    if (planDetailMatch) {
      return {
        type: "app",
        page: "plan-detail",
        project: planDetailMatch[1],
        planId: parseInt(planDetailMatch[2]),
      };
    }

    // Project-scoped: /{IDENTIFIER}/activity (audit log feed — LIF-158)
    const activityMatch = r.match(/^\/([A-Za-z][A-Za-z0-9_-]*)\/activity$/i);
    if (activityMatch) {
      return { type: "app", page: "activity", project: activityMatch[1] };
    }

    // Project-scoped: /{IDENTIFIER}/insights (analytics tab — LIF-240)
    const insightsMatch = r.match(/^\/([A-Za-z][A-Za-z0-9_-]*)\/insights$/i);
    if (insightsMatch) {
      return { type: "app", page: "insights", project: insightsMatch[1] };
    }

    // Project-scoped: /{IDENTIFIER}/modules
    const moduleListMatch = r.match(/^\/([A-Za-z][A-Za-z0-9_-]*)\/modules$/i);
    if (moduleListMatch) {
      return { type: "app", page: "modules", project: moduleListMatch[1] };
    }

    // Project-scoped: /{IDENTIFIER}/modules/{ID}
    const moduleDetailMatch = r.match(/^\/([A-Za-z][A-Za-z0-9_-]*)\/modules\/(\d+)$/i);
    if (moduleDetailMatch) {
      return {
        type: "app",
        page: "module-detail",
        project: moduleDetailMatch[1],
        moduleId: parseInt(moduleDetailMatch[2]),
      };
    }

    return { type: "not-found" };
  }

  let parsed = $derived(parseRoute(route));
  let onProjectChange = $state<(() => void) | undefined>();

  // ── Document tab title ──────────────────────────────────
  // One writer: this effect. Routes may register a richer title (an issue's
  // real title once its data lands) through the `lific:title` context — the
  // same set-on-load lifecycle as `lific:topbar` — and this effect applies
  // whatever is registered, falling back to the route-derived default
  // (routePageTitle, in lib/pageTitle). The override is reset on every
  // route change here, not by the old route's unmount, so a route never
  // has to clear and cannot leak its title into the next page.
  let titleOverride = $state<string | undefined>(undefined);
  setContext<{ set: (title?: string) => void }>(PAGE_TITLE_CONTEXT, {
    set: (title) => (titleOverride = title),
  });
  let titledRoute = "";
  $effect(() => {
    if (route !== titledRoute) {
      titledRoute = route;
      titleOverride = undefined;
    }
    document.title = titleOverride ?? routePageTitle(parsed);
  });

  // LIF-246: route-level fade-in. Keyed on the page *kind*, not the raw
  // route string — "issues" and "board" collapse to the same key so
  // toggling list/board (or navigating between projects on the same page
  // type) doesn't retrigger the fade or force IssueList to remount (see
  // the comment on the IssueList branch below: it's deliberately ONE
  // branch spanning both routes for exactly this reason). The fade only
  // replays when the page kind actually changes — a real navigation, not
  // a prop update on the already-mounted component.
  let routeTransitionKey = $derived(
    parsed.type === "app"
      ? parsed.page === "board"
        ? "issues"
        : parsed.page
      : parsed.type,
  );
  function routeFadeParams() {
    return motionReduced() ? { duration: 0 } : { duration: 120 };
  }
</script>

{#if bootstrapping}
  <div class="min-h-dvh flex items-center justify-center">
    <div
      class="size-6 rounded-full border-2 border-[var(--border)]
             border-t-[var(--accent)] animate-spin"
    ></div>
  </div>
{:else if parsed.type === "auth"}
  {#if parsed.page === "signup"}
    <!-- The auth screens are a stop on the way somewhere, so they replace
         their own history entry instead of stacking more. -->
    <Signup navigate={replaceRoute} next={nextRouteFrom(route)} />
  {:else}
    <Login navigate={replaceRoute} next={nextRouteFrom(route)} />
  {/if}
{:else if signedOut && publishedError !== null}
  <!-- LIF-583: the published list could not be read (rate limited, busy,
       offline). Not a reason to send a reader to login. -->
  <div class="min-h-dvh flex items-center justify-center">
    <ErrorState title="Couldn't load this page" message={publishedError}>
      <button
        class="text-body-sm font-medium text-[var(--btn-success-text)] bg-[var(--btn-success)]
               px-3 py-1.5 rounded-md hover:bg-[var(--btn-success-hover)] transition-colors"
        onclick={retryPublished}
      >
        Try again
      </button>
    </ErrorState>
  </div>
{:else if parsed.type === "loading" || !signedOutReady}
  <div class="min-h-dvh flex items-center justify-center">
    <div
      class="size-6 rounded-full border-2 border-[var(--border)]
             border-t-[var(--accent)] animate-spin"
    ></div>
  </div>
{:else if parsed.type === "not-found"}
  <Layout {navigate} {route} bind:onProjectChange>
    <ErrorState
      title="Page not found"
      message="We couldn't find that page. The link may be wrong, or it has moved."
    >
      <button
        class="text-body-sm font-medium text-[var(--btn-success-text)] bg-[var(--btn-success)]
               px-3 py-1.5 rounded-md hover:bg-[var(--btn-success-hover)] transition-colors"
        onclick={() => navigate("/")}
      >
        Back to home
      </button>
    </ErrorState>
  </Layout>
{:else}
  <!-- Keyed so signing in or out rebuilds the shell and the route for the
       new reader rather than patching the old one. -->
  {#key signedOut}
  <Layout {navigate} {route} bind:onProjectChange>
    <svelte:boundary
      onerror={(error) => {
        // The `failed` snippet below deliberately shows the reader nothing but
        // a generic message. That is right for the UI and wrong for everything
        // else: with no handler here the exception is swallowed whole, it never
        // reaches `window.onerror`, and a route that dies on every load leaves
        // no evidence anywhere. Log it to the console, which the reader does
        // not see but anyone diagnosing a broken route does.
        console.error("[lific] route render failed:", error);
      }}
    >
    {#key routeTransitionKey}
    <div class="h-full" in:fade={routeFadeParams()}>
    {#if parsed.page === "home" && signedOut}
      <PublishedProjects {navigate} projects={published ?? []} />
    {:else if parsed.page === "home"}
      <Home
        {navigate}
        realtimeActivityCounts={realtimeActivity.counts}
        {realtimeActivityReady}
        {realtimeActivityRevision}
      />
    {:else if parsed.page === "inbox"}
      <Inbox {navigate} />
    {:else if parsed.page === "assigned"}
      <Assigned {navigate} />
    {:else if parsed.page === "agents"}
      <AgentActivity {navigate} />
    {:else if parsed.page === "settings"}
      <Settings {navigate} />
    {:else if parsed.page === "instance-settings"}
      <InstanceSettings {navigate} />
    {:else if parsed.page === "project-new"}
      <ProjectNew {navigate} />
    {:else if parsed.page === "project-import"}
      <ProjectImport {navigate} />
    {:else if parsed.page === "project-overview"}
      <ProjectOverview {navigate} projectIdentifier={parsed.project} {onProjectChange} />
    {:else if parsed.page === "project-settings"}
      <ProjectSettings {navigate} projectIdentifier={parsed.project} section={parsed.section} {onProjectChange} />
    {:else if parsed.page === "issues" || parsed.page === "board"}
      <!-- Single IssueList instance shared across the list/board routes.
           Rendering them as one branch keeps Svelte from unmounting +
           remounting the component when toggling views — a remount would
           reset state to loading and refetch issues, making the topbar
           jump. Only the `layout` prop changes; data stays put. -->
      <IssueList
        {navigate}
        projectIdentifier={parsed.project}
        layout={parsed.page === "board" ? "board" : "list"}
      />
    {:else if parsed.page === "graph"}
      <DependencyGraph {navigate} projectIdentifier={parsed.project} />
    {:else if parsed.page === "issue-new"}
      <IssueNew
        {navigate}
        projectIdentifier={parsed.project}
        defaultModuleId={parsed.defaultModuleId}
        defaultStatus={parsed.defaultStatus}
      />
    {:else if parsed.page === "issue-detail"}
      <IssueDetail
        {navigate}
        projectIdentifier={parsed.project}
        issueIdentifier={parsed.identifier}
      />
    {:else if parsed.page === "pages"}
      <PageList {navigate} projectIdentifier={parsed.project} />
    {:else if parsed.page === "files"}
      <Files {navigate} projectIdentifier={parsed.project} />
    {:else if parsed.page === "page-detail"}
      <PageDetail {navigate} projectIdentifier={parsed.project} pageId={parsed.pageId} />
    {:else if parsed.page === "modules"}
      <ModuleList {navigate} projectIdentifier={parsed.project} />
    {:else if parsed.page === "module-detail"}
      <ModuleDetail
        {navigate}
        projectIdentifier={parsed.project}
        moduleId={parsed.moduleId}
      />
    {:else if parsed.page === "plans"}
      <PlanList {navigate} projectIdentifier={parsed.project} />
    {:else if parsed.page === "plan-detail"}
      <PlanDetail {navigate} projectIdentifier={parsed.project} planId={parsed.planId} />
    {:else if parsed.page === "activity"}
      <ProjectActivity {navigate} projectIdentifier={parsed.project} />
    {:else if parsed.page === "insights"}
      <Insights {navigate} projectIdentifier={parsed.project} />
    {/if}
    </div>
    {/key}

      <!-- LIF-193: catch any unexpected render error from a route. Shows a
           GENERIC message only — never the raw error/stack — so an exception
           can't leak internal state to the user. -->
      {#snippet failed(_error: unknown, reset: () => void)}
        <ErrorState
          title="Something went wrong"
          message="An unexpected error interrupted this page. Trying again usually clears it."
        >
          <button
            class="text-body-sm font-medium text-[var(--btn-success-text)] bg-[var(--btn-success)]
                   px-3 py-1.5 rounded-md hover:bg-[var(--btn-success-hover)] transition-colors"
            onclick={reset}
          >
            Try again
          </button>
          <button
            class="text-body-sm text-[var(--text-muted)] border border-[var(--border)]
                   px-3 py-1.5 rounded-md hover:bg-[var(--bg-subtle)] transition-colors"
            onclick={() => location.reload()}
          >
            Reload
          </button>
        </ErrorState>
      {/snippet}
    </svelte:boundary>
  </Layout>
  {/key}
{/if}

<!-- LIF-243: single global toast host, outside the route branches so it
     survives navigation and renders on auth screens too. -->
<Toaster />

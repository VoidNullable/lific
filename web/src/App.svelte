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
  import ProjectSettings from "./routes/ProjectSettings.svelte";
  import PageList from "./routes/PageList.svelte";
  import PageDetail from "./routes/PageDetail.svelte";
  import ModuleList from "./routes/ModuleList.svelte";
  import ModuleDetail from "./routes/ModuleDetail.svelte";
  import PlanList from "./routes/PlanList.svelte";
  import PlanDetail from "./routes/PlanDetail.svelte";
  import ProjectActivity from "./routes/ProjectActivity.svelte";
  import Insights from "./routes/Insights.svelte";
  import Layout from "./lib/Layout.svelte";
  import ErrorState from "./lib/ErrorState.svelte";
  import Toaster from "./lib/toast/Toaster.svelte"; // LIF-243
  import { hasSession, getInstance, autoLogin, saveSession } from "./lib/api";
  import { REALTIME_INVALIDATE_EVENT, type RealtimeEvent } from "./lib/autoRefresh.svelte";
  import { motionReduced } from "./lib/theme";
  import { fade } from "svelte/transition";
  import { onMount } from "svelte";

  // Path-style deep links (LIF-247): external tools (e.g. the Dashboard)
  // link to plain paths like /LIF/overview or /LIF/issues/LIF-42. The server
  // SPA-fallbacks those to index.html, but this app is hash-routed — so a
  // path-only URL would silently land on Home. Translate the path into the
  // hash route once at boot and clean the address bar. Runs before the
  // initial `route` read below so the very first render targets the right
  // page. Unknown paths fall through to the SPA's own 404, which is correct.
  if (window.location.pathname !== "/" && !window.location.hash) {
    history.replaceState(
      null,
      "",
      "/#" + window.location.pathname + window.location.search,
    );
  }

  let route = $state(window.location.hash.slice(1) || "/");

  // LIF-215: single-user mode. On a cold load with no session, ask the
  // instance whether web auto-login is enabled; if so, silently mint an admin
  // session before the redirect logic can bounce us to /login. We start
  // "bootstrapping" only when there's no session, so the logged-in common case
  // never shows a spinner.
  let bootstrapping = $state(!hasSession());
  let realtimeSocket: WebSocket | null = null;
  let realtimeReconnect: ReturnType<typeof setTimeout> | null = null;
  let realtimeDelayMs = 1000;

  onMount(async () => {
    if (!hasSession()) {
      const inst = await getInstance();
      if (inst.ok && inst.data.web_auto_login) {
        const res = await autoLogin();
        if (res.ok) saveSession(res.data.token);
      }
    }
    bootstrapping = false;
    syncRealtimeSocket();
  });

  function navigate(path: string) {
    window.location.hash = path;
    route = path;
    syncRealtimeSocket();
  }

  $effect(() => {
    function onHash() {
      route = window.location.hash.slice(1) || "/";
    }
    window.addEventListener("hashchange", onHash);
    return () => window.removeEventListener("hashchange", onHash);
  });

  // Redirect logic
  $effect(() => {
    // Hold off until the single-user auto-login probe resolves, so we don't
    // flash /login and then bounce into the app once the session lands.
    if (!bootstrapping) {
      if (hasSession()) {
        // LIF-237: "/" is now a real route (Home) rather than a redirect
        // target — only /login and /signup bounce once a session exists.
        if (route === "/login" || route === "/signup") {
          redirectToDefault();
        }
      } else if (route !== "/login" && route !== "/signup") {
        navigate("/login");
      }
    }
  });

  // LIF-237: the bare root URL is Home, the "My Work" landing dashboard.
  // (Project-scoped surfaces are still reached by navigating into a
  // project.)
  function redirectToDefault() {
    navigate("/");
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
    if (realtimeSocket) {
      realtimeSocket.close(1000, "teardown");
      realtimeSocket = null;
    }
  }

  function scheduleRealtimeReconnect() {
    if (!realtimeReconnect && hasSession()) {
      realtimeReconnect = window.setTimeout(() => {
        realtimeReconnect = null;
        syncRealtimeSocket();
      }, realtimeDelayMs);
      realtimeDelayMs = Math.min(realtimeDelayMs * 2, 10_000);
    }
  }

  function hasLiveRealtimeSocket() {
    return (
      realtimeSocket?.readyState === WebSocket.OPEN ||
      realtimeSocket?.readyState === WebSocket.CONNECTING
    );
  }

  function syncRealtimeSocket() {
    const shouldConnect = hasSession() && !bootstrapping;
    const shouldOpen = shouldConnect && !hasLiveRealtimeSocket();

    if (!shouldConnect) {
      closeRealtimeSocket();
    }

    if (shouldOpen) {
      openRealtimeSocket();
    }
  }

  function openRealtimeSocket() {
    const socket = new WebSocket(socketUrl());
    realtimeSocket = socket;
    socket.addEventListener("open", () => {
      realtimeDelayMs = 1000;
    });
    socket.addEventListener("message", (message) => {
      if (typeof message.data !== "string") return;
      try {
        const event = JSON.parse(message.data) as RealtimeEvent;
        if (typeof event?.type === "string") {
          window.dispatchEvent(
            new CustomEvent<RealtimeEvent>(REALTIME_INVALIDATE_EVENT, { detail: event }),
          );
        }
      } catch {
        // HTTP refresh remains source of truth.
      }
    });
    socket.addEventListener("close", () => {
      if (realtimeSocket === socket) {
        realtimeSocket = null;
        scheduleRealtimeReconnect();
      }
    });
    socket.addEventListener("error", () => {
      socket.close();
    });
  }

  type ParsedRoute =
    | { type: "auth"; page: "login" | "signup" }
    | { type: "app"; page: "home" }
    | { type: "app"; page: "settings" }
    | { type: "app"; page: "instance-settings" }
    | { type: "app"; page: "project-new" }
    | { type: "app"; page: "project-settings"; project: string }
    | { type: "app"; page: "issues"; project: string }
    | { type: "app"; page: "board"; project: string }
    | {
        type: "app";
        page: "issue-new";
        project: string;
        defaultModuleId: number | null;
        defaultStatus: string | null;
      }
    | { type: "app"; page: "issue-detail"; project: string; identifier: string }
    | { type: "app"; page: "pages"; project: string }
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
    if (r === "/settings") {
      return { type: "app", page: "settings" };
    }
    if (r === "/settings/instance") {
      return { type: "app", page: "instance-settings" };
    }
    if (r === "/projects/new") {
      return { type: "app", page: "project-new" };
    }

    // Project-scoped: /{IDENTIFIER}/overview (the project dashboard).
    // `/settings` is kept as a back-compat alias for old links/bookmarks.
    const projectOverviewMatch = r.match(/^\/([A-Za-z][A-Za-z0-9_-]*)\/(overview|settings)$/i);
    if (projectOverviewMatch) {
      return { type: "app", page: "project-settings", project: projectOverviewMatch[1] };
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

  // LIF-246: route-level fade-in. Keyed on the page *kind*, not the raw
  // route string — "issues" and "board" collapse to the same key so
  // toggling list/board (or navigating between projects on the same page
  // type) doesn't retrigger the fade or force IssueList to remount (see
  // the comment on the IssueList branch below: it's deliberately ONE
  // branch spanning both routes for exactly this reason). The fade only
  // replays when the page kind actually changes — a real navigation, not
  // a prop update on the already-mounted component.
  let routeTransitionKey = $derived(
    parsed.type === "app" ? (parsed.page === "board" ? "issues" : parsed.page) : parsed.type,
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
    <Signup {navigate} />
  {:else}
    <Login {navigate} />
  {/if}
{:else if parsed.type === "loading"}
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
  <Layout {navigate} {route} bind:onProjectChange>
    <svelte:boundary>
    {#key routeTransitionKey}
    <div class="h-full" in:fade={routeFadeParams()}>
    {#if parsed.page === "home"}
      <Home {navigate} />
    {:else if parsed.page === "settings"}
      <Settings {navigate} />
    {:else if parsed.page === "instance-settings"}
      <InstanceSettings {navigate} />
    {:else if parsed.page === "project-new"}
      <ProjectNew {navigate} />
    {:else if parsed.page === "project-settings"}
      <ProjectSettings {navigate} projectIdentifier={parsed.project} {onProjectChange} />
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
{/if}

<!-- LIF-243: single global toast host, outside the route branches so it
     survives navigation and renders on auth screens too. -->
<Toaster />

<script lang="ts">
  import Login from "./routes/Login.svelte";
  import Signup from "./routes/Signup.svelte";
  import Settings from "./routes/Settings.svelte";
  import IssueList from "./routes/IssueList.svelte";
  import IssueDetail from "./routes/IssueDetail.svelte";
  import IssueNew from "./routes/IssueNew.svelte";
  import ProjectNew from "./routes/ProjectNew.svelte";
  import ProjectSettings from "./routes/ProjectSettings.svelte";
  import PageList from "./routes/PageList.svelte";
  import PageDetail from "./routes/PageDetail.svelte";
  import ModuleList from "./routes/ModuleList.svelte";
  import ModuleDetail from "./routes/ModuleDetail.svelte";
  import Layout from "./lib/Layout.svelte";
  import { hasSession } from "./lib/api";

  let route = $state(window.location.hash.slice(1) || "/");

  function navigate(path: string) {
    window.location.hash = path;
    route = path;
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
    if (hasSession()) {
      if (route === "/" || route === "/login" || route === "/signup" || route === "/home") {
        redirectToDefault();
      }
    } else {
      if (route !== "/login" && route !== "/signup") {
        navigate("/login");
      }
    }
  });

  // The bare root URL lands on Settings. (Issue lists are still reached
  // by navigating into a project.)
  function redirectToDefault() {
    navigate("/settings");
  }

  type ParsedRoute =
    | { type: "auth"; page: "login" | "signup" }
    | { type: "app"; page: "settings" }
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
    | { type: "loading" };

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
    if (r === "/settings") {
      return { type: "app", page: "settings" };
    }
    if (r === "/projects/new") {
      return { type: "app", page: "project-new" };
    }

    // Project-scoped: /{IDENTIFIER}/settings
    const projectSettingsMatch = r.match(/^\/([A-Za-z][A-Za-z0-9_-]*)\/settings$/i);
    if (projectSettingsMatch) {
      return { type: "app", page: "project-settings", project: projectSettingsMatch[1] };
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

    return { type: "loading" };
  }

  let parsed = $derived(parseRoute(route));
  let onProjectChange = $state<(() => void) | undefined>();
</script>

{#if parsed.type === "auth"}
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
{:else}
  <Layout {navigate} {route} bind:onProjectChange>
    {#if parsed.page === "settings"}
      <Settings {navigate} />
    {:else if parsed.page === "project-new"}
      <ProjectNew {navigate} />
    {:else if parsed.page === "project-settings"}
      <ProjectSettings {navigate} projectIdentifier={parsed.project} {onProjectChange} />
    {:else if parsed.page === "issues"}
      <IssueList {navigate} projectIdentifier={parsed.project} />
    {:else if parsed.page === "board"}
      <IssueList
        {navigate}
        projectIdentifier={parsed.project}
        layout="board"
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
    {/if}
  </Layout>
{/if}

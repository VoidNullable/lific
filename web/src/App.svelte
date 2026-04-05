<script lang="ts">
  import Login from "./routes/Login.svelte";
  import Signup from "./routes/Signup.svelte";
  import Settings from "./routes/Settings.svelte";
  import IssueList from "./routes/IssueList.svelte";
  import IssueDetail from "./routes/IssueDetail.svelte";
  import IssueNew from "./routes/IssueNew.svelte";
  import ProjectNew from "./routes/ProjectNew.svelte";
  import ProjectSettings from "./routes/ProjectSettings.svelte";
  import Layout from "./lib/Layout.svelte";
  import { hasSession, listProjects } from "./lib/api";

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

  async function redirectToDefault() {
    const res = await listProjects();
    if (res.ok && res.data.length > 0) {
      navigate(`/${res.data[0].identifier}/issues`);
      return;
    }
    if (res.ok) {
      navigate("/settings");
      return;
    }
    navigate("/settings");
  }

  type ParsedRoute =
    | { type: "auth"; page: "login" | "signup" }
    | { type: "app"; page: "settings" }
    | { type: "app"; page: "project-new" }
    | { type: "app"; page: "project-settings"; project: string }
    | { type: "app"; page: "issues"; project: string }
    | { type: "app"; page: "issue-new"; project: string }
    | { type: "app"; page: "issue-detail"; project: string; identifier: string }
    | { type: "loading" };

  function parseRoute(r: string): ParsedRoute {
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

    // Project-scoped: /{IDENTIFIER}/issues/new
    const issueNewMatch = r.match(/^\/([A-Za-z][A-Za-z0-9_-]*)\/issues\/new$/i);
    if (issueNewMatch) {
      return { type: "app", page: "issue-new", project: issueNewMatch[1] };
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

    return { type: "loading" };
  }

  let parsed = $derived(parseRoute(route));
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
  <Layout {navigate} {route}>
    {#if parsed.page === "settings"}
      <Settings {navigate} />
    {:else if parsed.page === "project-new"}
      <ProjectNew {navigate} />
    {:else if parsed.page === "project-settings"}
      <ProjectSettings {navigate} projectIdentifier={parsed.project} />
    {:else if parsed.page === "issues"}
      <IssueList {navigate} projectIdentifier={parsed.project} />
    {:else if parsed.page === "issue-new"}
      <IssueNew {navigate} projectIdentifier={parsed.project} />
    {:else if parsed.page === "issue-detail"}
      <IssueDetail
        {navigate}
        projectIdentifier={parsed.project}
        issueIdentifier={parsed.identifier}
      />
    {/if}
  </Layout>
{/if}

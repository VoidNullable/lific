<script lang="ts">
  import Login from "./routes/Login.svelte";
  import Signup from "./routes/Signup.svelte";
  import Settings from "./routes/Settings.svelte";
  import IssueList from "./routes/IssueList.svelte";
  import IssueDetail from "./routes/IssueDetail.svelte";
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
      // Authenticated — redirect away from auth/root pages to the app
      if (route === "/" || route === "/login" || route === "/signup" || route === "/home") {
        redirectToDefault();
      }
    } else {
      // Not authenticated — anything that isn't login/signup should go to login
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
    // Network / proxy / auth failure: don't keep the infinite loading spinner on "/".
    navigate("/settings");
  }

  // Parse route for project-scoped views
  function parseRoute(r: string): {
    type: "auth";
    page: "login" | "signup";
  } | {
    type: "app";
    page: "settings";
  } | {
    type: "app";
    page: "issues";
    project: string;
  } | {
    type: "app";
    page: "issue-detail";
    project: string;
    identifier: string;
  } | {
    type: "loading";
  } {
    if (r === "/login" || r === "/signup") {
      return { type: "auth", page: r.slice(1) as "login" | "signup" };
    }
    if (r === "/settings") {
      return { type: "app", page: "settings" };
    }

    // Project-scoped: /{IDENTIFIER}/issues (identifier is typically uppercase, e.g. LIF)
    const issueListMatch = r.match(/^\/([A-Za-z][A-Za-z0-9_-]*)\/issues$/i);
    if (issueListMatch) {
      return { type: "app", page: "issues", project: issueListMatch[1] };
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

    // Default: show loading (will redirect)
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
    {:else if parsed.page === "issues"}
      <IssueList {navigate} projectIdentifier={parsed.project} />
    {:else if parsed.page === "issue-detail"}
      <IssueDetail
        {navigate}
        projectIdentifier={parsed.project}
        issueIdentifier={parsed.identifier}
      />
    {/if}
  </Layout>
{/if}

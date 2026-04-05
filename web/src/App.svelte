<script lang="ts">
  import Login from "./routes/Login.svelte";
  import Signup from "./routes/Signup.svelte";
  import Home from "./routes/Home.svelte";
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

  // Redirect to home if already logged in and on auth pages
  $effect(() => {
    if (hasSession() && (route === "/login" || route === "/signup" || route === "/")) {
      navigate("/home");
    }
  });
</script>

{#if route === "/signup"}
  <Signup {navigate} />
{:else if route === "/home"}
  <Home {navigate} />
{:else}
  <Login {navigate} />
{/if}

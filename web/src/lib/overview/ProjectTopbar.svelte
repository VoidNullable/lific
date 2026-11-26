<script lang="ts">
  // The project overview and project settings share one topbar: where you
  // are, a switch between the two, and the page's own actions on the right.
  import { LayoutDashboard, Settings2 } from "lucide-svelte";
  import { navLink } from "../navLink";
  import { isSignedOut } from "../signedOut";

  let {
    identifier,
    current,
    navigate,
    actions,
  }: {
    identifier: string;
    current: "overview" | "settings";
    navigate: (path: string) => void;
    actions?: import("svelte").Snippet;
  } = $props();

  // LIF-583: settings need an account, so a signed-out reader gets no switch.
  const TABS = isSignedOut()
    ? []
    : ([
        ["overview", "Overview", LayoutDashboard],
        ["settings", "Settings", Settings2],
      ] as const);
</script>

<div class="flex items-center gap-2 md:gap-4 px-3 md:px-6 h-12 md:h-[3.25rem] w-full">
  <div class="hidden md:flex items-baseline gap-2 min-w-0 shrink-0">
    <a
      href="#/{identifier}/issues"
      use:navLink={navigate}
      class="font-mono text-caption text-[var(--text-faint)] hover:text-[var(--text)] transition-colors"
    >
      {identifier}
    </a>
    <span class="text-[var(--text-faint)] opacity-60" aria-hidden="true">/</span>
    <h1 class="font-display text-heading font-medium tracking-tight text-[var(--text)] leading-none">
      {current === "overview" ? "Overview" : "Settings"}
    </h1>
  </div>

  {#if TABS.length > 0}
  <nav
    aria-label="Project overview and settings"
    class="flex items-center gap-0.5 p-[3px] rounded-lg bg-[var(--bg)] shadow-[inset_0_1px_2px_rgba(0,0,0,0.10)]"
  >
    {#each TABS as [slug, label, Icon] (slug)}
      <a
        href="#/{identifier}/{slug}"
        aria-current={current === slug ? "page" : undefined}
        class="flex items-center gap-1.5 h-7 px-2 md:px-2.5 rounded-md text-body-sm font-medium
               transition-[background,color,box-shadow] duration-150
               {current === slug
          ? 'bg-[var(--surface)] text-[var(--text)] shadow-[0_1px_2px_rgba(0,0,0,0.16),0_1px_1px_rgba(0,0,0,0.10)]'
          : 'text-[var(--text-muted)] hover:text-[var(--text)]'}"
        use:navLink={navigate}
      >
        <Icon size={13} class="shrink-0" />
        {label}
      </a>
    {/each}
  </nav>
  {/if}

  <div class="flex-1"></div>

  {#if actions}
    <div class="flex items-center gap-2 shrink-0">
      {@render actions()}
    </div>
  {/if}
</div>

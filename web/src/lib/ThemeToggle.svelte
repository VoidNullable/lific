<script lang="ts">
  import {
    getPreference,
    setPreference,
    resolveTheme,
    type ThemePreference,
  } from "./theme";

  let pref = $state<ThemePreference>(getPreference());
  let resolved = $derived(resolveTheme(pref));

  function cycle() {
    const order: ThemePreference[] = ["light", "dark", "system"];
    const next = order[(order.indexOf(pref) + 1) % order.length];
    pref = next;
    setPreference(next);
  }
</script>

<button
  onclick={cycle}
  class="inline-flex items-center gap-1.5 rounded px-2 py-1 text-[0.8125rem]
         text-[var(--text-muted)] transition-colors
         hover:text-[var(--text)] hover:bg-[var(--bg-subtle)]"
  title="Theme: {pref} ({resolved})"
  aria-label="Toggle theme, current: {pref}"
>
  {#if resolved === "dark"}
    <!-- Moon -->
    <svg class="size-4" viewBox="0 0 20 20" fill="currentColor">
      <path fill-rule="evenodd" d="M7.455 2.004a.75.75 0 0 1 .26.77 7 7 0 0 0 9.958 7.967.75.75 0 0 1 1.067.853A8.5 8.5 0 1 1 6.647 1.921a.75.75 0 0 1 .808.083Z" clip-rule="evenodd" />
    </svg>
  {:else}
    <!-- Sun -->
    <svg class="size-4" viewBox="0 0 20 20" fill="currentColor">
      <path d="M10 2a.75.75 0 0 1 .75.75v1.5a.75.75 0 0 1-1.5 0v-1.5A.75.75 0 0 1 10 2ZM10 15a.75.75 0 0 1 .75.75v1.5a.75.75 0 0 1-1.5 0v-1.5A.75.75 0 0 1 10 15ZM10 7a3 3 0 1 0 0 6 3 3 0 0 0 0-6ZM15.657 5.404a.75.75 0 1 0-1.06-1.06l-1.061 1.06a.75.75 0 0 0 1.06 1.06l1.06-1.06ZM6.464 14.596a.75.75 0 1 0-1.06-1.06l-1.06 1.06a.75.75 0 0 0 1.06 1.06l1.06-1.06ZM18 10a.75.75 0 0 1-.75.75h-1.5a.75.75 0 0 1 0-1.5h1.5A.75.75 0 0 1 18 10ZM5 10a.75.75 0 0 1-.75.75h-1.5a.75.75 0 0 1 0-1.5h1.5A.75.75 0 0 1 5 10ZM14.596 15.657a.75.75 0 0 0 1.06-1.06l-1.06-1.061a.75.75 0 1 0-1.06 1.06l1.06 1.06ZM5.404 6.464a.75.75 0 0 0 1.06-1.06l-1.06-1.06a.75.75 0 1 0-1.06 1.06l1.06 1.06Z" />
    </svg>
  {/if}
  <span class="capitalize">{pref}</span>
</button>

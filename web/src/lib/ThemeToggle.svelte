<script lang="ts">
  import {
    getPreference,
    setPreference,
    resolveTheme,
    type ThemePreference,
  } from "./theme";
  import { Sun, Moon, Monitor } from "lucide-svelte";

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
  {#if pref === "system"}
    <Monitor size={16} />
  {:else if resolved === "dark"}
    <Moon size={16} />
  {:else}
    <Sun size={16} />
  {/if}
  <span class="capitalize">{pref}</span>
</button>

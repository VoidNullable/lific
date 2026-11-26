<script lang="ts">
  // LIF-583: what a signed-out reader sees where an edit action would be.
  // The login route is worked out at click time so it always returns to the
  // page the reader is on, comment target included.
  import { LogIn } from "lucide-svelte";
  import { signInRoute } from "./signedOut";

  let {
    label = "Sign in",
    variant = "primary",
    class: className = "",
  }: {
    label?: string;
    variant?: "primary" | "quiet";
    class?: string;
  } = $props();

  function go(e: MouseEvent) {
    if (e.metaKey || e.ctrlKey || e.shiftKey || e.button !== 0) return;
    e.preventDefault();
    window.location.hash = signInRoute();
  }
</script>

<a
  href="#/login"
  data-sign-in
  onclick={go}
  class="inline-flex items-center gap-1.5 h-8 px-3 rounded-md text-body-sm font-medium transition-colors
         {variant === 'primary'
    ? 'text-[var(--btn-success-text)] bg-[var(--btn-success)] hover:bg-[var(--btn-success-hover)]'
    : 'text-[var(--text-muted)] border border-[var(--border)] hover:text-[var(--text)] hover:bg-[var(--bg-subtle)]'}
         {className}"
>
  <LogIn size={14} class="shrink-0" />
  {label}
</a>

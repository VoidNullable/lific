<script lang="ts">
  // Section nav for the settings area: Account (any user) vs Instance
  // (admin-only). The Instance tab only appears for admins, matching the
  // backend scope. Underlined tabs read as "sections of one area" rather
  // than a binary toggle.
  let {
    active,
    isAdmin,
    navigate,
  }: {
    active: "account" | "instance";
    isAdmin: boolean;
    navigate: (path: string) => void;
  } = $props();
</script>

<div class="flex items-center gap-6 border-b border-[var(--border)] mb-8">
  <button
    class="relative -mb-px px-0.5 pb-2.5 pt-1 text-body font-medium border-b-2 transition-colors
           {active === 'account'
      ? 'border-[var(--accent)] text-[var(--text)]'
      : 'border-transparent text-[var(--text-muted)] hover:text-[var(--text)]'}"
    onclick={() => navigate("/settings")}
  >
    Account
  </button>
  {#if isAdmin}
    <button
      class="relative -mb-px px-0.5 pb-2.5 pt-1 text-body font-medium border-b-2 transition-colors
             {active === 'instance'
        ? 'border-[var(--accent)] text-[var(--text)]'
        : 'border-transparent text-[var(--text-muted)] hover:text-[var(--text)]'}"
      onclick={() => navigate("/settings/instance")}
    >
      Instance
    </button>
  {/if}
</div>

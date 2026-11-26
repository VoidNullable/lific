<script lang="ts">
  // LIF-525: a compact "claimed by" chip for the peek panel and Home. Gone
  // the moment the claim expires, without waiting for a refetch.
  import { Lock } from "lucide-svelte";
  import type { IssueClaim } from "../api";
  import { now } from "../now.svelte";
  import { ensureServerClock, serverNow } from "../serverClock.svelte";
  import { claimHolder, claimRemaining, claimTitle, liveClaim } from "./claims";

  let { claim }: { claim: IssueClaim | undefined } = $props();

  // The server's time, not the browser's: see claims.ts.
  const at = $derived(serverNow(now()));
  const live = $derived(liveClaim(claim, at));
  $effect(() => {
    if (claim) ensureServerClock(now());
  });
</script>

{#if live}
  <span
    class="inline-flex items-center gap-1 max-w-[220px] shrink-0 text-micro font-medium px-1.5 py-0.5 rounded-full
           text-[var(--accent)] bg-[var(--accent-subtle)]"
    data-testid="claim-chip"
    title={claimTitle(live)}
    aria-label={claimTitle(live)}
  >
    <Lock size={11} class="shrink-0" />
    <span class="truncate">{claimHolder(live)}</span>
    <span class="shrink-0 opacity-75 tabular-nums">{claimRemaining(live, at ?? now())}</span>
  </span>
{/if}

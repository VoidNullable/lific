<script lang="ts">
  // LIF-525: the issue sidebar's "Claimed by" field. Shown only while a
  // worker holds the issue; claims are taken and released by agents over
  // MCP and REST, so there is no control here.
  import { Lock } from "lucide-svelte";
  import type { IssueClaim } from "../api";
  import { now } from "../now.svelte";
  import { ensureServerClock, serverNow } from "../serverClock.svelte";
  import { claimHolder, claimRemaining, claimSession, claimTitle, liveClaim } from "./claims";

  let { claim }: { claim: IssueClaim | undefined } = $props();

  // The server's time, not the browser's: see claims.ts.
  const at = $derived(serverNow(now()));
  const live = $derived(liveClaim(claim, at));
  $effect(() => {
    if (claim) ensureServerClock(now());
  });
</script>

{#if live}
  <div class="issue-meta-field" data-testid="issue-claim" title={claimTitle(live)}>
    <p class="issue-meta-field-label">Claimed by</p>
    <div class="flex flex-col gap-0.5 text-body-sm min-w-0">
      <span class="flex items-center gap-2 min-w-0">
        <Lock size={14} class="shrink-0 text-[var(--accent)]" />
        <span class="text-[var(--text)] truncate" data-testid="issue-claim-holder">{claimHolder(live)}</span>
        <span class="text-micro text-[var(--text-faint)] tabular-nums shrink-0">{claimRemaining(live, at ?? now())}</span>
      </span>
      <span class="pl-[22px] text-micro font-mono text-[var(--text-faint)] truncate">{claimSession(live)}</span>
    </div>
  </div>
{/if}

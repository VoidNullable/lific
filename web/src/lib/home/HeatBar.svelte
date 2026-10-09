<script lang="ts">
  // LIF-507: how pressing a queued issue is, 0..1. The ramp spans the whole
  // track and the unfilled part is covered, so a short bar stays green and
  // only a long one reaches the red end.
  let { value, label }: { value: number; label?: string } = $props();
  let pct = $derived(Math.round(Math.min(1, Math.max(0, value)) * 100));
</script>

<span
  class="relative block h-1.5 w-full overflow-hidden rounded-full"
  style="background: linear-gradient(90deg, var(--heat-1), var(--heat-2), var(--heat-3), var(--heat-4), var(--heat-5), var(--heat-6));"
  role="meter"
  aria-valuemin={0}
  aria-valuemax={100}
  aria-valuenow={pct}
  aria-label={label ?? "Urgency"}
>
  <span class="absolute inset-y-0 right-0 bg-[var(--bg-subtle)]" style="width: {100 - pct}%"></span>
</span>

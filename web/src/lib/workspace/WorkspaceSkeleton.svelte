<script lang="ts">
  // Loading shape for the workspace pages: header, then grouped rows.
  import Skeleton from "../Skeleton.svelte";

  let { groups = [3, 2] }: { groups?: number[] } = $props();
</script>

<div class="flex flex-col gap-2 mb-6">
  <Skeleton variant="bar" class="h-6 w-40" />
  <Skeleton variant="bar" class="h-3 w-64" />
</div>
<div class="flex flex-col gap-5">
  {#each groups as rows, g (g)}
    <div>
      <Skeleton variant="bar" class="h-3 w-32 mb-2.5 ml-1" />
      <div class="rounded-lg bg-[var(--surface)] border border-[var(--border)] overflow-hidden">
        {#each Array.from({ length: rows }, (_, i) => i) as row (row)}
          <div class="flex items-center gap-2.5 px-4 py-2.5 border-b border-[var(--border)] last:border-b-0">
            <Skeleton variant="circle" class="size-3.5 shrink-0" />
            <Skeleton variant="bar" class="h-3 w-14 shrink-0" />
            <Skeleton variant="bar" class="h-3 flex-1 max-w-[320px]" />
          </div>
        {/each}
      </div>
    </div>
  {/each}
</div>

<script lang="ts">
  import { icons as lucideIcons } from "lucide";
  import { Icon as LucideIcon, type IconNode } from "lucide-svelte";

  let {
    value,
    size = 16,
    class: className = "",
  }: {
    value: string | null | undefined;
    size?: number;
    class?: string;
  } = $props();

  let isLucide = $derived(!!value && value.startsWith("lucide:"));
  let lucideName = $derived(isLucide ? value!.slice(7) : "");
  let iconNode = $derived(
    isLucide && lucideName in lucideIcons
      ? (lucideIcons as Record<string, IconNode>)[lucideName]
      : null
  );
</script>

{#if value && isLucide && iconNode}
  <LucideIcon {iconNode} {size} class={className} />
{:else if value && !isLucide}
  <span class={className} style="font-size: {size}px; line-height: 1;">{value}</span>
{/if}

<script lang="ts">
  import { Icon as LucideIcon, Folder } from "lucide-svelte";
  import { projectIcon } from "./projectIcons";

  let {
    value,
    size = 16,
    class: className = "",
  }: {
    value: string | null | undefined;
    size?: number;
    class?: string;
  } = $props();

  let icon = $derived(projectIcon(value));
</script>

{#if icon === "lific:logo"}
  <img
    src="/logo.webp"
    alt="Lific"
    class="{className} object-contain inline-block"
    style="width: {size}px; height: {size}px;"
  />
{:else if typeof icon === "string"}
  <span class="{className} inline-flex shrink-0 items-center justify-center overflow-hidden" style="width: {size}px; height: {size}px; font-size: {size}px; line-height: 1;">{icon}</span>
{:else if icon}
  <LucideIcon iconNode={icon} {size} class={className} />
{:else if value}
  <Folder {size} class={className} />
{/if}

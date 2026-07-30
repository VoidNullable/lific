<script lang="ts">
  // One-tap copy for an identifier. The inline Copy -> Check flip is the
  // success cue — nicer than a toast for a one-tap copy — while
  // copyToClipboard still raises an error toast when the clipboard is
  // unavailable, so a blocked copy can never read as a silent no-op.
  //
  // Children are optional. Pass the identifier as children for the bordered
  // pill that reads "LIF-42 ⧉" (PeekPanel, ProjectSettings); omit them for an
  // icon-only button where something else already renders the label
  // (Breadcrumbs, where the crumb itself is the identifier).

  import type { Snippet } from "svelte";
  import { onDestroy } from "svelte";
  import { Copy, Check } from "lucide-svelte";
  import { copyToClipboard } from "./clipboard";

  let {
    value,
    label = "identifier",
    iconSize = 12,
    class: klass = "",
    iconClass = "",
    children,
  }: {
    /** The full text to copy. Call sites pass the whole identifier even when
     *  the visible label next to it is truncated. */
    value: string;
    /** Names the thing being copied in the tooltip and aria-label:
     *  "Copy identifier", "Copy LIF-42". */
    label?: string;
    iconSize?: number;
    class?: string;
    /** Extra classes for the Copy icon alone — pill call sites reveal just
     *  the icon on hover while their label stays put. Check never takes them:
     *  the confirmation has to be visible wherever the click came from. */
    iconClass?: string;
    children?: Snippet;
  } = $props();

  let copied = $state(false);
  let resetTimer: number | undefined;

  async function copy(event: MouseEvent) {
    // This button sits beside a breadcrumb link and inside clickable panel
    // rows, so the copy must never double as a navigation.
    event.preventDefault();
    event.stopPropagation();
    if (!(await copyToClipboard(value, { silentSuccess: true }))) return;
    copied = true;
    window.clearTimeout(resetTimer);
    resetTimer = window.setTimeout(() => {
      copied = false;
    }, 1500);
  }

  onDestroy(() => window.clearTimeout(resetTimer));
</script>

<button
  type="button"
  class={klass}
  onclick={copy}
  title={`Copy ${label}`}
  aria-label={`Copy ${label}`}
>
  {@render children?.()}
  {#if copied}
    <Check size={iconSize} />
  {:else}
    <Copy size={iconSize} class={iconClass} />
  {/if}
</button>

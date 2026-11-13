<script lang="ts">
  // The floating toolbar over a text selection in the body editor. It sits
  // above the selection on a desktop and below it on a touch screen, where
  // the system's own copy/paste menu takes the space above.

  import {
    Bold,
    Italic,
    Strikethrough,
    Code,
    Link as LinkIcon,
    Heading1,
    Heading2,
    List,
    ListChecks,
    TextQuote,
    MessageSquareQuote,
  } from "lucide-svelte";
  import type { FormatAction } from "./cm/commands";

  let {
    x,
    y,
    placement = "above",
    active,
    onAction,
    onQuote,
  }: {
    x: number;
    y: number;
    placement?: "above" | "below";
    active: Set<FormatAction>;
    onAction: (action: FormatAction) => void;
    onQuote?: () => void;
  } = $props();

  type Item = {
    action: FormatAction;
    // eslint-disable-next-line @typescript-eslint/no-explicit-any
    icon: any;
    label: string;
    keys?: string;
  };

  const mod = typeof navigator !== "undefined" && /Mac|iP(hone|ad)/.test(navigator.platform) ? "⌘" : "Ctrl+";

  const groups: Item[][] = [
    [
      { action: "bold", icon: Bold, label: "Bold", keys: `${mod}B` },
      { action: "italic", icon: Italic, label: "Italic", keys: `${mod}I` },
      { action: "strike", icon: Strikethrough, label: "Strikethrough", keys: `${mod}⇧X` },
      { action: "code", icon: Code, label: "Inline code", keys: `${mod}E` },
      { action: "link", icon: LinkIcon, label: "Link", keys: `${mod}⇧K` },
    ],
    [
      { action: "h1", icon: Heading1, label: "Heading 1" },
      { action: "h2", icon: Heading2, label: "Heading 2" },
      { action: "bullet", icon: List, label: "Bulleted list" },
      { action: "checklist", icon: ListChecks, label: "Checklist" },
      { action: "quote", icon: TextQuote, label: "Quote" },
    ],
  ];

  let el = $state<HTMLDivElement | null>(null);
  let width = $state(0);
  let left = $derived(
    typeof window === "undefined"
      ? x
      : Math.max(8 + width / 2, Math.min(x, window.innerWidth - 8 - width / 2)),
  );
</script>

<!-- svelte-ignore a11y_no_static_element_interactions -->
<div
  bind:this={el}
  bind:offsetWidth={width}
  class="fmt"
  class:fmt--below={placement === "below"}
  style="left: {left}px; top: {y}px;"
  role="toolbar"
  tabindex="-1"
  aria-label="Format selection"
  onmousedown={(e) => e.preventDefault()}
>
  {#each groups as group, gi (gi)}
    {#if gi > 0}<span class="fmt__sep" aria-hidden="true"></span>{/if}
    {#each group as item (item.action)}
      {@const Icon = item.icon}
      <button
        type="button"
        class="fmt__btn"
        aria-pressed={active.has(item.action)}
        title={item.keys ? `${item.label} (${item.keys})` : item.label}
        aria-label={item.label}
        onclick={() => onAction(item.action)}
      >
        <Icon size={15} />
      </button>
    {/each}
  {/each}
  {#if onQuote}
    <span class="fmt__sep" aria-hidden="true"></span>
    <button type="button" class="fmt__btn fmt__btn--text" title="Quote in a comment" onclick={onQuote}>
      <MessageSquareQuote size={15} />
      <span class="fmt__label">Quote</span>
    </button>
  {/if}
</div>

<style>
  .fmt {
    position: fixed;
    z-index: 60;
    transform: translate(-50%, calc(-100% - 8px));
    display: flex;
    align-items: center;
    gap: 0.0625rem;
    padding: 0.1875rem;
    border-radius: 0.5rem;
    background: var(--surface);
    border: 1px solid var(--border);
    box-shadow:
      0 4px 12px rgba(0, 0, 0, 0.12),
      0 1px 2px rgba(0, 0, 0, 0.08);
    animation: fmt-in 0.12s var(--ease-out-expo);
  }
  .fmt--below {
    transform: translate(-50%, 8px);
  }
  @keyframes fmt-in {
    from {
      opacity: 0;
    }
  }
  .fmt__btn {
    display: inline-flex;
    align-items: center;
    justify-content: center;
    gap: 0.3125rem;
    min-width: 1.875rem;
    height: 1.875rem;
    padding: 0 0.375rem;
    border: 0;
    border-radius: 0.375rem;
    background: transparent;
    color: var(--text-muted);
    cursor: pointer;
    transition:
      background 0.12s ease,
      color 0.12s ease;
  }
  .fmt__btn:hover {
    background: var(--bg-subtle);
    color: var(--text);
  }
  .fmt__btn[aria-pressed="true"] {
    background: var(--accent-subtle);
    color: var(--accent);
  }
  .fmt__btn--text {
    padding: 0 0.5rem;
  }
  .fmt__label {
    font-size: 0.75rem;
    font-weight: 500;
  }
  .fmt__sep {
    width: 1px;
    align-self: stretch;
    margin: 0.25rem 0.1875rem;
    background: var(--border);
  }
  @media (max-width: 480px) {
    .fmt__label {
      display: none;
    }
  }
  @media (prefers-reduced-motion: reduce) {
    .fmt {
      animation: none;
    }
  }
</style>

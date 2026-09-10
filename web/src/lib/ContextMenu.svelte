<script lang="ts">
  import { onMount, untrack } from "svelte";
  import { contextMenuState, closeContextMenu, type ContextMenuItem } from "./contextMenuState.svelte";
  import { motionReduced } from "./theme";
  import { fade } from "svelte/transition";

  let menuEl = $state<HTMLDivElement | null>(null);
  let highlighted = $state(-1);
  let menuPos = $state({ top: 0, left: 0 });

  function focusItem(index: number) {
    if (contextMenuState.items[index]?.disabled) return;
    const element = menuEl?.querySelector<HTMLElement>(`[data-menu-index="${index}"]`);
    element?.focus({ preventScroll: true });
    element?.scrollIntoView({ block: "nearest" });
    // Focusing an already active item does not emit another focus event.
    if (element && document.activeElement === element) highlighted = index;
  }

  $effect(() => {
    if (!contextMenuState.open || !menuEl) return;
    // Track replacements, including a second right-click at the same coordinates.
    contextMenuState.generation;
    const items = contextMenuState.items;
    const m = menuEl.getBoundingClientRect();
    const { x, y } = contextMenuState;
    menuPos = {
      top: Math.max(8, Math.min(y + m.height > window.innerHeight - 8 ? y - m.height : y, window.innerHeight - m.height - 8)),
      left: Math.max(8, Math.min(x + m.width > window.innerWidth - 8 ? x - m.width : x, window.innerWidth - m.width - 8)),
    };
    untrack(() => {
      highlighted = -1;
      const first = items.findIndex(item => !item.disabled);
      if (first >= 0) focusItem(first);
      else menuEl?.focus({ preventScroll: true });
    });
  });

  function activate(e: MouseEvent, item: ContextMenuItem) {
    e.stopPropagation();
    if (item.disabled) { e.preventDefault(); return; }
    if (e.button !== 0 || e.metaKey || e.ctrlKey || e.shiftKey || e.altKey) return;
    if (item.action) e.preventDefault();
    closeContextMenu();
    item.action?.();
  }

  function handleKeydown(e: KeyboardEvent) {
    if (!contextMenuState.open) return;
    if (e.key === "Escape" || e.key === "Tab") {
      e.preventDefault();
      e.stopImmediatePropagation();
      closeContextMenu();
      return;
    }
    if (!menuEl?.contains(document.activeElement)) return;
    const enabled = contextMenuState.items.flatMap((item, i) => item.disabled ? [] : [i]);
    if (["ArrowDown", "ArrowUp", "Home", "End"].includes(e.key)) {
      e.preventDefault();
      e.stopImmediatePropagation();
      if (!enabled.length) return;
      const current = enabled.indexOf(highlighted);
      const next = e.key === "Home" ? 0 : e.key === "End" ? enabled.length - 1
        : (current + (e.key === "ArrowDown" ? 1 : -1) + enabled.length) % enabled.length;
      focusItem(enabled[next]);
    } else if (e.key === " " && !e.metaKey && !e.ctrlKey && !e.shiftKey && !e.altKey && document.activeElement instanceof HTMLAnchorElement) {
      e.preventDefault();
      e.stopImmediatePropagation();
      document.activeElement.click();
    }
    // Enter and button Space use native activation on the actually focused item.
  }

  onMount(() => {
    // Capture owns the top layer before window bubble listeners can close a parent.
    window.addEventListener("keydown", handleKeydown, true);
    return () => {
      window.removeEventListener("keydown", handleKeydown, true);
      closeContextMenu();
    };
  });

  function handleWindowClick(e: MouseEvent) {
    if (!contextMenuState.open || (e.target instanceof Node && menuEl?.contains(e.target))) return;
    // A click may already have focused another input. Do not steal that focus back.
    closeContextMenu(document.activeElement === document.body || !!menuEl?.contains(document.activeElement));
  }

  function handleScrollOrResize(e: Event) {
    if (e.type === "scroll" && e.target instanceof Node && menuEl?.contains(e.target)) return;
    closeContextMenu();
  }

  const itemClass = (i: number, disabled?: boolean) =>
    `w-full flex items-center gap-2.5 px-3 py-1.5 min-h-11 sm:min-h-8 text-left text-body-sm outline-none transition-colors text-[var(--text)] ${disabled ? 'opacity-40 cursor-not-allowed' : i === highlighted ? 'bg-[var(--bg-subtle)]' : ''}`;
</script>

<svelte:window
  oncontextmenu={() => closeContextMenu()}
  onclick={handleWindowClick}
  onscrollcapture={handleScrollOrResize}
  onresize={handleScrollOrResize}
/>

{#if contextMenuState.open}
  <!-- svelte-ignore a11y_no_noninteractive_tabindex -->
  <div
    bind:this={menuEl}
    data-context-menu
    role="menu"
    tabindex="-1"
    aria-label="Context menu"
    class="fixed z-[120] min-w-[180px] max-w-[calc(100vw-16px)] max-h-[calc(100dvh-16px)] overflow-y-auto py-1.5 rounded-lg border border-[var(--border)] bg-[var(--surface)] shadow-lg outline-none"
    style="top: {menuPos.top}px; left: {menuPos.left}px;"
    transition:fade={{ duration: motionReduced() ? 0 : 100 }}
  >
    {#each contextMenuState.items as item, i}
      {#snippet content()}
        {@const Icon = item.icon}
        {#if Icon}<Icon size={14} class="shrink-0 text-[var(--text-faint)]" />{/if}
        <span class="flex-1 truncate">{item.label}</span>
      {/snippet}
      {#if item.href && !item.disabled}
        <a
          role="menuitem"
          href={item.href}
          data-menu-index={i}
          tabindex={!item.disabled && i === highlighted ? 0 : -1}
          class={itemClass(i, item.disabled)}
          onfocus={() => highlighted = i}
          onpointerenter={() => focusItem(i)}
          onclick={(e) => activate(e, item)}
        >{@render content()}</a>
      {:else}
        <button
          role="menuitem"
          type="button"
          disabled={item.disabled}
          data-menu-index={i}
          tabindex={!item.disabled && i === highlighted ? 0 : -1}
          class={itemClass(i, item.disabled)}
          onfocus={() => highlighted = i}
          onpointerenter={() => focusItem(i)}
          onclick={(e) => activate(e, item)}
        >{@render content()}</button>
      {/if}
    {/each}
  </div>
{/if}

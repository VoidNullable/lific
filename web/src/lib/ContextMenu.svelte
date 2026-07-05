<script lang="ts">
  // LIF-248 — reusable right-click context menu primitive. Mounted once in
  // Layout.svelte (alongside PeekPanel/CommandPalette/ShortcutHelp) and
  // driven by the contextMenu.svelte.ts module singleton, so any surface
  // (IssueRow, IssueCard, Markdown's identifier links) can summon it via
  // `openContextMenu(x, y, items)` without a prop chain.
  //
  // Positioning: `position: fixed` seeded at the pointer coordinates the
  // triggering `contextmenu` event carried, then refined once the menu has
  // a real size — clamped to the viewport and flipped up/left near an
  // edge. Same math family as Select.svelte's fixed-position dropdown and
  // IssueHoverCard's flip-above-when-no-room logic, just anchored to a
  // point instead of a trigger element's bounding rect.
  import { contextMenuState, closeContextMenu, type ContextMenuItem } from "./contextMenuState.svelte";
  import { motionReduced } from "./theme";
  import { fade } from "svelte/transition";

  let menuEl = $state<HTMLDivElement | null>(null);
  let highlighted = $state(0);
  let menuPos = $state({ top: 0, left: 0 });

  // Seed immediately at the cursor position — before the menu has ever
  // painted, so there's no (0,0) flash. Reset the keyboard cursor to the
  // first item every time a menu opens (including "moved" reopens from a
  // second right-click elsewhere, which replace items/x/y without ever
  // flipping `open` false→true — see the window contextmenu listener
  // below for why that's fine).
  $effect(() => {
    if (!contextMenuState.open) return;
    menuPos = { top: contextMenuState.y, left: contextMenuState.x };
    highlighted = 0;
  });

  // Refine once the menu has a real size: flip above/left of the cursor
  // when it would run off the bottom/right edge of the viewport, same
  // clamp Select.svelte applies to its own menu.
  $effect(() => {
    if (!contextMenuState.open || !menuEl) return;
    const m = menuEl.getBoundingClientRect();
    let top = contextMenuState.y;
    let left = contextMenuState.x;
    if (left + m.width > window.innerWidth - 8) {
      left = Math.max(8, contextMenuState.x - m.width);
    }
    if (top + m.height > window.innerHeight - 8) {
      top = Math.max(8, contextMenuState.y - m.height);
    }
    menuPos = { top, left };
  });

  // Focus the menu on open so screen readers land on it and arrow-key nav
  // works without requiring a prior click — mirrors CommandPalette's own
  // open-focus behavior. Deferred a frame so it runs after the flip/clamp
  // effect above has settled the menu's final position.
  $effect(() => {
    if (!contextMenuState.open) return;
    const raf = requestAnimationFrame(() => menuEl?.focus());
    return () => cancelAnimationFrame(raf);
  });

  function activate(item: ContextMenuItem) {
    if (item.disabled) return;
    closeContextMenu();
    item.action();
  }

  function handleKeydown(e: KeyboardEvent) {
    if (!contextMenuState.open) return;
    const items = contextMenuState.items;
    if (e.key === "Escape") {
      e.preventDefault();
      closeContextMenu();
      return;
    }
    if (e.key === "ArrowDown") {
      e.preventDefault();
      highlighted = (highlighted + 1) % items.length;
      return;
    }
    if (e.key === "ArrowUp") {
      e.preventDefault();
      highlighted = (highlighted - 1 + items.length) % items.length;
      return;
    }
    if (e.key === "Enter" || e.key === " ") {
      e.preventDefault();
      const item = items[highlighted];
      if (item) activate(item);
    }
  }

  function handleWindowClick(e: MouseEvent) {
    if (!contextMenuState.open) return;
    if (menuEl && e.target instanceof Node && menuEl.contains(e.target)) return;
    closeContextMenu();
  }

  // A fixed-position menu doesn't travel with whatever it was opened on —
  // close it on scroll/resize, mirroring Select.svelte, except a scroll
  // that originates inside the menu itself (it has no scrollable content
  // today, but this keeps the two components' logic identical if that
  // ever changes).
  function handleScrollOrResize(e: Event) {
    if (!contextMenuState.open) return;
    if (e.type === "scroll" && menuEl && e.target instanceof Node && menuEl.contains(e.target)) {
      return;
    }
    closeContextMenu();
  }

  // Right-clicking a SECOND target (another row, another identifier chip)
  // doesn't need an explicit close here — that target's own `contextmenu`
  // handler calls `openContextMenu()` again, which just replaces
  // position/items in place (no flicker, same "swap in place" idea as
  // peek re-targeting a different issue). This window-level listener only
  // fires for right-clicks that reach `window` — i.e. NOT on a wired chip,
  // since every wired handler calls `e.stopPropagation()` before this can
  // see the event. So: right-click a chip while the menu is open → it
  // moves. Right-click blank space while the menu is open → it closes.
  function handleWindowContextMenu() {
    closeContextMenu();
  }

  function enterParams() {
    return motionReduced() ? { duration: 0 } : { duration: 100 };
  }
</script>

<svelte:window
  oncontextmenu={handleWindowContextMenu}
  onclick={handleWindowClick}
  onscrollcapture={handleScrollOrResize}
  onresize={handleScrollOrResize}
  onkeydown={handleKeydown}
/>

{#if contextMenuState.open}
  <!-- svelte-ignore a11y_no_noninteractive_tabindex -->
  <div
    bind:this={menuEl}
    role="menu"
    tabindex="-1"
    aria-label="Context menu"
    class="fixed z-[120] min-w-[180px] py-1.5 rounded-lg border border-[var(--border)]
           bg-[var(--surface)] shadow-lg outline-none"
    style="top: {menuPos.top}px; left: {menuPos.left}px;"
    transition:fade={enterParams()}
    onclick={(e) => e.stopPropagation()}
  >
    {#each contextMenuState.items as item, i (item.label)}
      {@const Icon = item.icon}
      <button
        role="menuitem"
        type="button"
        disabled={item.disabled}
        class="w-full flex items-center gap-2.5 px-3 py-1.5 text-left text-body-sm
               transition-colors disabled:opacity-40 disabled:cursor-not-allowed
               {i === highlighted && !item.disabled
          ? 'bg-[var(--bg-subtle)] text-[var(--text)]'
          : 'text-[var(--text)] hover:bg-[var(--bg-subtle)]'}"
        onmouseenter={() => (highlighted = i)}
        onclick={() => activate(item)}
      >
        {#if Icon}
          <Icon size={14} class="shrink-0 text-[var(--text-faint)]" />
        {/if}
        <span class="flex-1 truncate">{item.label}</span>
      </button>
    {/each}
  </div>
{/if}

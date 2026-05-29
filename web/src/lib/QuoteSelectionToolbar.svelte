<script lang="ts">
  // LIF-111 — quote-in-comment selection helper.
  //
  // When the user selects text inside a rendered markdown surface
  // (`container`), a small floating toolbar appears centered above the
  // selection with a single "Quote in comment" button. Clicking it hands
  // the selected plaintext up via `onQuote`, which the comment composer
  // turns into a markdown blockquote.
  //
  // v1 uses window.getSelection().toString() — rendered plaintext, not the
  // source markdown. Source reconstruction / inline anchored comments are
  // out of scope.

  import { Quote } from "lucide-svelte";

  let {
    container,
    onQuote,
  }: {
    container: HTMLElement | null;
    onQuote: (text: string) => void;
  } = $props();

  let visible = $state(false);
  let x = $state(0);
  let y = $state(0);
  let selectedText = $state("");

  // The selection we last showed for — so a scroll handler can decide
  // whether the anchor has moved off-screen and we should hide.
  let lastRect: DOMRect | null = null;

  function insideContainer(node: Node | null): boolean {
    if (!container || !node) return false;
    const el = node.nodeType === Node.ELEMENT_NODE ? (node as Element) : node.parentElement;
    return !!el && container.contains(el);
  }

  function inEditableField(node: Node | null): boolean {
    let el: Element | null =
      node && node.nodeType === Node.ELEMENT_NODE ? (node as Element) : node?.parentElement ?? null;
    while (el) {
      const tag = el.tagName;
      if (tag === "INPUT" || tag === "TEXTAREA" || (el as HTMLElement).isContentEditable) {
        return true;
      }
      el = el.parentElement;
    }
    return false;
  }

  function evaluateSelection() {
    const sel = typeof window !== "undefined" ? window.getSelection() : null;
    if (!sel || sel.rangeCount === 0 || sel.isCollapsed) {
      hide();
      return;
    }

    const text = sel.toString();
    if (!text.trim()) {
      hide();
      return;
    }

    const range = sel.getRangeAt(0);
    const anchor = range.commonAncestorContainer;

    if (!insideContainer(anchor) || inEditableField(anchor)) {
      hide();
      return;
    }

    const rect = range.getBoundingClientRect();
    if (rect.width === 0 && rect.height === 0) {
      hide();
      return;
    }

    lastRect = rect;
    selectedText = text;
    x = rect.left + rect.width / 2;
    y = rect.top; // toolbar sits above; CSS transform lifts it clear
    visible = true;
  }

  function hide() {
    visible = false;
    selectedText = "";
    lastRect = null;
  }

  function onSelectionChange() {
    evaluateSelection();
  }

  function onScroll() {
    if (!visible || !lastRect) return;
    // Re-measure the live selection; if it has scrolled out of view or
    // away, hide. Cheapest correct approach: just re-evaluate.
    evaluateSelection();
  }

  function onDocPointerDown(e: PointerEvent) {
    // A pointerdown outside both the toolbar and the current selection
    // surface will collapse the selection anyway; selectionchange handles
    // the hide. Nothing extra needed here, but keep the hook for clarity.
    void e;
  }

  // Prevent the toolbar's own mousedown from clearing the selection.
  function onToolbarMouseDown(e: MouseEvent) {
    e.preventDefault();
  }

  function quote() {
    const text = selectedText;
    if (text) onQuote(text);
    const sel = typeof window !== "undefined" ? window.getSelection() : null;
    sel?.removeAllRanges();
    hide();
  }

  $effect(() => {
    if (typeof document === "undefined") return;
    document.addEventListener("selectionchange", onSelectionChange);
    document.addEventListener("pointerdown", onDocPointerDown, true);
    window.addEventListener("scroll", onScroll, true);
    window.addEventListener("resize", onScroll);
    return () => {
      document.removeEventListener("selectionchange", onSelectionChange);
      document.removeEventListener("pointerdown", onDocPointerDown, true);
      window.removeEventListener("scroll", onScroll, true);
      window.removeEventListener("resize", onScroll);
    };
  });
</script>

{#if visible}
  <div
    class="qsel"
    style="left: {x}px; top: {y}px;"
    onmousedown={onToolbarMouseDown}
    role="toolbar"
    tabindex="-1"
    aria-label="Selection actions"
  >
    <button type="button" class="qsel__btn" onclick={quote}>
      <Quote size={13} />
      Quote in comment
    </button>
  </div>
{/if}

<style>
  .qsel {
    position: fixed;
    z-index: 60;
    transform: translate(-50%, calc(-100% - 8px));
    display: flex;
    align-items: center;
    padding: 0.1875rem;
    border-radius: 0.5rem;
    background: var(--surface);
    border: 1px solid var(--border);
    box-shadow:
      0 4px 12px rgba(0, 0, 0, 0.12),
      0 1px 2px rgba(0, 0, 0, 0.08);
  }

  .qsel__btn {
    display: inline-flex;
    align-items: center;
    gap: 0.375rem;
    border: 0;
    border-radius: 0.375rem;
    padding: 0.3125rem 0.5625rem;
    background: transparent;
    color: var(--text);
    font-size: 0.75rem;
    font-weight: 500;
    line-height: 1;
    cursor: pointer;
    white-space: nowrap;
    transition: background 0.12s ease;
  }
  .qsel__btn:hover {
    background: var(--bg-subtle);
  }
</style>

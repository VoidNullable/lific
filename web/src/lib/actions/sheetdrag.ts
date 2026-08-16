// Swipe-down-to-dismiss for mobile bottom sheets (issue peek, page peek).
//
// The sheets render a drag-handle pill that used to be purely decorative;
// this action makes the promise real. Attach it to the sheet's HEADER
// region (pill + title row) — deliberately not the body, whose vertical
// gestures mean scrolling the content.
//
// Gesture contract (same physics vocabulary as MobileNav's swipe-back):
// - Touch/pen only, and only while the sheet is in bottom-sheet mode
//   (<768px). On desktop the same element is a side panel; a vertical drag
//   there means nothing.
// - Presses that start on a button/link/input are left alone so the
//   header's copy and close controls keep working.
// - The sheet translates 1:1 with the finger (downward only). Releasing
//   past 28% of the sheet's height, or with a downward flick faster than
//   0.45 px/ms after at least 24px of travel, commits the dismiss; anything
//   less springs back.
// - On commit the action finishes the slide itself (one smooth continuation
//   from the finger's last position), then calls `onDismiss`. The caller
//   should suppress its own exit transition for that close — otherwise
//   Svelte's out: would yank the already-offscreen sheet back up and replay
//   the slide. Both peek panels do this with a `dragDismissed` flag read by
//   their out-params function.

export interface SheetDragOptions {
  /** The element to translate — the sheet root, not the header the action
   *  is attached to. */
  sheet: () => HTMLElement | null;
  /** Called after the dismiss slide finishes. */
  onDismiss: () => void;
}

const COMMIT_RATIO = 0.28;
const COMMIT_VELOCITY = 0.45; // px/ms
const MIN_FLICK_TRAVEL = 24;
const SLIDE_MS = 170;

export function sheetDrag(node: HTMLElement, options: SheetDragOptions) {
  let opts = options;
  let pointerId: number | null = null;
  let startY = 0;
  let startTime = 0;
  let dy = 0;

  function bottomSheetMode(): boolean {
    return typeof window !== "undefined" && window.innerWidth < 768;
  }

  function clearInline(el: HTMLElement) {
    el.style.transition = "";
    el.style.transform = "";
  }

  function onMove(e: PointerEvent) {
    if (e.pointerId !== pointerId) return;
    const el = opts.sheet();
    if (!el) return;
    dy = Math.max(0, e.clientY - startY);
    el.style.transition = "none";
    el.style.transform = dy > 0 ? `translateY(${dy}px)` : "";
    // The gesture owns this pointer; don't let the browser scroll too.
    e.preventDefault();
  }

  function onEnd(e: PointerEvent) {
    if (e.pointerId !== pointerId) return;
    pointerId = null;
    track(false);
    const el = opts.sheet();
    if (!el) return;

    const height = el.getBoundingClientRect().height || 480;
    const elapsed = Math.max(1, e.timeStamp - startTime);
    const commit =
      e.type === "pointerup" &&
      (dy > height * COMMIT_RATIO ||
        (dy > MIN_FLICK_TRAVEL && dy / elapsed > COMMIT_VELOCITY));

    el.style.transition = `transform ${SLIDE_MS}ms cubic-bezier(0.2, 0.8, 0.3, 1)`;
    if (commit) {
      el.style.transform = `translateY(${height + 40}px)`;
      window.setTimeout(() => {
        opts.onDismiss();
        // The sheet unmounts on dismiss; clear inline styles anyway in case
        // the caller keeps the element (or the dismiss is a no-op).
        clearInline(el);
      }, SLIDE_MS);
    } else {
      el.style.transform = "translateY(0px)";
      window.setTimeout(() => clearInline(el), SLIDE_MS + 30);
    }
    dy = 0;
  }

  function track(on: boolean) {
    const method = on ? "addEventListener" : "removeEventListener";
    // Non-passive: onMove calls preventDefault to keep the drag from also
    // scrolling whatever is behind the header.
    window[method]("pointermove", onMove as EventListener, { passive: false } as never);
    window[method]("pointerup", onEnd as EventListener, true as never);
    window[method]("pointercancel", onEnd as EventListener, true as never);
  }

  function onPointerDown(e: PointerEvent) {
    if (e.pointerType === "mouse" || !e.isPrimary) return;
    if (!bottomSheetMode()) return;
    if ((e.target as HTMLElement).closest("button, a, input, textarea, select")) return;
    pointerId = e.pointerId;
    startY = e.clientY;
    startTime = e.timeStamp;
    dy = 0;
    track(true);
  }

  node.addEventListener("pointerdown", onPointerDown);
  // Vertical finger movement on the header belongs to this gesture, never
  // to browser scrolling — without this, pointermove streams stop the
  // moment the browser decides the touch is a scroll.
  const prevTouchAction = node.style.touchAction;
  node.style.touchAction = "none";

  return {
    update(next: SheetDragOptions) {
      opts = next;
    },
    destroy() {
      track(false);
      node.removeEventListener("pointerdown", onPointerDown);
      node.style.touchAction = prevTouchAction;
    },
  };
}

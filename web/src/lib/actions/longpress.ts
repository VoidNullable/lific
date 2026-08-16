// Long-press (press and hold) action for touch surfaces.
//
// Mobile has no hover, so every hover-revealed preview in the app needs a
// touch equivalent; press-and-hold is that equivalent (graph nodes, issue
// rows/cards, page rows). Mouse pointers are deliberately ignored: pointer
// devices with hover get hover previews, and a mouse held still for half a
// second usually means reading, not requesting.
//
// Gesture contract:
// - Fires after `duration` ms (default 425) of a primary touch/pen press
//   that travels less than 10px. Movement, release, or cancel before the
//   deadline aborts silently (a scroll or tap stays a scroll or tap).
// - Tracking listens on window during the press, not the element: pointer
//   capture (e.g. xyflow's node drag) retargets move/up events away from
//   descendants mid-gesture, and window-capture still sees them.
// - On fire, the gesture's artifacts are swallowed at window capture for a
//   beat: the native context menu (Android raises it on long-press at
//   roughly the same deadline) and the click that follows release, so a
//   long-press never *also* navigates or opens a browser menu. iOS's
//   text-selection callout is suppressed via -webkit-touch-callout on the
//   element (iOS never fires contextmenu for plain elements).

export interface LongPressOptions {
  onLongPress: (event: PointerEvent) => void;
  /** Milliseconds before the press fires. Default 425 — deliberately under
   *  Android's native long-press so the swallow is armed before the
   *  browser's contextmenu arrives. */
  duration?: number;
  enabled?: boolean;
}

const MOVE_TOLERANCE = 10;
const SWALLOW_WINDOW_MS = 700;

export function longpress(node: HTMLElement, options: LongPressOptions) {
  let opts = options;
  let timer: number | undefined;
  let pointerId: number | null = null;
  let startX = 0;
  let startY = 0;

  function swallowGestureArtifacts() {
    const swallow = (ev: Event) => {
      ev.preventDefault();
      ev.stopPropagation();
    };
    window.addEventListener("contextmenu", swallow, { capture: true });
    window.addEventListener("click", swallow, { capture: true });
    window.setTimeout(() => {
      window.removeEventListener("contextmenu", swallow, { capture: true });
      window.removeEventListener("click", swallow, { capture: true });
    }, SWALLOW_WINDOW_MS);
  }

  function onWindowMove(e: PointerEvent) {
    if (e.pointerId !== pointerId) return;
    if (Math.hypot(e.clientX - startX, e.clientY - startY) > MOVE_TOLERANCE) {
      cancel();
    }
  }

  function onWindowEnd(e: PointerEvent) {
    if (e.pointerId !== pointerId) return;
    cancel();
  }

  function trackWindow(on: boolean) {
    const method = on ? "addEventListener" : "removeEventListener";
    window[method]("pointermove", onWindowMove as EventListener, { capture: true } as never);
    window[method]("pointerup", onWindowEnd as EventListener, { capture: true } as never);
    window[method]("pointercancel", onWindowEnd as EventListener, { capture: true } as never);
  }

  function cancel() {
    if (timer !== undefined) {
      clearTimeout(timer);
      timer = undefined;
    }
    if (pointerId !== null) {
      pointerId = null;
      trackWindow(false);
    }
  }

  function onPointerDown(e: PointerEvent) {
    if (opts.enabled === false) return;
    if (e.pointerType === "mouse" || !e.isPrimary) return;
    cancel();
    pointerId = e.pointerId;
    startX = e.clientX;
    startY = e.clientY;
    trackWindow(true);
    timer = window.setTimeout(() => {
      timer = undefined;
      const fired = e;
      cancel();
      swallowGestureArtifacts();
      // Subtle confirmation on hardware that supports it (Android).
      navigator.vibrate?.(8);
      opts.onLongPress(fired);
    }, opts.duration ?? 425);
  }

  node.addEventListener("pointerdown", onPointerDown);
  const prevCallout = node.style.getPropertyValue("-webkit-touch-callout");
  node.style.setProperty("-webkit-touch-callout", "none");

  return {
    update(next: LongPressOptions) {
      opts = next;
    },
    destroy() {
      cancel();
      node.removeEventListener("pointerdown", onPointerDown);
      node.style.setProperty("-webkit-touch-callout", prevCallout);
    },
  };
}

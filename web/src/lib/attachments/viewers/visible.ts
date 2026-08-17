// LIF-418: fire once when an element first scrolls into view.
//
// The inline viewers fetch attachment bytes to render a preview. A page with
// twelve attached logs must not fetch twelve logs on load, so every viewer
// waits for its own card to become visible. `rootMargin` starts the fetch
// slightly before the card reaches the viewport, so the preview is usually
// already there by the time it is read.

export interface WhenVisibleOptions {
  onVisible: () => void;
  /** Skip observation entirely (e.g. content is already loaded). */
  enabled?: boolean;
  rootMargin?: string;
}

export function whenVisible(node: HTMLElement, options: WhenVisibleOptions) {
  let fired = false;
  let observer: IntersectionObserver | null = null;

  function start(opts: WhenVisibleOptions) {
    if (fired || opts.enabled === false) return;
    // Environments without IntersectionObserver (older WebViews, SSR-ish test
    // harnesses) get the eager behavior rather than nothing at all.
    if (typeof IntersectionObserver === "undefined") {
      fired = true;
      opts.onVisible();
      return;
    }
    observer = new IntersectionObserver(
      (entries) => {
        if (!entries.some((e) => e.isIntersecting)) return;
        fired = true;
        observer?.disconnect();
        observer = null;
        opts.onVisible();
      },
      { rootMargin: opts.rootMargin ?? "200px" },
    );
    observer.observe(node);
  }

  start(options);

  return {
    update(next: WhenVisibleOptions) {
      options = next;
      if (!observer) start(next);
    },
    destroy() {
      observer?.disconnect();
      observer = null;
    },
  };
}

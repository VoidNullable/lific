// Preserve browser event evidence when a native new-tab gesture fails.
import { mkdtemp } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join } from "node:path";
import type { Locator, Page } from "playwright";

type Gesture = "ctrl" | "middle";
type OpenPopup = (link: Locator, gesture: Gesture, verify: (popup: Page) => Promise<void>, afterClose?: () => Promise<void>) => Promise<void>;

export async function withNativeLinkDiagnostics(page: Page, run: (openPopup: OpenPopup) => Promise<void>) {
  const context = page.context();
  let phase = "setup";
  let tracing = false;
  const cdp = await context.newCDPSession(page);
  const windowOpens: unknown[] = [];
  await cdp.send("Page.enable");
  cdp.on("Page.windowOpen", event => {
    windowOpens.push({ phase, ...event });
    if (windowOpens.length > 32) windowOpens.shift();
  });
  try {
    await context.tracing.start({ screenshots: true, snapshots: true, sources: true });
    tracing = true;
    await page.evaluate(() => {
      const w = window as any;
      const events: Record<string, unknown>[] = [], samples: Record<string, unknown>[] = [];
      let label = "setup", link: Element | null = null;
      const timers = new Set<ReturnType<typeof setTimeout>>();
      const describe = (target: EventTarget | null) => target instanceof Element
        ? { tag: target.tagName, id: target.id, role: target.getAttribute("role"), text: target.textContent?.trim().slice(0, 80) }
        : target === document ? "document" : target === window ? "window" : null;
      const state = () => ({
        active: describe(document.activeElement), hasFocus: document.hasFocus(), visibility: document.visibilityState,
        menuOpen: typeof w.fixture?.menuOpen === "function" ? w.fixture.menuOpen() : !!document.querySelector('[data-context-menu]'),
        scrollX, scrollY, menuScrollTop: document.querySelector('[data-context-menu]')?.scrollTop,
      });
      const geometry = (el: Element | null) => el ? {
        connected: el.isConnected, rect: el.getBoundingClientRect().toJSON(), scrollTop: el.scrollTop,
        scrollLeft: el.scrollLeft, scrollHeight: el.scrollHeight, clientHeight: el.clientHeight,
      } : null;
      const snapshot = () => ({ time: performance.now(), phase: label, ...state(), url: location.href,
        fonts: document.fonts.status, viewport: { width: innerWidth, height: innerHeight },
        menu: geometry(document.querySelector('[data-context-menu]')), link: geometry(link) });
      const capture = (event: Event) => {
        const mouse = event instanceof MouseEvent ? event : null;
        const keyboard = event instanceof KeyboardEvent ? event : null;
        const entry: Record<string, unknown> = {
          time: performance.now(), phase: label, type: event.type, target: describe(event.target),
          anchor: event.target instanceof Element ? event.target.closest("a")?.getAttribute("href") : null,
          button: mouse?.button, buttons: mouse?.buttons, ctrl: mouse?.ctrlKey, meta: mouse?.metaKey,
          shift: mouse?.shiftKey, alt: mouse?.altKey, x: mouse?.clientX, y: mouse?.clientY,
          trusted: event.isTrusted, defaultPreventedCapture: event.defaultPrevented, ...state(),
          key: keyboard?.key, code: keyboard?.code,
        };
        events.push(entry);
        if (events.length > 160) events.shift();
        if (mouse) {
          // A later task observes cancellation after all dispatch listeners have run.
          const timer = setTimeout(() => {
            timers.delete(timer);
            entry.defaultPreventedAfterDispatch = event.defaultPrevented;
            entry.afterDispatch = state();
          }, 0);
          timers.add(timer);
        }
      };
      const types = ["pointerover", "pointerenter", "pointerdown", "pointerup", "mousedown", "mouseup", "click", "auxclick", "scroll", "focusin", "focus", "blur", "keydown", "keyup"];
      types.forEach(type => window.addEventListener(type, capture, true));
      document.addEventListener("visibilitychange", capture, true);
      w.nativeLinkDiagnostics = {
        mark: (next: string, target?: Element) => {
          label = next;
          if (target) link = target;
          samples.push(snapshot());
          if (samples.length > 32) samples.shift();
        },
        dump: () => ({ current: snapshot(), samples, events, actions: w.actions }),
        dispose: () => {
          types.forEach(type => window.removeEventListener(type, capture, true));
          document.removeEventListener("visibilitychange", capture, true);
          timers.forEach(timer => clearTimeout(timer));
          delete w.nativeLinkDiagnostics;
        },
      };
    });
    const mark = async (next: string) => {
      phase = next;
      await page.evaluate(next => (window as any).nativeLinkDiagnostics.mark(next), next);
    };
    await run(async (link, gesture, verify, afterClose) => {
      const label = `${await link.getAttribute("href")} ${gesture}-click`;
      phase = `${label}: before gesture`;
      await link.evaluate((el, phase) => (window as any).nativeLinkDiagnostics.mark(phase, el), phase);
      console.log(`[native links:${label}] clicking and waiting for page`);
      // Keep the modifier held through the native tab-open acknowledgment.
      // Release it in finally so a failed gesture cannot affect later checks.
      if (gesture === "ctrl") await page.keyboard.down("Control");
      let popup: Page;
      try {
        [popup] = await Promise.all([
          context.waitForEvent("page").then(popup => {
            console.log(`[native links:${label}] page received`);
            return popup;
          }),
          link.click(gesture === "ctrl" ? {} : { button: "middle" })
            .then(() => console.log(`[native links:${label}] click completed`)),
        ]);
      } finally {
        if (gesture === "ctrl") await page.keyboard.up("Control");
      }
      phase = `${label}: popup load`;
      await popup.waitForLoadState();
      phase = `${label}: verify popup`;
      await verify(popup);
      await mark(`${label}: before popup close`);
      await popup.close();
      await mark(`${label}: after popup close`);
      await afterClose?.();
      console.log(`[native links:${label}] passed; popup closed`);
    });
  } catch (error) {
    console.error(`[native links:${phase}] failed`, error);
    try {
      console.error(JSON.stringify({ phase, windowOpens, pages: context.pages().map(p => p.url()),
        source: await page.evaluate(() => (window as any).nativeLinkDiagnostics?.dump() ?? { diagnosticsLost: true, url: location.href }),
      }, null, 2));
    } catch (diagnosticError) { console.error("Native link state capture failed", diagnosticError); }
    if (tracing) {
      try {
        const path = join(await mkdtemp(join(tmpdir(), "lific-native-links-")), "trace.zip");
        await context.tracing.stop({ path });
        tracing = false;
        console.error(`Native link failure trace: ${path}`);
      } catch (traceError) { console.error("Native link trace capture failed", traceError); }
    }
    throw error;
  } finally {
    await cdp.detach();
    try { await page.evaluate(() => (window as any).nativeLinkDiagnostics?.dispose()); }
    catch (error) { console.error("Native link diagnostic cleanup failed", error); }
    if (tracing) {
      try { await context.tracing.stop(); }
      catch (error) { console.error("Native link trace cleanup failed", error); }
    }
  }
}

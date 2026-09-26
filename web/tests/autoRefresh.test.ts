import { afterEach, describe, expect, test } from "bun:test";
import { REALTIME_INVALIDATE_EVENT, startAutoRefresh } from "../src/lib/autoRefresh.svelte";

const originalDocument = globalThis.document;
const originalWindow = globalThis.window;

function installBrowserTargets() {
  const documentTarget = new EventTarget();
  Object.defineProperty(documentTarget, "hidden", { value: false });
  const windowTarget = new EventTarget();
  Object.defineProperty(globalThis, "document", {
    configurable: true,
    value: documentTarget,
  });
  Object.defineProperty(globalThis, "window", {
    configurable: true,
    value: windowTarget,
  });
  return { documentTarget, windowTarget };
}

function restoreGlobal(name: "document" | "window", value: typeof document | typeof window) {
  if (value === undefined) {
    Reflect.deleteProperty(globalThis, name);
  } else {
    Object.defineProperty(globalThis, name, { configurable: true, value });
  }
}

afterEach(() => {
  restoreGlobal("document", originalDocument);
  restoreGlobal("window", originalWindow);
});

describe("auto refresh", () => {
  test("bounds refresh delay during a continuous realtime event burst", async () => {
    const { windowTarget } = installBrowserTargets();
    let refreshCount = 0;
    const stop = startAutoRefresh({
      refresh: () => {
        refreshCount += 1;
      },
      realtimeDebounceMs: 40,
      realtimeMaxWaitMs: 100,
      shouldRefresh: () => true,
    });

    try {
      for (let i = 0; i < 8; i += 1) {
        windowTarget.dispatchEvent(
          new CustomEvent(REALTIME_INVALIDATE_EVENT, {
            detail: { type: "issue.updated" },
          }),
        );
        await new Promise((resolve) => setTimeout(resolve, 20));
        if (i === 5) expect(refreshCount).toBeGreaterThan(0);
      }

      expect(refreshCount).toBeGreaterThan(0);
    } finally {
      stop();
    }
  });
});

import { afterEach, expect, test } from "bun:test";
import { selectedIssueExport } from "../src/lib/issues/export";

const originalFetch = globalThis.fetch;
const originalStorage = globalThis.localStorage;
afterEach(() => {
  globalThis.fetch = originalFetch;
  globalThis.localStorage = originalStorage;
});

test("exports only requested issues, in order, using authentication", async () => {
  globalThis.localStorage = { getItem: () => "test-token" } as Storage;
  const calls: string[] = [];
  globalThis.fetch = (async (url, init) => {
    calls.push(String(url));
    expect(init?.headers).toEqual({ Authorization: "Bearer test-token" });
    return new Response(`# ${String(url).split("/").pop()}`);
  }) as typeof fetch;
  const blob = await selectedIssueExport(["DEMO-1", "DEMO-3"]);
  expect(await blob.text()).toBe("# DEMO-1\n\n---\n\n# DEMO-3");
  expect(calls).toEqual(["/api/export/issues/DEMO-1", "/api/export/issues/DEMO-3"]);
});

test("refuses a partial export when a selected issue fails", async () => {
  globalThis.localStorage = { getItem: () => null } as Storage;
  let calls = 0;
  globalThis.fetch = (async () => ++calls === 1 ? new Response("first") : new Response("denied", { status: 403 })) as typeof fetch;
  await expect(selectedIssueExport(["DEMO-1", "DEMO-2", "DEMO-3"])).rejects.toThrow("DEMO-2 (HTTP 403)");
  expect(calls).toBe(2);
});

test("cancels the response when aggregate bytes exceed the limit", async () => {
  globalThis.localStorage = { getItem: () => null } as Storage;
  let cancelled = false;
  globalThis.fetch = (async () => new Response(new ReadableStream({
    pull(controller) { controller.enqueue(new Uint8Array(1024 * 1024)); },
    cancel() { cancelled = true; },
  }))) as typeof fetch;
  await expect(selectedIssueExport(["DEMO-1"])).rejects.toThrow("16 MiB");
  expect(cancelled).toBe(true);
});

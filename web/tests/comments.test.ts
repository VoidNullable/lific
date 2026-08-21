import { afterAll, afterEach, beforeAll, expect, test } from "bun:test";
import type { Component } from "svelte";
import { createServer, type ViteDevServer } from "vite";
import { fileURLToPath } from "node:url";
import type { Comment } from "../src/lib/api";

Object.assign(globalThis, {
  window: {
    location: {
      origin: "http://localhost",
      hash: "",
      pathname: "/LIFIC/issues/LIFIC-6",
      search: "",
    },
  },
  localStorage: { getItem: () => null },
});

// Both modules read `window` at import time, so they load after the stub
// above rather than through a hoisted static import.
const { COMMENT_PAGE_SIZE, deleteComment, listComments, listPageComments, updateComment } =
  await import("../src/lib/api");
const {
  ANCHOR_AUTO_PAGE_BUDGET,
  COMMENT_REFRESH_PAGE_BUDGET,
  COMMENT_REFRESH_TRANSFER_LIMIT,
  COMMENT_WINDOW_RETRY_LIMIT,
  commentOpIsCurrent,
  commentWindowOutcome,
  compareComments,
  reconcileCommentWindow,
  ANCHOR_AUTO_SEARCH_LIMIT,
  anchorNeedsOlderPage,
  canManageComment,
  commentKeyboardAction,
  commentWasEdited,
  loadCommentWindow,
  nextAnchorAttempt,
  olderCursor,
  prependOlderComments,
  removeComment,
  upsertComment,
} = await import("../src/lib/commentState");

const originalFetch = globalThis.fetch;
let vite: ViteDevServer;
let Comments: Component<any>;
let renderComponent: typeof import("svelte/server").render;

beforeAll(async () => {
  vite = await createServer({
    server: { middlewareMode: true },
    appType: "custom",
    resolve: {
      alias: {
        dompurify: fileURLToPath(new URL("./dompurify.ssr.ts", import.meta.url)),
      },
    },
  });
  ({ default: Comments } = await vite.ssrLoadModule("/src/lib/Comments.svelte"));
  ({ render: renderComponent } = await vite.ssrLoadModule("svelte/server"));
}, 20_000);

afterAll(async () => {
  await vite.close();
});

afterEach(() => {
  globalThis.fetch = originalFetch;
});

function comment(overrides: Partial<Comment> = {}): Comment {
  return {
    id: 42,
    issue_id: 7,
    page_id: null,
    user_id: 3,
    author: "owner",
    author_display_name: "Owner",
    content: "Original",
    created_at: "2026-08-13 10:00:00",
    updated_at: "2026-08-13 10:00:00",
    ...overrides,
  };
}

test("only the comment author gets web mutation actions", () => {
  const own = comment();

  expect(canManageComment(own, { id: 3 }, true)).toBe(true);
  expect(canManageComment(own, { id: 9 }, true)).toBe(false);
  expect(canManageComment(own, { id: 3 }, false)).toBe(false);
});

test("routes comment shortcuts by the focused interaction", () => {
  expect(commentKeyboardAction("new", "Enter", true)).toBe("submit");
  expect(commentKeyboardAction("edit", "Enter", true)).toBe("save");
  expect(commentKeyboardAction("edit", "Escape", false)).toBe("cancel");
  expect(commentKeyboardAction("menu", "Escape", false)).toBe("close-menu");
  expect(commentKeyboardAction("new", "Escape", false)).toBeNull();
});

test("renders mutation actions only for the comment author", () => {
  const props = {
    comments: [comment()],
    onSubmit: async () => null,
    onUpdate: async () => null,
    onDelete: async () => false,
  };

  const owner = renderComponent(Comments, { props: { ...props, currentUser: { id: 3 } } }).body;
  const otherUser = renderComponent(Comments, { props: { ...props, currentUser: { id: 9 } } }).body;

  expect(owner).toContain("Comment 42 actions");
  expect(otherUser).not.toContain("Comment 42 actions");
});

test("marks comments edited only when the update timestamp changes", () => {
  expect(commentWasEdited(comment())).toBe(false);
  expect(commentWasEdited(comment({ updated_at: "2026-08-13 10:05:00" }))).toBe(true);
});

test("renders the original time and exact edited timestamp", () => {
  const edited = comment({ updated_at: "2026-08-13 10:05:00" });
  const html = renderComponent(Comments, {
    props: { comments: [edited], onSubmit: async () => null },
  }).body;

  expect(html).toContain("edited");
  expect(html).toContain('title="Edited 2026-08-13 10:05:00"');
});

test("folding a comment in by id is idempotent and keeps thread order", () => {
  const at = (id: number, second: number, content = "body") =>
    comment({ id, content, created_at: `2026-08-13 10:00:0${second}` });
  const first = at(1, 0);
  const third = at(3, 2);

  // Replace in place. An edit changes neither created_at nor id, so the
  // position is already right and re-sorting could only move it wrongly.
  const edited = at(1, 0, "Revised");
  expect(upsertComment([first, third], edited)).toEqual([edited, third]);

  // A brand new comment is the newest, so it lands at the end.
  const fourth = at(4, 3);
  expect(upsertComment([first, third], fourth)).toEqual([first, third, fourth]);

  // A comment that is *not* in the loaded window belongs where its ordering
  // key says, not on the end. Appending it would put it visibly out of order
  // and hand the next keyset cursor the wrong boundary.
  const second = at(2, 1);
  expect(upsertComment([first, third], second)).toEqual([first, second, third]);

  // created_at has one-second resolution, so ties are ordinary. The id half
  // of the key decides them, exactly as the cursor does.
  const tie = comment({ id: 2, created_at: third.created_at });
  expect(upsertComment([first, third], tie).map((c) => c.id)).toEqual([1, 2, 3]);
  const laterTie = comment({ id: 9, created_at: third.created_at });
  expect(upsertComment([first, third], laterTie).map((c) => c.id)).toEqual([1, 3, 9]);

  // Applying the same result twice cannot duplicate a keyed row or move it,
  // which is how a mutation and a refresh carrying the same comment collide.
  const once = upsertComment([first, third], second);
  expect(upsertComment(once, second)).toEqual([first, second, third]);
  expect(new Set(upsertComment(once, second).map((c) => c.id)).size).toBe(3);

  // The input is never mutated.
  const input = [first, third];
  upsertComment(input, second);
  expect(input).toEqual([first, third]);

  // Delete is idempotent too: removing a comment that is already gone is a
  // no-op rather than an error or a resurrection.
  expect(removeComment([first, third], 1)).toEqual([third]);
  expect(removeComment(removeComment([first, third], 1), 1)).toEqual([third]);
});

test("the thread comparator is the key the cursor pages by", () => {
  const early = comment({ id: 9, created_at: "2026-08-13 10:00:00" });
  const late = comment({ id: 2, created_at: "2026-08-13 10:00:01" });

  // Timestamp first, even when the ids run the other way.
  expect(compareComments(early, late)).toBeLessThan(0);
  expect(compareComments(late, early)).toBeGreaterThan(0);
  // Then id, for the common same-second case.
  const tie = comment({ id: 10, created_at: early.created_at });
  expect(compareComments(early, tie)).toBeLessThan(0);
  expect(compareComments(early, early)).toBe(0);
});

test("updates a comment through its typed API call", async () => {
  let call: { url: string; init?: RequestInit } | undefined;
  globalThis.fetch = (async (url, init) => {
    call = { url: String(url), init };
    return new Response(JSON.stringify({ id: 42, content: "Revised" }), { status: 200 });
  }) as typeof fetch;

  const result = await updateComment(42, "Revised");

  expect(result).toEqual({ ok: true, data: { id: 42, content: "Revised" } });
  expect(call).toMatchObject({
    url: "/api/comments/42",
    init: { method: "PUT", body: JSON.stringify({ content: "Revised" }) },
  });
});

// ── Bounded, stable comment paging ──────────────────────────
//
// Two things are being pinned here. First, the REST default is `order=asc`,
// so an unqualified request for a long thread hands back the *oldest* 50
// comments and the UI must never show that page and call it the thread.
// Second, a thread is written to while it is read, so paging back must be
// keyed on a position rather than an offset. An offset points at a different
// row the moment anyone posts above the reader, which surfaces as a repeated
// comment (a duplicate Svelte key, i.e. a runtime crash) or a skipped one.

/** A server holding `count` comments, answering keyset pages newest first. */
function stubThread(count: number) {
  const calls: string[] = [];
  globalThis.fetch = (async (url) => {
    calls.push(String(url));
    const params = new URL(String(url), "http://localhost").searchParams;
    const limit = Number(params.get("limit"));
    const beforeId = params.get("before_id");
    // Ids descend from `count`; every row shares a timestamp, which is the
    // realistic case and the reason the cursor carries an id at all.
    const highest = beforeId === null ? count : Number(beforeId) - 1;
    const rows = Array.from({ length: Math.max(0, Math.min(highest, limit)) }, (_, i) =>
      comment({ id: highest - i, content: `comment ${highest - i}` }),
    );
    return new Response(JSON.stringify(rows), { status: 200 });
  }) as typeof fetch;
  return calls;
}

test("requests the newest bounded comment page and shows it chronologically", async () => {
  const calls = stubThread(3);

  const res = await listComments(7);

  expect(calls).toEqual(["/api/issues/7/comments?order=desc&limit=51"]);
  expect(res.ok).toBe(true);
  if (!res.ok) return;
  // Reversed back into reading order: oldest of the page first.
  expect(res.data.items.map((c) => c.content)).toEqual([
    "comment 1",
    "comment 2",
    "comment 3",
  ]);
  expect(res.data.hasMore).toBe(false);
  expect(res.data.nextCursor).toEqual({ created_at: "2026-08-13 10:00:00", id: 1 });
});

test("infers hasMore from the over-fetched row without ever showing it", async () => {
  stubThread(1000);

  const res = await listComments(7);

  expect(res.ok).toBe(true);
  if (!res.ok) return;
  expect(res.data.items).toHaveLength(COMMENT_PAGE_SIZE);
  expect(res.data.hasMore).toBe(true);
  // The newest comment is the last one rendered, never dropped for an
  // over-fetch row.
  expect(res.data.items.at(-1)?.content).toBe("comment 1000");
  expect(res.data.items[0].content).toBe(`comment ${1000 - COMMENT_PAGE_SIZE + 1}`);
  expect(res.data.nextCursor?.id).toBe(1000 - COMMENT_PAGE_SIZE + 1);
});

test("sends the keyset cursor pair for older pages and never exceeds the cap", async () => {
  const calls = stubThread(1000);

  await listComments(7, { created_at: "2026-08-13 10:00:00", id: 951 });
  await listPageComments(9, { created_at: "2026-08-13 10:00:00", id: 400 }, 499);

  expect(calls).toEqual([
    "/api/issues/7/comments?order=desc&limit=51&before_created_at=2026-08-13+10%3A00%3A00&before_id=951",
    "/api/pages/9/comments?order=desc&limit=500&before_created_at=2026-08-13+10%3A00%3A00&before_id=400",
  ]);
  for (const url of calls) {
    const params = new URL(url, "http://localhost").searchParams;
    expect(Number(params.get("limit"))).toBeLessThanOrEqual(500);
    // Offsets are never sent: they are the thing being replaced.
    expect(params.get("offset")).toBeNull();
    expect(params.get("order")).toBe("desc");
  }
});

test("pages back through a thread that is being written to, without duplicates", async () => {
  // A live thread: every page is served relative to the cursor, so comments
  // arriving above the reader cannot shift the boundary.
  let total = 6;
  const calls: string[] = [];
  globalThis.fetch = (async (url) => {
    calls.push(String(url));
    const params = new URL(String(url), "http://localhost").searchParams;
    const limit = Number(params.get("limit"));
    const beforeId = params.get("before_id");
    const highest = beforeId === null ? total : Number(beforeId) - 1;
    const rows = Array.from({ length: Math.max(0, Math.min(highest, limit)) }, (_, i) =>
      comment({ id: highest - i, content: `comment ${highest - i}` }),
    );
    // Someone posts a new comment between every request.
    total += 1;
    return new Response(JSON.stringify(rows), { status: 200 });
  }) as typeof fetch;

  let loaded: Comment[] = [];
  for (let page = 0; page < 3; page += 1) {
    const res = await listComments(7, olderCursor(loaded), 2);
    expect(res.ok).toBe(true);
    if (!res.ok) return;
    loaded = prependOlderComments(loaded, res.data.items);
  }

  expect(loaded.map((c) => c.id)).toEqual([1, 2, 3, 4, 5, 6]);
  expect(new Set(loaded.map((c) => c.id)).size).toBe(loaded.length);
});

test("dedupes defensively when two pages overlap", () => {
  const onScreen = [comment({ id: 5 }), comment({ id: 6 })];
  const overlapping = [comment({ id: 3 }), comment({ id: 4 }), comment({ id: 5 })];

  const merged = prependOlderComments(onScreen, overlapping);

  expect(merged.map((c) => c.id)).toEqual([3, 4, 5, 6]);
  expect(new Set(merged.map((c) => c.id)).size).toBe(merged.length);
  // The copy already on screen wins, so an in-place edit is not undone by a
  // stale row arriving from an older page.
  expect(prependOlderComments([comment({ id: 5, content: "edited" })], [comment({ id: 5 })])[0]
    .content).toBe("edited");
});

test("olderCursor names the oldest loaded comment, or nothing when empty", () => {
  expect(olderCursor([])).toBeNull();
  expect(olderCursor([comment({ id: 4 }), comment({ id: 9 })])).toEqual({
    created_at: "2026-08-13 10:00:00",
    id: 4,
  });
});

// ── Refreshing the whole loaded window ──────────────────────

/** A fetcher over a fixed thread that answers at whatever page size the window
 *  asks for, and records what each request cost on the wire.
 *
 *  `commentPage` requests `size + 1` rows so it can answer `hasMore`, so that
 *  is what a request actually transfers. Modelling it here is the whole point:
 *  a budget counted in retained rows under-counts every request by one. */
function windowFetcher(rows: Comment[]) {
  const calls: { before: number | null; size: number }[] = [];
  const newestFirst = [...rows].sort((a, b) => b.id - a.id);
  const fetchPage = async (before: { id: number } | null, size: number) => {
    calls.push({ before: before?.id ?? null, size });
    const eligible = before === null
      ? newestFirst
      : newestFirst.filter((row) => row.id < before.id);
    const transferred = eligible.slice(0, size + 1);
    const hasMore = transferred.length > size;
    const items = transferred.slice(0, size).slice().reverse();
    return {
      ok: true as const,
      data: {
        items,
        hasMore,
        nextCursor: items.length > 0
          ? { created_at: items[0].created_at, id: items[0].id }
          : before,
      },
    };
  };
  const cursors = () => calls.map((call) => call.before);
  const transferred = () => calls.reduce((total, call) => total + call.size + 1, 0);
  return { fetchPage, calls, cursors, transferred };
}

test("a refresh reconciles every loaded page, not just the newest", async () => {
  // Six comments loaded across three pages of two. The one in the middle was
  // edited elsewhere and the oldest was deleted; both must land.
  const server = [1, 2, 4, 5, 6].map((id) =>
    comment({ id, content: id === 4 ? "edited elsewhere" : `comment ${id}` }),
  );
  const { fetchPage, cursors } = windowFetcher(server);

  const res = await loadCommentWindow(fetchPage, 6, 2);

  expect(res.ok).toBe(true);
  if (!res.ok) return;
  expect(res.data.items.map((c) => c.id)).toEqual([1, 2, 4, 5, 6]);
  expect(res.data.items.find((c) => c.id === 4)?.content).toBe("edited elsewhere");
  expect(res.data.hasOlder).toBe(false);
  // Walked back with cursors, never an offset, and stopped at the start.
  expect(cursors()).toEqual([null, 5, 2]);
});

test("a refresh never fetches more than the reader already loaded", async () => {
  const server = Array.from({ length: 100 }, (_, i) => comment({ id: i + 1 }));
  const { fetchPage, cursors } = windowFetcher(server);

  // Nothing loaded yet: one page, and no crawl into the rest of the thread.
  const first = await loadCommentWindow(fetchPage, 0, 10);
  expect(first.ok).toBe(true);
  if (!first.ok) return;
  expect(first.data.items).toHaveLength(10);
  expect(first.data.hasOlder).toBe(true);
  expect(cursors()).toHaveLength(1);

  // Thirty rows on screen: three pages, and it stops there.
  const { fetchPage: refetch, cursors: refreshCursors } = windowFetcher(server);
  const refreshed = await loadCommentWindow(refetch, 30, 10);
  expect(refreshed.ok).toBe(true);
  if (!refreshed.ok) return;
  expect(refreshed.data.items).toHaveLength(30);
  expect(refreshed.data.items.map((c) => c.id)).toEqual(
    Array.from({ length: 30 }, (_, i) => i + 71),
  );
  expect(refreshed.data.hasOlder).toBe(true);
  expect(refreshCursors()).toHaveLength(3);
});

test("a refresh keeps exactly the rows it was asked for, not whole pages", async () => {
  // 51 rows on screen and a page size of 50: the second page overshoots by 49.
  // Handing all 100 back would grow the thread a little on every refresh.
  const server = Array.from({ length: 500 }, (_, i) => comment({ id: i + 1 }));
  const { fetchPage } = windowFetcher(server);

  const res = await loadCommentWindow(fetchPage, 51, 50);

  expect(res.ok).toBe(true);
  if (!res.ok) return;
  expect(res.data.items).toHaveLength(51);
  // The newest side is what survives the trim, and it is still chronological.
  expect(res.data.items.at(-1)?.id).toBe(500);
  expect(res.data.items[0].id).toBe(450);
  // The rows just trimmed away are older comments, so say so.
  expect(res.data.hasOlder).toBe(true);
});

test("an exact page boundary needs no trimming", async () => {
  const server = Array.from({ length: 500 }, (_, i) => comment({ id: i + 1 }));
  const { fetchPage, cursors } = windowFetcher(server);

  const res = await loadCommentWindow(fetchPage, 100, 50);

  expect(res.ok).toBe(true);
  if (!res.ok) return;
  expect(res.data.items).toHaveLength(100);
  expect(res.data.items[0].id).toBe(401);
  expect(res.data.items.at(-1)?.id).toBe(500);
  expect(res.data.hasOlder).toBe(true);
  expect(cursors()).toHaveLength(2);
});

test("a thread shorter than the window is returned whole", async () => {
  const server = Array.from({ length: 7 }, (_, i) => comment({ id: i + 1 }));
  const { fetchPage } = windowFetcher(server);

  const res = await loadCommentWindow(fetchPage, 51, 50);

  expect(res.ok).toBe(true);
  if (!res.ok) return;
  expect(res.data.items.map((c) => c.id)).toEqual([1, 2, 3, 4, 5, 6, 7]);
  expect(res.data.hasOlder).toBe(false);
});

test("an automatic refresh is capped no matter how much history is loaded", async () => {
  // A reader who pressed Load older twenty times must not turn every focus
  // event into twenty requests. The budget is absolute: `minRows` cannot lift
  // it, so no call site can ask for an unbounded refresh by accident.
  const server = Array.from({ length: 5000 }, (_, i) => comment({ id: i + 1 }));
  const { fetchPage, calls, transferred } = windowFetcher(server);

  const res = await loadCommentWindow(fetchPage, 5000);

  // At the default page size a request costs 51 rows, so nine of them fit
  // inside the transfer limit and a tenth would breach it.
  expect(calls).toHaveLength(9);
  expect(transferred()).toBe(9 * (COMMENT_PAGE_SIZE + 1));
  expect(transferred()).toBeLessThanOrEqual(COMMENT_REFRESH_TRANSFER_LIMIT);
  expect(res.ok).toBe(true);
  if (!res.ok) return;
  expect(res.data.items).toHaveLength(9 * COMMENT_PAGE_SIZE);
  // The newest 450, in reading order, and honest that more sit below them.
  expect(res.data.items.at(-1)?.id).toBe(5000);
  expect(res.data.items[0].id).toBe(5000 - 9 * COMMENT_PAGE_SIZE + 1);
  expect(res.data.hasOlder).toBe(true);
});

test("refresh bounds are enforced against any caller argument", async () => {
  const server = Array.from({ length: 5000 }, (_, i) => comment({ id: i + 1 }));

  // Nonsense from a caller must still produce a working, bounded refresh
  // rather than a loop that never runs or never stops.
  const nonsense: [number, number][] = [
    [Number.NaN, Number.NaN],
    [0, 0],
    [-10, -10],
    [Number.POSITIVE_INFINITY, Number.POSITIVE_INFINITY],
    [10_000, 10_000],
    [50.9, 10.9],
    [1, 10_000],
    [49, 10],
  ];
  for (const [size, budget] of nonsense) {
    const { fetchPage, calls, transferred } = windowFetcher(server);
    const res = await loadCommentWindow(fetchPage, Number.NaN, size, budget);

    // Always at least one request, never more than the request budget.
    expect(calls.length).toBeGreaterThanOrEqual(1);
    expect(calls.length).toBeLessThanOrEqual(COMMENT_REFRESH_PAGE_BUDGET);
    // Every individual request stays inside the page bound, and so inside the
    // server's own 500-row cap once the lookahead row is added.
    for (const call of calls) {
      expect(call.size).toBeGreaterThanOrEqual(1);
      expect(call.size).toBeLessThanOrEqual(COMMENT_PAGE_SIZE);
    }
    // And the refresh as a whole never pulls more than its ceiling, counting
    // the lookahead row each request pays for.
    expect(transferred()).toBeLessThanOrEqual(COMMENT_REFRESH_TRANSFER_LIMIT);
    expect(res.ok).toBe(true);
    if (!res.ok) return;
    expect(res.data.items.length).toBeGreaterThanOrEqual(1);
    expect(res.data.items.length).toBeLessThanOrEqual(COMMENT_REFRESH_TRANSFER_LIMIT);
    expect(res.data.items.length).toBeLessThanOrEqual(calls.length * COMMENT_PAGE_SIZE);
  }
});

test("a fractional page size rounds down and still pages", async () => {
  const server = Array.from({ length: 40 }, (_, i) => comment({ id: i + 1 }));
  const { fetchPage, calls, cursors } = windowFetcher(server);

  // 7.9 rows per page, 2.9 pages: floored to 7 and 2.
  const res = await loadCommentWindow(fetchPage, 100, 7.9, 2.9);

  expect(calls.every((call) => call.size === 7)).toBe(true);
  expect(cursors()).toHaveLength(2);
  expect(res.ok).toBe(true);
  if (!res.ok) return;
  expect(res.data.items).toHaveLength(14);
  expect(res.data.items.at(-1)?.id).toBe(40);
  expect(res.data.hasOlder).toBe(true);
});

test("a capped refresh keeps the older rows the reader loaded by hand", () => {
  // 800 rows on screen; the refresh reconciled only the newest 500. The other
  // 300 are below the refreshed window and stay, contiguous and unduplicated.
  const onScreen = {
    items: Array.from({ length: 800 }, (_, i) => comment({ id: i + 1 })),
    hasOlder: true,
  };
  const refreshed = {
    items: Array.from({ length: 500 }, (_, i) =>
      comment({ id: 301 + i, content: 301 + i === 400 ? "edited elsewhere" : "body" }),
    ),
    hasOlder: true,
  };

  const merged = reconcileCommentWindow(onScreen, refreshed);

  expect(merged.items).toHaveLength(800);
  expect(merged.items[0].id).toBe(1);
  expect(merged.items.at(-1)?.id).toBe(800);
  expect(new Set(merged.items.map((c) => c.id)).size).toBe(800);
  // No gap at the seam between preserved and refreshed rows.
  expect(merged.items.map((c) => c.id)).toEqual(
    Array.from({ length: 800 }, (_, i) => i + 1),
  );
  // Inside the refreshed window the server's copy wins.
  expect(merged.items.find((c) => c.id === 400)?.content).toBe("edited elsewhere");
  // The oldest loaded row did not move, so what lies below it did not either.
  expect(merged.hasOlder).toBe(true);
});

test("reconciliation replaces the window when nothing older was preserved", () => {
  const onScreen = { items: [comment({ id: 9 }), comment({ id: 10 })], hasOlder: true };

  // The refresh reaches further back than anything on screen: it is the whole
  // truth, including its own hasOlder.
  const deeper = {
    items: [comment({ id: 8 }), comment({ id: 9 }), comment({ id: 10 })],
    hasOlder: false,
  };
  expect(reconcileCommentWindow(onScreen, deeper)).toEqual(deeper);

  // A comment deleted inside the refreshed window is gone, not resurrected.
  const withoutNine = { items: [comment({ id: 8 }), comment({ id: 10 })], hasOlder: false };
  expect(reconcileCommentWindow(onScreen, withoutNine).items.map((c) => c.id)).toEqual([8, 10]);

  // An empty refresh means an empty thread.
  expect(reconcileCommentWindow(onScreen, { items: [], hasOlder: false })).toEqual({
    items: [],
    hasOlder: false,
  });
});

test("reconciliation orders preserved rows by the same key the cursor uses", () => {
  // Same timestamp on both sides, so the id half of the ordering key decides
  // which rows are below the refreshed boundary.
  const onScreen = {
    items: [comment({ id: 5 }), comment({ id: 6 }), comment({ id: 7 })],
    hasOlder: true,
  };
  const refreshed = { items: [comment({ id: 6 }), comment({ id: 7 })], hasOlder: true };

  const merged = reconcileCommentWindow(onScreen, refreshed);

  expect(merged.items.map((c) => c.id)).toEqual([5, 6, 7]);
  expect(new Set(merged.items.map((c) => c.id)).size).toBe(3);
});

// ── Operation ordering within one route ─────────────────────

test("only the newest comment-window operation may write", () => {
  // A refresh claims op 4 on route 2 and is still in flight when a manual
  // older page claims op 5. The refresh must not land.
  expect(commentOpIsCurrent({ route: 2, op: 4 }, 2, 5)).toBe(false);
  expect(commentOpIsCurrent({ route: 2, op: 5 }, 2, 5)).toBe(true);
  // A navigation invalidates an operation even if it still holds the newest
  // comment token, because that token belongs to the issue just left.
  expect(commentOpIsCurrent({ route: 2, op: 5 }, 3, 5)).toBe(false);
  // Both stale.
  expect(commentOpIsCurrent({ route: 1, op: 1 }, 3, 5)).toBe(false);
});

test("a replacement window applies, re-reads, or stands down", () => {
  // Nothing moved: this read is still the newest truth about the thread.
  expect(commentWindowOutcome({ route: 2, op: 4, epoch: 1 }, 2, 4, 1)).toBe("apply");

  // A mutation landed while the read was in flight. Applying it would undo an
  // edit the user watched succeed; discarding it would leave the thread as
  // just that edit, which is the navigate-away-and-back-with-a-pending-write
  // case. Read again against the epoch the mutation established.
  expect(commentWindowOutcome({ route: 2, op: 4, epoch: 1 }, 2, 4, 2)).toBe("retry");

  // A newer window operation owns the list and is already producing a better
  // answer, so this one must not touch anything, epoch notwithstanding.
  expect(commentWindowOutcome({ route: 2, op: 4, epoch: 1 }, 2, 5, 1)).toBe("abandon");
  expect(commentWindowOutcome({ route: 2, op: 4, epoch: 1 }, 2, 5, 2)).toBe("abandon");

  // The reader navigated. Route identity outranks everything else.
  expect(commentWindowOutcome({ route: 2, op: 4, epoch: 1 }, 3, 4, 1)).toBe("abandon");

  // The re-read loop is bounded, so sustained churn stops rather than storms.
  expect(COMMENT_WINDOW_RETRY_LIMIT).toBeGreaterThanOrEqual(1);
  expect(COMMENT_WINDOW_RETRY_LIMIT).toBeLessThanOrEqual(5);
});

test("a failed page aborts the refresh instead of showing a half window", async () => {
  const failure = async () => ({ ok: false as const, error: "offline", status: null });

  const res = await loadCommentWindow(failure, 30, 10);

  expect(res).toEqual({ ok: false, error: "offline", status: null });
});

test("an empty thread refreshes to an empty window without looping", async () => {
  const { fetchPage, cursors } = windowFetcher([]);

  const res = await loadCommentWindow(fetchPage, 50, 10);

  expect(res.ok).toBe(true);
  if (!res.ok) return;
  expect(res.data.items).toEqual([]);
  expect(res.data.hasOlder).toBe(false);
  expect(cursors()).toHaveLength(1);
});

// ── Deep links to a comment outside the newest page ─────────

test("a deep link to an unloaded comment asks for the previous page", () => {
  const onScreen = [comment({ id: 900 }), comment({ id: 901 })];

  // Not loaded, older pages exist, nothing in flight: fetch.
  expect(anchorNeedsOlderPage("comment-812", onScreen, true, false)).toBe(true);
  // Already on screen: nothing to do.
  expect(anchorNeedsOlderPage("comment-900", onScreen, true, false)).toBe(false);
  // A request is already in flight, so do not stack a second one.
  expect(anchorNeedsOlderPage("comment-812", onScreen, true, true)).toBe(false);
  // The thread has no older pages, so the comment is simply gone. Stop.
  expect(anchorNeedsOlderPage("comment-812", onScreen, false, false)).toBe(false);
  // No target, or one that is not a comment anchor.
  expect(anchorNeedsOlderPage(null, onScreen, true, false)).toBe(false);
  expect(anchorNeedsOlderPage("comment-abc", onScreen, true, false)).toBe(false);
  expect(anchorNeedsOlderPage("comment-0", onScreen, true, false)).toBe(false);
});

test("a failing anchor fetch is not retried until more comments arrive", () => {
  const onScreen = [comment({ id: 900 }), comment({ id: 901 })];

  const first = nextAnchorAttempt("issue:LIF-9", "comment-812", onScreen, true, false, null);
  expect(first).toEqual({
    parent: "issue:LIF-9",
    target: "comment-812",
    loaded: 2,
    pages: 1,
  });

  // The request failed: same target, same thread length, so stop rather than
  // hammer the endpoint.
  expect(nextAnchorAttempt("issue:LIF-9", "comment-812", onScreen, true, false, first)).toBeNull();

  // A page landed, so there is new state to act on and the walk continues.
  const grown = [comment({ id: 850 }), ...onScreen];
  expect(nextAnchorAttempt("issue:LIF-9", "comment-812", grown, true, false, first)).toEqual({
    parent: "issue:LIF-9",
    target: "comment-812",
    loaded: 3,
    pages: 2,
  });

  // A different deep link is a fresh walk even at the same length.
  expect(nextAnchorAttempt("issue:LIF-9", "comment-700", onScreen, true, false, first)).toEqual({
    parent: "issue:LIF-9",
    target: "comment-700",
    loaded: 2,
    pages: 1,
  });

  // Once the comment is on screen there is nothing left to fetch.
  expect(
    nextAnchorAttempt(
      "issue:LIF-9",
      "comment-812",
      [comment({ id: 812 }), ...onScreen],
      true,
      false,
      null,
    ),
  ).toBeNull();
});

test("the automatic anchor walk stops after its page budget", () => {
  // A link to a comment that was deleted, or that lives thousands of rows
  // back, must not walk the whole thread a page at a time on page load.
  let loaded: Comment[] = [comment({ id: 900 })];
  let attempt = nextAnchorAttempt("issue:LIF-9", "comment-1", loaded, true, false, null);

  for (let page = 1; page <= ANCHOR_AUTO_PAGE_BUDGET; page += 1) {
    expect(attempt?.pages).toBe(page);
    // The page lands, the comment is still not there, and the walk continues.
    loaded = [comment({ id: 900 - page }), ...loaded];
    attempt = nextAnchorAttempt("issue:LIF-9", "comment-1", loaded, true, false, attempt);
  }

  // Budget spent. Automatic loading stops even though older pages exist.
  expect(attempt).toBeNull();
  expect(ANCHOR_AUTO_SEARCH_LIMIT).toBe(ANCHOR_AUTO_PAGE_BUDGET * COMMENT_PAGE_SIZE);
});

test("a manual load older does not hand the automatic walk a fresh budget", () => {
  let loaded: Comment[] = [comment({ id: 900 })];
  let attempt = nextAnchorAttempt("issue:LIF-9", "comment-1", loaded, true, false, null);
  for (let page = 1; page <= ANCHOR_AUTO_PAGE_BUDGET; page += 1) {
    loaded = [comment({ id: 900 - page }), ...loaded];
    attempt = nextAnchorAttempt("issue:LIF-9", "comment-1", loaded, true, false, attempt);
  }
  expect(attempt).toBeNull();

  // The reader presses Load older by hand. The thread grows, which is exactly
  // the condition that used to release another request, but the budget for
  // this walk is spent and stays spent.
  const spent = { parent: "issue:LIF-9", target: "comment-1", loaded: loaded.length, pages: ANCHOR_AUTO_PAGE_BUDGET };
  for (let manual = 1; manual <= 3; manual += 1) {
    loaded = [comment({ id: 800 - manual }), ...loaded];
    expect(nextAnchorAttempt("issue:LIF-9", "comment-1", loaded, true, false, spent)).toBeNull();
  }
});

test("navigating to another thread starts the anchor walk over", () => {
  const spent = { parent: "issue:LIF-9", target: "comment-1", loaded: 2, pages: ANCHOR_AUTO_PAGE_BUDGET };
  // A different issue that happens to hold the same number of comments must
  // not inherit the abandoned walk's exhausted budget.
  const onScreen = [comment({ id: 900 }), comment({ id: 901 })];

  expect(nextAnchorAttempt("issue:LIF-10", "comment-1", onScreen, true, false, spent)).toEqual({
    parent: "issue:LIF-10",
    target: "comment-1",
    loaded: 2,
    pages: 1,
  });
  // And the page routes are keyed the same way.
  expect(nextAnchorAttempt("page:17", "comment-1", onScreen, true, false, spent)?.pages).toBe(1);
});

test("deletes a comment through its typed API call", async () => {
  let call: { url: string; init?: RequestInit } | undefined;
  globalThis.fetch = (async (url, init) => {
    call = { url: String(url), init };
    return new Response(JSON.stringify({ deleted: true }), { status: 200 });
  }) as typeof fetch;

  const result = await deleteComment(42);

  expect(result).toEqual({ ok: true, data: { deleted: true } });
  expect(call).toMatchObject({ url: "/api/comments/42", init: { method: "DELETE" } });
});

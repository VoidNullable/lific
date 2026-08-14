import { afterAll, afterEach, beforeAll, expect, test } from "bun:test";
import type { Component } from "svelte";
import { createServer, type ViteDevServer } from "vite";
import { fileURLToPath } from "node:url";
import type { Comment } from "../src/lib/api";
import {
  canManageComment,
  commentKeyboardAction,
  commentWasEdited,
  removeComment,
  replaceComment,
} from "../src/lib/commentState";

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

const { deleteComment, updateComment } = await import("../src/lib/api");

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

test("replaces and removes comments without disturbing their order", () => {
  const first = comment({ id: 1 });
  const second = comment({ id: 2 });
  const updated = comment({ id: 1, content: "Revised" });

  expect(replaceComment([first, second], updated)).toEqual([updated, second]);
  expect(removeComment([first, second], 1)).toEqual([second]);
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

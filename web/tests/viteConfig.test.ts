import { expect, test } from "bun:test";

import config from "../vite.config";

test("the dev server proxies private and public API routes to Rust", () => {
  const proxy = config.server?.proxy;
  expect(proxy).toBeDefined();
  expect(Object.keys(proxy ?? {})).toContain("/api");
  expect(Object.keys(proxy ?? {})).toContain("/public/api");
});

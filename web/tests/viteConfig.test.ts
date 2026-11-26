import { expect, test } from "bun:test";

import config from "../vite.config";

test("the dev server proxies the API to Rust", () => {
  const proxy = config.server?.proxy;
  expect(proxy).toBeDefined();
  expect(Object.keys(proxy ?? {})).toContain("/api");
  // LIF-583: the separate `/public/api` surface is gone.
  expect(Object.keys(proxy ?? {})).not.toContain("/public/api");
});

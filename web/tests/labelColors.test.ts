import { describe, expect, test } from "bun:test";
import {
  DEFAULT_LABEL_COLOR,
  safeLabelColor,
} from "../src/lib/labelColors";

describe("safeLabelColor", () => {
  test("preserves hex colors and rejects CSS source", () => {
    expect(safeLabelColor("#12aBcF")).toBe("#12aBcF");
    expect(safeLabelColor("red; background-image: url(https://example.test)"))
      .toBe(DEFAULT_LABEL_COLOR);
  });
});

import { describe, expect, test } from "bun:test";
import {
  claimMermaidBudget,
  createMermaidBudget,
  mermaidIsTooComplex,
} from "../src/lib/mermaidLimits";

describe("mermaidIsTooComplex", () => {
  test("accepts ordinary diagrams and rejects large or dense input", () => {
    expect(mermaidIsTooComplex("graph TD\nA-->B\nB-->C")).toBe(false);
    expect(mermaidIsTooComplex("x".repeat(4097))).toBe(true);
    expect(mermaidIsTooComplex(Array(129).fill("node").join("\n"))).toBe(true);
    expect(mermaidIsTooComplex(`graph TD\nA${"-->A".repeat(128)}`)).toBe(true);
  });

  test("rejects tiny inputs that trigger known Mermaid resource exhaustion", () => {
    expect(mermaidIsTooComplex("xychart\n  x-axis 1 --> 1\n  line [1, 2]")).toBe(true);
    expect(
      mermaidIsTooComplex("xychart\n  x-axis score 1 --> 1\n  line [1, 2]"),
    ).toBe(true);
    expect(
      mermaidIsTooComplex(
        "radar-beta\n  axis a, b\n  curve c {1,1}\n  ticks 1000000000",
      ),
    ).toBe(true);
  });

  test("rejects Mermaid architecture prototype pollution keys", () => {
    expect(
      mermaidIsTooComplex(
        "architecture-beta\n  group __proto__(cloud)[Attacker controlled]",
      ),
    ).toBe(true);
  });
});

describe("claimMermaidBudget", () => {
  test("shares aggregate block and source limits", () => {
    const budget = createMermaidBudget();

    expect(claimMermaidBudget(4096, budget)).toBeUndefined();
    expect(claimMermaidBudget(4096, budget)).toBeUndefined();
    expect(claimMermaidBudget(1, budget)).toBe("blocks");
    expect(budget).toEqual({ blocks: 2, sourceBytes: 8192 });
  });

  test("does not consume rejected source", () => {
    const budget = createMermaidBudget();

    expect(claimMermaidBudget(8193, budget)).toBe("bytes");
    expect(budget).toEqual({ blocks: 0, sourceBytes: 0 });
  });
});

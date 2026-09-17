import { describe, expect, test } from "bun:test";
import mermaid from "mermaid";
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

  test.each([
    "xychart; x-axis 1 --> 1; line [1, 2]",
    "xychart\nx-axis 1 --> 10\nx-axis 2 --> 2\nline [1, 2]",
    "radar-beta\naxis a, b\ncurve c {1,1}\nticks 5\nticks 1000000000",
    "radar-beta\naxis a, b\ncurve c {1,1}\nticks 1000000000 %% comment",
  ])("rejects dangerous directives in valid Mermaid syntax: %s", async (source) => {
    // Parse only: a regression must never render a potentially hostile input.
    await expect(mermaid.parse(source)).resolves.toBeTruthy();
    expect(mermaidIsTooComplex(source)).toBe(true);
  });

  test("preserves safe ranges and quoted labels containing directive-like text", () => {
    for (const source of [
      "xychart; x-axis 1 --> 10; line [1, 2]",
      'xychart\ntitle "Example; x-axis 1 --> 1"\nx-axis 1 --> 10\nline [1, 2]',
      'xychart\nx-axis "Revenue; %% total" 1 --> 10\nline [1, 2]',
      "radar-beta\naxis a, b\ncurve c {1,1}\nticks 128 %% allowed",
      'architecture-beta\ngroup safe(cloud)[Safe]',
    ]) {
      expect(mermaidIsTooComplex(source)).toBe(false);
    }
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

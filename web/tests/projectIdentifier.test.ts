import { describe, expect, it } from "bun:test";
import { cleanIdentifier, identifierProblem, suggestIdentifier } from "../src/lib/projectIdentifier";

describe("suggestIdentifier", () => {
  it("takes the first five letters and digits of the name", () => {
    expect(suggestIdentifier("Moon Base Alpha")).toBe("MOONB");
    expect(suggestIdentifier("Is It DNS?")).toBe("ISITD");
  });

  it("never starts with a digit", () => {
    expect(suggestIdentifier("3D Printer")).toBe("DPRIN");
    expect(suggestIdentifier("2048")).toBe("");
  });

  it("swaps the tail for a digit when the suggestion is taken", () => {
    expect(suggestIdentifier("Lif", new Set(["LIF"]))).toBe("LIF2");
    expect(suggestIdentifier("Moon Base", new Set(["MOONB", "MOON2"]))).toBe("MOON3");
  });

  it("steps around the reserved DOC", () => {
    expect(suggestIdentifier("Doc")).toBe("DOC2");
  });
});

describe("cleanIdentifier", () => {
  it("uppercases, drops other characters and caps at five", () => {
    expect(cleanIdentifier("ab-c d12345")).toBe("ABCD1");
  });
});

describe("identifierProblem", () => {
  const taken = new Map([["LIF", "Lific"]]);

  it("accepts a free, well-formed identifier", () => {
    expect(identifierProblem("PRO2", taken)).toBeNull();
    expect(identifierProblem("")).toBeNull();
  });

  it("names what is wrong", () => {
    expect(identifierProblem("2PRO")).toBe("Start with a letter");
    expect(identifierProblem("DOC")).toBe("DOC is reserved for page links");
    expect(identifierProblem("LIF", taken)).toBe("Already used by Lific");
  });
});

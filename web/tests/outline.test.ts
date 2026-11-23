import { describe, expect, test } from "bun:test";
import { outlineOf, readingMinutes, tableOfContents, wordCount } from "../src/lib/detail/outline";

describe("outlineOf", () => {
  test("finds ATX and setext headings with their lines, skipping code", () => {
    const md = [
      "Intro",
      "",
      "## Read *model*",
      "text",
      "```",
      "# not a heading",
      "```",
      "",
      "Setext title",
      "------------",
      "### [Linked](https://x) ###",
    ].join("\n");
    expect(outlineOf(md)).toEqual([
      { level: 2, text: "Read model", line: 3, index: 0 },
      { level: 2, text: "Setext title", line: 9, index: 1 },
      { level: 3, text: "Linked", line: 11, index: 2 },
    ]);
  });

  test("a dash rule under a list or after text in a paragraph is not a heading", () => {
    expect(outlineOf("- item\n---")).toEqual([]);
    expect(outlineOf("one\ntwo\n---")).toEqual([]);
  });

  test("an empty ATX heading is skipped", () => {
    expect(outlineOf("##\n# Real")).toEqual([{ level: 1, text: "Real", line: 2, index: 0 }]);
  });
});

describe("tableOfContents", () => {
  test("needs a few headings, and keeps three levels below the top one", () => {
    const h = (level: number, index: number) => ({ level, text: `h${index}`, line: index, index });
    expect(tableOfContents([h(2, 0), h(2, 1)])).toEqual([]);
    const all = [h(2, 0), h(3, 1), h(4, 2), h(5, 3)];
    expect(tableOfContents(all).map((x) => x.level)).toEqual([2, 3, 4]);
  });
});

describe("length", () => {
  test("counts words outside code", () => {
    expect(wordCount("Hello **world**, it's [a link](http://x.y/z).\n```\ncode here\n```")).toBe(5);
    expect(readingMinutes(10)).toBe(1);
    expect(readingMinutes(1150)).toBe(5);
  });
});

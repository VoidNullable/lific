import { expect, test } from "bun:test";
import { insertSnippetAt, markdownFor } from "../src/lib/attachments/compose";
import type { UploadResponse } from "../src/lib/api";

const SNIPPET = "![shot.png](/api/attachments/7)";

function upload(over: Partial<UploadResponse> = {}): UploadResponse {
  return {
    id: 7,
    url: "/api/attachments/7",
    filename: "shot.png",
    mime: "image/png",
    size: 1234,
    ...over,
  };
}

test("images embed, everything else links", () => {
  expect(markdownFor(upload())).toBe("![shot.png](/api/attachments/7)");
  expect(markdownFor(upload({ filename: "trace.log", mime: "text/plain" }))).toBe(
    "[trace.log](/api/attachments/7)",
  );
});

test("an image's alt text stands in for its filename", () => {
  expect(markdownFor(upload(), "The [login] dialog\nafter")).toBe(
    "![The login dialog after](/api/attachments/7)",
  );
  expect(markdownFor(upload(), "  ")).toBe("![shot.png](/api/attachments/7)");
  expect(markdownFor(upload({ filename: "a.pdf", mime: "application/pdf" }), "ignored")).toBe(
    "[a.pdf](/api/attachments/7)",
  );
});

test("inserting into an empty composer adds no leading break", () => {
  const { text, caret } = insertSnippetAt("", 0, 0, SNIPPET);
  expect(text).toBe(`${SNIPPET}\n`);
  expect(caret).toBe(SNIPPET.length + 1);
});

test("an image inserted mid-line becomes its own paragraph", () => {
  const current = "see this";
  const { text, caret } = insertSnippetAt(current, 8, 8, SNIPPET);
  expect(text).toBe(`see this\n\n${SNIPPET}\n`);
  expect(caret).toBe(text.length);
});

test("an image under a line of text is separated from it by a blank line", () => {
  const current = "intro\n";
  const { text, caret } = insertSnippetAt(current, 6, 6, SNIPPET);
  expect(text).toBe(`intro\n\n${SNIPPET}\n`);
  expect(caret).toBe(text.length);
});

test("inserting in the middle keeps the tail and lands the caret before it", () => {
  const current = "before\nafter";
  const { text, caret } = insertSnippetAt(current, 7, 7, SNIPPET);
  expect(text).toBe(`before\n\n${SNIPPET}\n\nafter`);
  expect(caret).toBe(`before\n\n${SNIPPET}\n`.length);
  expect(text.slice(caret)).toBe("\nafter");
});

test("a selection is replaced, not pushed aside", () => {
  const current = "keep DROP keep";
  const { text, caret } = insertSnippetAt(current, 5, 9, SNIPPET);
  expect(text).toBe(`keep \n\n${SNIPPET}\n\n keep`);
  expect(text.slice(caret)).toBe("\n keep");
});

test("a backwards selection is normalised", () => {
  const forward = insertSnippetAt("keep DROP keep", 5, 9, SNIPPET);
  const backward = insertSnippetAt("keep DROP keep", 9, 5, SNIPPET);
  expect(backward).toEqual(forward);
});

test("out-of-range offsets clamp to the text instead of throwing", () => {
  const current = "short";
  const { text, caret } = insertSnippetAt(current, 999, 999, SNIPPET);
  expect(text).toBe(`short\n\n${SNIPPET}\n`);
  expect(caret).toBe(text.length);

  const negative = insertSnippetAt(current, -4, -4, SNIPPET);
  expect(negative.text).toBe(`${SNIPPET}\n\nshort`);
  expect(negative.caret).toBe(SNIPPET.length + 1);
});

test("consecutive inserts stack one per line", () => {
  const first = insertSnippetAt("", 0, 0, "![a](/api/attachments/1)");
  const second = insertSnippetAt(
    first.text,
    first.caret,
    first.caret,
    "![b](/api/attachments/2)",
  );
  expect(second.text).toBe("![a](/api/attachments/1)\n![b](/api/attachments/2)\n");
  expect(second.caret).toBe(second.text.length);
});

test("a file link keeps to its own line without paragraph breaks", () => {
  const link = "[trace.log](/api/attachments/8)";
  expect(insertSnippetAt("see this", 8, 8, link).text).toBe(`see this\n${link}\n`);
  expect(insertSnippetAt("before\nafter", 7, 7, link).text).toBe(`before\n${link}\nafter`);
});

test("a multi-image drop under text lands as one paragraph of images", () => {
  let state = { text: "Screenshots:\nmore text", caret: "Screenshots:".length };
  for (const n of [1, 2, 3]) {
    state = insertSnippetAt(state.text, state.caret, state.caret, `![${n}](/api/attachments/${n})`);
  }
  expect(state.text).toBe(
    "Screenshots:\n\n![1](/api/attachments/1)\n![2](/api/attachments/2)\n![3](/api/attachments/3)\n\nmore text",
  );
});

test("an image placed next to other images joins their paragraph", () => {
  const current = "![a](/api/attachments/1)\n";
  expect(insertSnippetAt(current, current.length, current.length, SNIPPET).text).toBe(
    `![a](/api/attachments/1)\n${SNIPPET}\n`,
  );
});

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

test("inserting into an empty composer adds no leading break", () => {
  const { text, caret } = insertSnippetAt("", 0, 0, SNIPPET);
  expect(text).toBe(`${SNIPPET}\n`);
  expect(caret).toBe(SNIPPET.length + 1);
});

test("inserting mid-line breaks onto its own block", () => {
  const current = "see this";
  const { text, caret } = insertSnippetAt(current, 8, 8, SNIPPET);
  expect(text).toBe(`see this\n${SNIPPET}\n`);
  expect(caret).toBe(text.length);
});

test("inserting at the start of a fresh line keeps it inline", () => {
  const current = "intro\n";
  const { text, caret } = insertSnippetAt(current, 6, 6, SNIPPET);
  expect(text).toBe(`intro\n${SNIPPET}\n`);
  expect(caret).toBe(text.length);
});

test("inserting in the middle keeps the tail and lands the caret before it", () => {
  const current = "before\nafter";
  const { text, caret } = insertSnippetAt(current, 7, 7, SNIPPET);
  expect(text).toBe(`before\n${SNIPPET}\nafter`);
  expect(caret).toBe(`before\n${SNIPPET}\n`.length);
  expect(text.slice(caret)).toBe("after");
});

test("a selection is replaced, not pushed aside", () => {
  const current = "keep DROP keep";
  const { text, caret } = insertSnippetAt(current, 5, 9, SNIPPET);
  expect(text).toBe(`keep \n${SNIPPET}\n keep`);
  expect(text.slice(caret)).toBe(" keep");
});

test("a backwards selection is normalised", () => {
  const forward = insertSnippetAt("keep DROP keep", 5, 9, SNIPPET);
  const backward = insertSnippetAt("keep DROP keep", 9, 5, SNIPPET);
  expect(backward).toEqual(forward);
});

test("out-of-range offsets clamp to the text instead of throwing", () => {
  const current = "short";
  const { text, caret } = insertSnippetAt(current, 999, 999, SNIPPET);
  expect(text).toBe(`short\n${SNIPPET}\n`);
  expect(caret).toBe(text.length);

  const negative = insertSnippetAt(current, -4, -4, SNIPPET);
  expect(negative.text).toBe(`${SNIPPET}\nshort`);
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

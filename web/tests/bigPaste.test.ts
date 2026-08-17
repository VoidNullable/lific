import { expect, test } from "bun:test";
import {
  BIG_PASTE_CHAR_LIMIT,
  BIG_PASTE_LINE_LIMIT,
  countLines,
  describePaste,
  isBigPaste,
  pasteFileFrom,
  pasteFilename,
} from "../src/lib/attachments/bigPaste";

function lines(n: number): string {
  return Array.from({ length: n }, (_, i) => `line ${i + 1}`).join("\n");
}

test("counts lines without inventing one for empty text", () => {
  expect(countLines("")).toBe(0);
  expect(countLines("one")).toBe(1);
  expect(countLines("one\ntwo")).toBe(2);
  expect(countLines("trailing\n")).toBe(2);
});

test("the line threshold is strict at 60", () => {
  expect(isBigPaste(lines(BIG_PASTE_LINE_LIMIT))).toBe(false);
  expect(isBigPaste(lines(BIG_PASTE_LINE_LIMIT + 1))).toBe(true);
});

test("the character threshold is strict at 6000", () => {
  expect(isBigPaste("x".repeat(BIG_PASTE_CHAR_LIMIT))).toBe(false);
  expect(isBigPaste("x".repeat(BIG_PASTE_CHAR_LIMIT + 1))).toBe(true);
});

test("ordinary pastes are left alone", () => {
  expect(isBigPaste("")).toBe(false);
  expect(isBigPaste("a quick note")).toBe(false);
  expect(isBigPaste(lines(12))).toBe(false);
});

test("generates a local-time paste-YYYYMMDD-HHMM.txt filename", () => {
  expect(pasteFilename(new Date(2026, 7, 17, 9, 4))).toBe("paste-20260817-0904.txt");
  expect(pasteFilename(new Date(2026, 11, 1, 23, 59))).toBe("paste-20261201-2359.txt");
  expect(pasteFilename(new Date(2026, 0, 5, 0, 0))).toBe("paste-20260105-0000.txt");
});

test("wraps the pasted text as a text/plain file", async () => {
  const at = new Date(2026, 7, 17, 13, 30);
  const file = pasteFileFrom("hello\nworld", at);
  expect(file.name).toBe("paste-20260817-1330.txt");
  // Bun's File appends a charset to the declared type; browsers do not.
  expect(file.type.startsWith("text/plain")).toBe(true);
  expect(await file.text()).toBe("hello\nworld");
});

test("describes the paste by whichever threshold it tripped", () => {
  expect(describePaste(lines(412))).toBe("412 lines");
  expect(describePaste("x".repeat(8204))).toBe("8,204 characters");
});

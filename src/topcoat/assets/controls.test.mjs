import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import test from "node:test";

const stylesheet = readFileSync(new URL("./controls.css", import.meta.url), "utf8");

test("modal sheets fit the viewport and return to a centered dialog on wide screens", () => {
  assert.match(stylesheet, /\.tc-sheet\s*\{[^}]*max-height:\s*min\(85dvh,/s);
  assert.match(stylesheet, /\.tc-sheet\s*\{[^}]*width:\s*100%/s);
  assert.match(stylesheet, /@media\s*\(min-width:\s*40rem\)\s*\{\s*\.tc-sheet/s);
  assert.match(stylesheet, /max-width:\s*34rem/);
});

test("shared controls retain visible keyboard focus and reduced-motion support", () => {
  assert.match(stylesheet, /:focus-visible/);
  assert.match(stylesheet, /prefers-reduced-motion:\s*reduce/);
  assert.match(stylesheet, /\.tc-tooltip:is\(:hover, :focus-within\)/);
});

test("native popovers and toast controls remain scrollable and legible", () => {
  assert.match(stylesheet, /\.tc-popover\s*\{[^}]*max-height:\s*calc\(100dvh/s);
  assert.match(stylesheet, /\.tc-popover\s*\{[^}]*overflow:\s*auto/s);
  assert.match(stylesheet, /\.tc-toast p\s*\{[^}]*overflow-wrap:\s*anywhere/s);
});

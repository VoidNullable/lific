import { describe, expect, test } from "bun:test";
import {
  ansiLineToSpans,
  ansiStyleToCss,
  ansiToSpans,
  hasAnsi,
  stripAnsi,
} from "../src/lib/attachments/viewers/ansi";

const ESC = "\u001b";

describe("ansi to spans", () => {
  test("plain text becomes one unstyled span", () => {
    const { spans } = ansiLineToSpans("cargo test");
    expect(spans).toEqual([{ text: "cargo test", style: {} }]);
  });

  test("colors the run between an SGR code and its reset", () => {
    const { spans } = ansiLineToSpans(`ok ${ESC}[32mpassed${ESC}[0m done`);
    expect(spans.map((s) => s.text)).toEqual(["ok ", "passed", " done"]);
    expect(spans[1].style.fg).toBe("var(--ansi-green)");
    expect(spans[2].style.fg).toBeUndefined();
  });

  test("combines attributes and clears them individually", () => {
    const { spans } = ansiLineToSpans(`${ESC}[1;4;31mloud${ESC}[24mquieter`);
    expect(spans[0].style).toMatchObject({
      bold: true,
      underline: true,
      fg: "var(--ansi-red)",
    });
    expect(spans[1].style).toMatchObject({ bold: true, underline: false });
  });

  test("reads bright, 256-color and truecolor forms", () => {
    expect(ansiLineToSpans(`${ESC}[91mx`).spans[0].style.fg).toBe("var(--ansi-bright-red)");
    expect(ansiLineToSpans(`${ESC}[38;5;33mx`).spans[0].style.fg).toBe("#0087ff");
    expect(ansiLineToSpans(`${ESC}[38;5;250mx`).spans[0].style.fg).toBe("#bcbcbc");
    expect(ansiLineToSpans(`${ESC}[38;2;18;52;86mx`).spans[0].style.fg).toBe("#123456");
    expect(ansiLineToSpans(`${ESC}[48;5;1mx`).spans[0].style.bg).toBe("var(--ansi-red)");
  });

  test("an empty parameter list is a reset", () => {
    const { spans } = ansiLineToSpans(`${ESC}[31mred${ESC}[mplain`);
    expect(spans[1].style.fg).toBeUndefined();
  });

  test("strips sequences it does not support instead of printing them", () => {
    const { spans } = ansiLineToSpans(
      `${ESC}[2K${ESC}[1Gprogress${ESC}]0;window title${ESC}\\ done`,
    );
    expect(spans.map((s) => s.text).join("")).toBe("progress done");
  });

  test("carries style across a line boundary", () => {
    const lines = ansiToSpans(`${ESC}[33mfirst\nsecond${ESC}[0m\nthird`);
    expect(lines).toHaveLength(3);
    expect(lines[0][0].style.fg).toBe("var(--ansi-yellow)");
    expect(lines[1][0].style.fg).toBe("var(--ansi-yellow)");
    expect(lines[2][0].style.fg).toBeUndefined();
  });

  test("a line of pure escapes still yields one empty span", () => {
    const { spans } = ansiLineToSpans(`${ESC}[0m`);
    expect(spans).toEqual([{ text: "", style: {} }]);
  });

  test("merges adjacent runs that share a style", () => {
    const { spans } = ansiLineToSpans(`a${ESC}[32mb${ESC}[32mc`);
    expect(spans).toHaveLength(2);
    expect(spans[1].text).toBe("bc");
  });
});

describe("ansi helpers", () => {
  test("detects and removes escapes", () => {
    expect(hasAnsi("plain")).toBe(false);
    expect(hasAnsi(`${ESC}[31mred`)).toBe(true);
    expect(stripAnsi(`${ESC}[1;31merror${ESC}[0m: boom`)).toBe("error: boom");
  });

  test("maps style to css, swapping colors when inverted", () => {
    expect(ansiStyleToCss({ fg: "#ff0000", bold: true })).toBe(
      "color:#ff0000;font-weight:600",
    );
    expect(ansiStyleToCss({ fg: "#ff0000", bg: "#000000", inverse: true })).toBe(
      "color:#000000;background:#ff0000",
    );
    expect(ansiStyleToCss({ underline: true, strike: true })).toBe(
      "text-decoration:underline line-through",
    );
    expect(ansiStyleToCss({})).toBe("");
  });
});

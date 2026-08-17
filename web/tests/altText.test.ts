import { describe, expect, test } from "bun:test";
import {
  applyAltText,
  findAltSpan,
  sanitizeAltText,
} from "../src/lib/attachments/altText";

describe("sanitizeAltText", () => {
  test("folds newlines and collapses runs of whitespace", () => {
    expect(sanitizeAltText("  the crash\n  dialog  ")).toBe("the crash dialog");
  });

  test("strips brackets that would terminate the alt early", () => {
    expect(sanitizeAltText("panel [left] side")).toBe("panel left side");
  });

  test("reduces a whitespace-only input to nothing", () => {
    expect(sanitizeAltText("  \n\t ")).toBe("");
  });
});

describe("findAltSpan", () => {
  const body = "intro\n![shot.png](/api/attachments/9)\noutro";

  test("locates the alt of the reference at the insertion point", () => {
    const span = findAltSpan(body, body.indexOf("!["))!;
    expect(body.slice(span.start, span.end)).toBe("shot.png");
  });

  test("ignores a bracket pair that is not an image reference", () => {
    const text = "![not a link] then ![real.png](/api/attachments/1)";
    const span = findAltSpan(text, 0)!;
    expect(text.slice(span.start, span.end)).toBe("real.png");
  });

  test("falls back to searching from the start when the offset is past it", () => {
    const span = findAltSpan(body, body.length)!;
    expect(body.slice(span.start, span.end)).toBe("shot.png");
  });

  test("returns null when there is no image reference", () => {
    expect(findAltSpan("just [a link](/x) here", 0)).toBeNull();
  });
});

describe("applyAltText", () => {
  test("replaces the filename placeholder with the description", () => {
    const body = "Here it is:\n![shot.png](/api/attachments/9)\n";
    expect(applyAltText(body, body.indexOf("!["), "The crash dialog")).toBe(
      "Here it is:\n![The crash dialog](/api/attachments/9)\n",
    );
  });

  test("rewrites the reference at the offset, not an earlier one", () => {
    const body = "![first.png](/api/attachments/1)\n![second.png](/api/attachments/2)";
    const offset = body.indexOf("![second");
    expect(applyAltText(body, offset, "Second shot")).toBe(
      "![first.png](/api/attachments/1)\n![Second shot](/api/attachments/2)",
    );
  });

  test("fills an empty alt slot", () => {
    expect(applyAltText("![](/api/attachments/3)", 0, "Board view")).toBe(
      "![Board view](/api/attachments/3)",
    );
  });

  test("treats a blank description as a skip", () => {
    const body = "![shot.png](/api/attachments/9)";
    expect(applyAltText(body, 0, "   ")).toBe(body);
  });

  test("leaves the document alone when the reference is gone", () => {
    expect(applyAltText("nothing to see", 4, "Anything")).toBe("nothing to see");
  });

  test("sanitizes on the way in so the reference cannot be broken", () => {
    expect(applyAltText("![a.png](/x)", 0, "two\nlines [and] brackets")).toBe(
      "![two lines and brackets](/x)",
    );
  });

  test("tolerates an out-of-range offset", () => {
    const body = "![a.png](/x)";
    expect(applyAltText(body, 9999, "Fine")).toBe("![Fine](/x)");
    expect(applyAltText(body, -12, "Fine")).toBe("![Fine](/x)");
  });
});

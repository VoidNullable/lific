import { describe, expect, test } from "bun:test";
import { imageUrlFromPaste, replacePastedUrl } from "../src/lib/attachments/imageUrl";

describe("image URL paste", () => {
  test("recognizes only single HTTP image URLs, including query strings", () => {
    for (const url of ["https://example.com/a.PNG?download=1#image", "http://example.com/a.webp"]) {
      expect(imageUrlFromPaste(` ${url}\n`)).toBe(url);
    }
    for (const text of ["https://example.com/", "words https://example.com/a.png", "https://example.com/a.png https://example.com/b.png",
      "https://example.com/a.svg", "https://example.com/a.SVG?download=1",
      "data:image/png;base64,aaaa", "file:///a.png", "https://u:p@example.com/a.png", "https://example.com/page?name=a.png"]) {
      expect(imageUrlFromPaste(text)).toBeNull();
    }
  });
  test("retains text edited during import and does not resurrect a deleted URL", () => {
    const url = "https://example.com/a.png";
    const md = "![a.png](/api/attachments/1)";
    expect(replacePastedUrl(`before ${url} after`, url, 7, md)?.text).toBe(`before ${md} after`);
    expect(replacePastedUrl(`added before ${url} after`, url, 7, md)?.text).toBe(`added before ${md} after`);
    expect(replacePastedUrl("removed", url, 7, md)).toBeNull();
    expect(replacePastedUrl(`${url} and ${url}`, url, 7, md)).toBeNull();
  });
});

import { describe, expect, test } from "bun:test";
import { marked } from "marked";
import { splitImageAlt, joinImageAlt, withImageWidths } from "../src/lib/imageSize";
import { applyAltText, applyImageWidth } from "../src/lib/attachments/altText";

function render(markdown: string): string {
  const renderer = new marked.Renderer();
  renderer.image = withImageWidths(renderer.image.bind(renderer));
  return marked.parse(markdown, { breaks: true, gfm: true, renderer }) as string;
}

function renderDefault(markdown: string): string {
  return marked.parse(markdown, { breaks: true, gfm: true }) as string;
}

describe("splitImageAlt", () => {
  test("reads a trailing pipe and number as the width", () => {
    expect(splitImageAlt("login screen|320")).toEqual({ alt: "login screen", width: 320 });
    expect(splitImageAlt("login screen | 320 ")).toEqual({ alt: "login screen", width: 320 });
    expect(splitImageAlt("|240")).toEqual({ alt: "", width: 240 });
  });

  test("leaves alt text without a width alone", () => {
    expect(splitImageAlt("a | b")).toEqual({ alt: "a | b", width: null });
    expect(splitImageAlt("v2|beta")).toEqual({ alt: "v2|beta", width: null });
    expect(splitImageAlt("plain")).toEqual({ alt: "plain", width: null });
  });

  test("an out-of-range width stays part of the alt", () => {
    expect(splitImageAlt("tiny|8")).toEqual({ alt: "tiny|8", width: null });
    expect(splitImageAlt("huge|99999")).toEqual({ alt: "huge|99999", width: null });
  });

  test("joins back to the written form", () => {
    expect(joinImageAlt("shot", 320)).toBe("shot|320");
    expect(joinImageAlt("shot", null)).toBe("shot");
  });
});

describe("image renderer", () => {
  test("a width suffix becomes a width attribute and leaves the alt", () => {
    const html = render("![login screen|320](/api/attachments/4)");
    expect(html).toContain('<img width="320" src="/api/attachments/4" alt="login screen">');
  });

  test("an image without a width renders exactly as marked's default", () => {
    for (const md of [
      "![shot](/api/attachments/4)",
      '![shot](/api/attachments/4 "Caption")',
      "![a | b](x.png)",
      "text ![*em*](x.png) more",
    ]) {
      expect(render(md)).toBe(renderDefault(md));
    }
  });

  test("the title is kept beside the width", () => {
    const html = render('![shot|240](/api/attachments/4 "Login")');
    expect(html).toContain('<img width="240" src="/api/attachments/4" alt="shot" title="Login">');
  });

  test("alt text is escaped", () => {
    const html = render('![a "quoted" <b>|240](x.png)');
    expect(html).toContain('alt="a &quot;quoted&quot; &lt;b&gt;"');
  });

  test("an escaped pipe inside a table cell still sets the width", () => {
    const html = render("| a |\n| - |\n| ![shot\\|240](x.png) |");
    expect(html).toContain('<img width="240" src="x.png" alt="shot">');
  });
});

describe("rewriting the width in a draft", () => {
  const draft = "intro\n![shot.png](/api/attachments/9)\noutro";

  test("sets, changes and removes the width", () => {
    const small = applyImageWidth(draft, 6, 240);
    expect(small).toBe("intro\n![shot.png|240](/api/attachments/9)\noutro");
    expect(applyImageWidth(small, 6, 480)).toBe(
      "intro\n![shot.png|480](/api/attachments/9)\noutro",
    );
    expect(applyImageWidth(small, 6, null)).toBe(draft);
  });

  test("alt text applied after a size keeps the size", () => {
    const small = applyImageWidth(draft, 6, 240);
    expect(applyAltText(small, 6, "the login dialog")).toBe(
      "intro\n![the login dialog|240](/api/attachments/9)\noutro",
    );
  });

  test("a draft without an image is unchanged", () => {
    expect(applyImageWidth("no image here", 0, 240)).toBe("no image here");
  });
});

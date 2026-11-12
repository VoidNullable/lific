import { describe, expect, test } from "bun:test";
import { marked } from "marked";
import {
  imageOnlyParagraph,
  joinImageAlt,
  splitImageAlt,
  withImageParagraphs,
  withImageWidths,
} from "../src/lib/imageSize";
import { applyAltText, applyImageWidth } from "../src/lib/attachments/altText";

function render(markdown: string): string {
  const renderer = new marked.Renderer();
  renderer.image = withImageWidths(renderer.image.bind(renderer));
  renderer.paragraph = withImageParagraphs(renderer.paragraph.bind(renderer), () => renderer.parser);
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

  test("an image without a width or caption renders exactly as marked's default", () => {
    for (const md of [
      "![shot](/api/attachments/4)",
      'text ![shot](/api/attachments/4 "Inline title") more',
      "![a | b](x.png)",
      "text ![*em*](x.png) more",
    ]) {
      expect(render(md)).toBe(renderDefault(md));
    }
  });

  test("the title is kept beside the width", () => {
    const html = render('text ![shot|240](/api/attachments/4 "Login")');
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

describe("captions", () => {
  test("a lone titled image renders as a figure with the title as caption", () => {
    expect(render('![a](/api/attachments/1 "Login screen")')).toBe(
      '<figure class="md-figure"><img src="/api/attachments/1" alt="a" title="Login screen"><figcaption>Login screen</figcaption></figure>\n',
    );
  });

  test("a caption keeps the image's width", () => {
    expect(render('![a|240](/api/attachments/1 "Small")')).toContain(
      '<figure class="md-figure"><img width="240" src="/api/attachments/1" alt="a" title="Small"><figcaption>Small</figcaption></figure>',
    );
  });

  test("the caption is escaped", () => {
    expect(render('![a](x.png "<b>bold</b> & co")')).toContain(
      "<figcaption>&lt;b&gt;bold&lt;/b&gt; &amp; co</figcaption>",
    );
  });

  test("an image with text around it keeps its title as a tooltip only", () => {
    const md = 'See ![a](x.png "Login") here';
    expect(render(md)).toBe(renderDefault(md));
  });

  test("paragraphs made only of images are recognised", () => {
    const tokens = (md: string) => (marked.lexer(md, { breaks: true, gfm: true })[0] as any).tokens;
    expect(imageOnlyParagraph(tokens("![a](x.png)\n![b](y.png)"))?.length).toBe(2);
    expect(imageOnlyParagraph(tokens("![a](x.png) ![b](y.png)"))?.length).toBe(2);
    expect(imageOnlyParagraph(tokens("![a](x.png) and text"))).toBeNull();
    expect(imageOnlyParagraph(tokens("just text"))).toBeNull();
  });
});

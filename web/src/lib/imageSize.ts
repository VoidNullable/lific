// Image width in Markdown: `![alt|320](url)` renders the image at most 320
// CSS pixels wide.
//
// The width rides in the alt text (the Obsidian convention) rather than in an
// attribute list or raw HTML, so every other Markdown renderer and every
// export still shows a normal image; the only trace there is "|320" in the
// alt. Lific strips the suffix from the rendered alt and turns it into a
// `width` attribute, which the sanitizer keeps in both signed-in and public
// scope. Inside a GFM table the pipe has to be escaped (`![alt\|320](url)`),
// as any pipe in a cell does.

import type { Token, Tokens } from "marked";

/** Widths outside this range are read as ordinary alt text. */
export const MIN_IMAGE_WIDTH = 16;
export const MAX_IMAGE_WIDTH = 4096;

/** Long edge of the server's thumbnails. An image asked to render wider than
 *  this loads the original instead, or it would be shown blurry. */
export const THUMBNAIL_EDGE = 480;

/** The sizes the upload chip offers. Without one an image renders as before:
 *  the thumbnail at its natural size, within the column. */
export const IMAGE_SIZE_PRESETS: { label: string; title: string; width: number }[] = [
  { label: "S", title: "Small: at most 240 px wide", width: 240 },
  { label: "M", title: "Medium: at most 480 px wide", width: 480 },
  { label: "L", title: "Large: up to the full column, from the original", width: 960 },
];

const WIDTH_SUFFIX_RE = /\s*\|\s*(\d{1,5})\s*$/;

/** Split `alt|320` into the alt text and the width. A suffix that is not a
 *  width in range leaves the alt untouched and the width null. */
export function splitImageAlt(raw: string): { alt: string; width: number | null } {
  const m = raw.match(WIDTH_SUFFIX_RE);
  if (!m) return { alt: raw, width: null };
  const width = Number(m[1]);
  if (width < MIN_IMAGE_WIDTH || width > MAX_IMAGE_WIDTH) return { alt: raw, width: null };
  return { alt: raw.slice(0, m.index), width };
}

/** Join alt text and an optional width back into the alt slot. */
export function joinImageAlt(alt: string, width: number | null): string {
  return width === null ? alt : `${alt}|${width}`;
}

/**
 * Wrap marked's image renderer so a width suffix becomes a `width` attribute.
 * Images without one go through the original renderer untouched, so their
 * markup is byte-identical to marked's default.
 */
export function withImageWidths(
  original: (token: Tokens.Image) => string,
): (token: Tokens.Image) => string {
  return function (this: unknown, token: Tokens.Image): string {
    const { alt, width } = splitImageAlt(token.text);
    if (width === null) return original.call(this, token);
    // marked builds the alt from the inline tokens, so replace them with one
    // plain text token that no longer carries the suffix.
    const html = original.call(this, {
      ...token,
      text: alt,
      tokens: [{ type: "text", raw: alt, text: alt }],
    });
    if (!html.startsWith("<img ")) return html;
    return `<img width="${width}" ${html.slice(5)}`;
  };
}

function escapeHtml(text: string): string {
  return text
    .replace(/&/g, "&amp;")
    .replace(/</g, "&lt;")
    .replace(/>/g, "&gt;")
    .replace(/"/g, "&quot;")
    .replace(/'/g, "&#39;");
}

/** The images of a paragraph that holds nothing else (line breaks and
 *  whitespace aside), or null when it has any other content. */
export function imageOnlyParagraph(tokens: Token[]): Tokens.Image[] | null {
  const images: Tokens.Image[] = [];
  for (const token of tokens) {
    if (token.type === "image") images.push(token as Tokens.Image);
    else if (token.type === "br") continue;
    else if (token.type === "text" && token.raw.trim() === "") continue;
    else return null;
  }
  return images.length > 0 ? images : null;
}

/**
 * Route paragraphs made only of images through image layout. A lone image
 * with a title (`![alt](url "Caption")`) renders as a figure whose caption is
 * the title. Every other paragraph renders exactly as marked's default.
 */
export function withImageParagraphs(
  original: (token: Tokens.Paragraph) => string,
  parser: () => { parseInline: (tokens: Token[]) => string },
): (token: Tokens.Paragraph) => string {
  return function (this: unknown, token: Tokens.Paragraph): string {
    const images = imageOnlyParagraph(token.tokens);
    if (images?.length === 1 && images[0].title) {
      const img = parser().parseInline([images[0]]);
      return `<figure class="md-figure">${img}<figcaption>${escapeHtml(images[0].title)}</figcaption></figure>\n`;
    }
    return original.call(this, token);
  };
}

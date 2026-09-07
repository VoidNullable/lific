// LIF-465: the sanitizer behind the public issue view.
//
// Markdown bodies may contain raw HTML, so the rendered HTML is parsed and a
// NEW tree is built from it against an allowlist. Two rules carry the whole
// argument:
//
//   1. tags are an allowlist. Anything not on it is dropped with its subtree
//      (if it can fetch or execute) or unwrapped to its children;
//   2. no attribute is ever copied. Every output element is created fresh and
//      given only attributes this file writes, so `style`, `srcset`, `poster`,
//      `background`, `onerror` and anything unforeseen are excluded by one
//      rule rather than by a list that has to stay current.
//
// The only surviving URLs are rebuilt here: an attachment id the server
// allowlisted, an issue in the same published project, or an external
// http(s)/mailto link. That is what stops a public body being a tracking pixel
// against its readers.
//
// DOMPurify still runs at the end as an independent second mechanism. It is
// not load-bearing for the fetch rules above: a `<video src>` pointing at
// someone else's server is valid HTML by its rules.

import DOMPurify from "dompurify";

/** What the caller is willing to let a body reference. */
export interface PublicHtmlPolicy {
  /** The published project. Scopes attachment URLs and issue links. */
  project: string;
  /** Attachment ids the server said are public for this issue or comment. */
  allowedAttachments: ReadonlySet<number>;
  attachmentUrl: (id: number) => string;
  filenameFor: (id: number) => string | undefined;
  altFor: (id: number) => string | undefined;
}

/** Kept, rebuilt with no attributes: marked's GFM output plus common inline
 *  tags. Stripped of attributes, `div` and `span` are inert containers. */
const KEEP_BARE = new Set([
  "p", "br", "hr", "strong", "b", "em", "i", "u", "s", "del", "ins", "mark",
  "sub", "sup", "small", "abbr", "cite", "q", "kbd", "samp", "var",
  "code", "pre", "blockquote",
  "ul", "ol", "li", "dl", "dt", "dd",
  "h1", "h2", "h3", "h4", "h5", "h6",
  "table", "thead", "tbody", "tfoot", "tr", "th", "td", "caption",
  "span", "div", "section", "article", "figure", "figcaption",
]);

/** Dropped with everything inside them: elements that fetch or execute, and
 *  interactive controls that have no business in a document a stranger reads.
 *  Dropping the subtree matters for `style`, whose CSS would otherwise be left
 *  behind as visible prose. */
const DROP_SUBTREE = new Set([
  "script", "style", "iframe", "object", "embed", "applet", "frame",
  "frameset", "video", "audio", "source", "track", "canvas", "svg", "math",
  "link", "meta", "base", "title", "noscript", "template", "portal",
  "form", "button", "select", "textarea", "option", "optgroup", "fieldset",
  "legend", "label", "output", "progress", "meter", "dialog", "map", "area",
  "marquee", "param", "slot", "xmp",
]);

/** Strip the characters HTML attribute parsing tolerates inside a value, so
 *  the scheme tests below see what the browser would see (`java\tscript:`).
 *  Every test built on this is a positive allowlist. */
function normalizeUrl(raw: string | null): string {
  // eslint-disable-next-line no-control-regex
  return (raw ?? "").replace(/[\u0000-\u0020\u007f]/g, "").trim();
}

const ATTACHMENT_RE = /^(?:https?:\/\/[^/]+)?\/api\/attachments\/(\d+)\/?$/i;
const EXTERNAL_RE = /^(?:https?:\/\/|mailto:)/i;

/** `/DEMO/issues/DEMO-4`, `#/DEMO/issues/DEMO-4`, `#/public/DEMO/DEMO-4`. */
const INTERNAL_ISSUE_RE =
  /^#?\/(?:public\/)?([A-Za-z][A-Za-z0-9_-]*)\/(?:issues\/)?([A-Za-z][A-Za-z0-9_-]*-\d+)$/;

function allowedAttachmentId(url: string, policy: PublicHtmlPolicy): number | null {
  const match = url.match(ATTACHMENT_RE);
  if (!match) return null;
  const id = Number(match[1]);
  return policy.allowedAttachments.has(id) ? id : null;
}

/**
 * The public route for an in-project issue link, or null.
 *
 * Both the path's project segment and the identifier's own prefix must equal
 * the project being read. Equality, not a prefix match: `DEMOX` starts with
 * `DEMO` and is a different project, whose publication status this page cannot
 * vouch for.
 */
function internalIssueRoute(url: string, policy: PublicHtmlPolicy): string | null {
  const match = url.match(INTERNAL_ISSUE_RE);
  if (!match) return null;
  const [, project, identifier] = match;
  const prefix = identifier.slice(0, identifier.lastIndexOf("-"));
  const want = policy.project.toLowerCase();
  if (project.toLowerCase() !== want) return null;
  if (prefix.toLowerCase() !== want) return null;
  return `#/public/${policy.project}/${identifier}`;
}

function textOf(node: Node): string {
  return (node.textContent ?? "").trim();
}

interface Ctx {
  doc: Document;
  policy: PublicHtmlPolicy;
  depth: number;
}

/** Guards the recursion against a pathologically nested body. */
const MAX_DEPTH = 100;

function cleanChildren(ctx: Ctx, source: Node, target: Node): void {
  for (const child of Array.from(source.childNodes)) {
    cleanNode({ ...ctx, depth: ctx.depth + 1 }, child, target);
  }
}

function cleanNode(ctx: Ctx, node: Node, target: Node): void {
  if (ctx.depth > MAX_DEPTH) return;

  if (node.nodeType === Node.TEXT_NODE) {
    target.appendChild(ctx.doc.createTextNode(node.nodeValue ?? ""));
    return;
  }
  // Comments, CDATA, processing instructions, doctypes: nothing a reader sees.
  if (node.nodeType !== Node.ELEMENT_NODE) return;

  const el = node as Element;
  const tag = el.tagName.toLowerCase();

  if (DROP_SUBTREE.has(tag)) return;
  if (tag === "a") {
    cleanAnchor(ctx, el, target);
    return;
  }
  if (tag === "img") {
    target.appendChild(cleanImage(ctx, el));
    return;
  }
  if (tag === "input") {
    // GFM task lists are the only input marked emits. Anything else, most
    // sharply `<input type="image" src>`, is a fetch.
    if ((el.getAttribute("type") ?? "").toLowerCase() !== "checkbox") return;
    const box = ctx.doc.createElement("input");
    box.setAttribute("type", "checkbox");
    box.setAttribute("disabled", "");
    if (el.hasAttribute("checked")) box.setAttribute("checked", "");
    target.appendChild(box);
    return;
  }

  if (KEEP_BARE.has(tag)) {
    const fresh = ctx.doc.createElement(tag);
    cleanChildren(ctx, el, fresh);
    target.appendChild(fresh);
    return;
  }

  // Unknown but harmless in itself (a custom element, `picture`, `center`):
  // keep what it said, drop the element. Children are still cleaned, so a
  // `<picture>` cannot smuggle its `<source>` through.
  cleanChildren(ctx, el, target);
}

function cleanAnchor(ctx: Ctx, el: Element, target: Node): void {
  const { doc, policy } = ctx;
  const href = normalizeUrl(el.getAttribute("href"));

  const attachmentId = allowedAttachmentId(href, policy);
  if (attachmentId !== null) {
    const anchor = doc.createElement("a");
    anchor.setAttribute("href", policy.attachmentUrl(attachmentId));
    anchor.setAttribute("class", "public-md__file");
    anchor.setAttribute("rel", "noopener noreferrer");
    anchor.setAttribute("download", "");
    anchor.textContent = textOf(el) || policy.filenameFor(attachmentId) || "file";
    target.appendChild(anchor);
    return;
  }

  const internal = internalIssueRoute(href, policy);
  if (internal !== null) {
    const anchor = doc.createElement("a");
    anchor.setAttribute("href", internal);
    anchor.setAttribute("class", "public-md__issue");
    cleanChildren(ctx, el, anchor);
    target.appendChild(anchor);
    return;
  }

  if (EXTERNAL_RE.test(href)) {
    const anchor = doc.createElement("a");
    anchor.setAttribute("href", href);
    anchor.setAttribute("rel", "noopener noreferrer nofollow");
    anchor.setAttribute("referrerpolicy", "no-referrer");
    anchor.setAttribute("target", "_blank");
    cleanChildren(ctx, el, anchor);
    target.appendChild(anchor);
    return;
  }

  // An app route into private content, a javascript:/data: URL, a relative
  // path, an unknown scheme: keeps its words, loses its destination.
  cleanChildren(ctx, el, target);
}

function cleanImage(ctx: Ctx, el: Element): Element {
  const { doc, policy } = ctx;
  const src = normalizeUrl(el.getAttribute("src"));
  const id = allowedAttachmentId(src, policy);

  if (id === null) {
    // A remote image is described, never fetched.
    const placeholder = doc.createElement("span");
    placeholder.setAttribute("class", "public-md__blocked-image");
    placeholder.textContent = (el.getAttribute("alt") ?? "").trim() || "image";
    return placeholder;
  }

  const img = doc.createElement("img");
  img.setAttribute("src", policy.attachmentUrl(id));
  img.setAttribute(
    "alt",
    (el.getAttribute("alt") ?? "").trim() || policy.altFor(id) || policy.filenameFor(id) || "",
  );
  img.setAttribute("class", "public-md__image");
  img.setAttribute("loading", "lazy");
  img.setAttribute("decoding", "async");
  img.setAttribute("referrerpolicy", "no-referrer");
  return img;
}

/**
 * Clean rendered Markdown HTML for a public page. Returns a string already
 * passed through DOMPurify, safe for `{@html}`.
 */
export function sanitizePublicHtml(html: string, policy: PublicHtmlPolicy): string {
  // Parsed into a `<template>`, whose contents belong to a separate inert
  // document. Browsers agree that nothing there loads or runs, which
  // `DOMParser` documents do not guarantee as uniformly, and the untrusted
  // source must not be able to issue a request while it is being cleaned.
  const holder = document.createElement("template");
  holder.innerHTML = html;

  // The output is built in that same inert document, so not even an approved
  // attachment image starts a fetch here; the one request it makes is the one
  // the live page issues after this HTML is inserted.
  const inert = holder.content.ownerDocument;
  const out = inert.createElement("div");
  cleanChildren({ doc: inert, policy, depth: 0 }, holder.content, out);

  return DOMPurify.sanitize(out.innerHTML, {
    ADD_ATTR: ["download", "referrerpolicy", "target"],
    FORBID_TAGS: [
      "script", "style", "iframe", "object", "embed", "form", "video",
      "audio", "source", "track", "svg", "math", "link", "canvas",
    ],
    FORBID_ATTR: ["style", "srcset", "poster", "background", "formaction", "ping"],
  });
}

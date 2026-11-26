/** Recognize a single image URL without contacting its host. */
export function imageUrlFromPaste(text: string): string | null {
  const value = text.trim();
  if (/\s/.test(value)) return null;
  try {
    const url = new URL(value);
    if (!["http:", "https:"].includes(url.protocol) || url.username || url.password) return null;
    if (!/\.(png|jpe?g|gif|webp)$/i.test(url.pathname)) return null;
    return value;
  } catch {
    return null;
  }
}

/** Preserve edits made during a fetch. A deleted or ambiguous URL stays alone. */
export function replacePastedUrl(
  text: string, url: string, offset: number, snippet: string,
): { text: string; caret: number } | null {
  if (text.slice(offset, offset + url.length) !== url) {
    offset = text.indexOf(url);
    if (offset === -1 || text.indexOf(url, offset + url.length) !== -1) return null;
  }
  return {
    text: text.slice(0, offset) + snippet + text.slice(offset + url.length),
    caret: offset + snippet.length,
  };
}

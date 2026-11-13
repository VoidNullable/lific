// The editor's look: the same type scale, colours and spacing as `.prose`
// in app.css, so a body reads the same whether or not you can edit it.
// Colours come from the app's CSS variables and follow the theme live.

import { EditorView } from "@codemirror/view";

export const editorTheme = EditorView.theme({
  "&": {
    color: "var(--text)",
    backgroundColor: "transparent",
    fontSize: "var(--md-editor-size, 0.875rem)",
  },
  "&.cm-focused": { outline: "none" },
  ".cm-scroller": {
    fontFamily: "var(--font-body)",
    lineHeight: "1.7",
    overflow: "visible",
  },
  ".cm-content": {
    padding: "0",
    caretColor: "var(--accent)",
    minHeight: "var(--md-editor-min-height, 3.4em)",
  },
  ".cm-line": { padding: "0" },
  ".cm-line.cm-md-blank": { lineHeight: "0.75" },
  ".cm-cursor, .cm-dropCursor": { borderLeftColor: "var(--accent)", borderLeftWidth: "2px" },
  "&.cm-focused .cm-selectionBackground, .cm-selectionBackground, ::selection": {
    backgroundColor: "color-mix(in srgb, var(--accent) 22%, transparent) !important",
  },
  ".cm-placeholder": { color: "var(--text-faint)", fontStyle: "italic" },

  // Syntax revealed around the caret: present but quiet.
  ".cm-md-syntax": { color: "var(--text-faint)", fontWeight: "400", fontStyle: "normal" },
  ".cm-md-url": { fontSize: "0.92em" },

  // Headings
  ".cm-md-h": {
    fontFamily: "var(--font-display)",
    fontWeight: "600",
    letterSpacing: "-0.02em",
    lineHeight: "1.3",
    paddingTop: "0.9em !important",
    paddingBottom: "0.25em !important",
  },
  ".cm-md-h1": { fontSize: "1.375rem" },
  ".cm-md-h2": { fontSize: "1.125rem" },
  ".cm-md-h3": { fontSize: "1rem" },
  ".cm-md-h4, .cm-md-h5, .cm-md-h6": { fontSize: "0.875rem" },
  ".cm-line.cm-md-h:first-child": { paddingTop: "0 !important" },
  ".cm-md-setext-mark": { lineHeight: "1", fontSize: "0.75em" },

  // Inline
  ".cm-md-strong": { fontWeight: "600" },
  ".cm-md-em": { fontStyle: "italic" },
  ".cm-md-strike": { textDecoration: "line-through", color: "var(--text-muted)" },
  ".cm-md-code": {
    fontFamily: "var(--font-mono)",
    fontSize: "0.8125em",
    background: "var(--bg-subtle)",
    border: "1px solid var(--border)",
    borderRadius: "4px",
    padding: "0.1em 0.3em",
  },
  ".cm-md-link": {
    color: "var(--accent)",
    textDecoration: "underline",
    textDecorationColor: "color-mix(in srgb, var(--accent) 55%, transparent)",
    textUnderlineOffset: "2px",
  },
  ".cm-md-attachment": {
    textDecoration: "none",
    background: "var(--bg-subtle)",
    border: "1px solid var(--border)",
    borderRadius: "0.5rem",
    padding: "0.1em 0.45em",
    color: "var(--text)",
  },
  ".cm-md-attachment::before": { content: '"📎 "', fontSize: "0.85em" },
  ".cm-md-ref": {
    fontFamily: "var(--font-mono)",
    fontSize: "0.8125em",
    fontWeight: "500",
    color: "var(--accent)",
    background: "var(--accent-subtle)",
    borderRadius: "4px",
    padding: "0.1em 0.35em",
  },
  "&.cm-mod-held .cm-md-link, &.cm-mod-held .cm-md-ref": { cursor: "pointer" },

  // Lists
  ".cm-md-bullet": {
    display: "inline-block",
    color: "var(--text-muted)",
    width: "0.7em",
    textAlign: "center",
  },
  ".cm-md-list-num": { color: "var(--text-muted)", fontVariantNumeric: "tabular-nums" },
  ".cm-md-task": {
    margin: "0 0.45em 0 0",
    verticalAlign: "-0.15em",
    width: "0.95em",
    height: "0.95em",
    accentColor: "var(--accent)",
    cursor: "pointer",
  },
  ".cm-md-task-done": { color: "var(--text-muted)" },

  // Quotes
  ".cm-md-quote": {
    borderLeft: "3px solid var(--border)",
    paddingLeft: "1em !important",
    color: "var(--text-muted)",
    fontStyle: "italic",
  },

  // Rules
  ".cm-md-hr": {
    display: "inline-block",
    width: "100%",
    verticalAlign: "middle",
    borderTop: "1px solid var(--border)",
  },

  // Code blocks: one box drawn across its lines.
  ".cm-md-codeblock": {
    fontFamily: "var(--font-mono)",
    fontSize: "0.8125rem",
    lineHeight: "1.55",
    background: "var(--bg-subtle)",
    borderLeft: "1px solid var(--border)",
    borderRight: "1px solid var(--border)",
    padding: "0 1rem !important",
  },
  ".cm-md-codeblock-first": {
    borderTop: "1px solid var(--border)",
    borderTopLeftRadius: "6px",
    borderTopRightRadius: "6px",
    paddingTop: "0.35rem !important",
  },
  ".cm-md-codeblock-last": {
    borderBottom: "1px solid var(--border)",
    borderBottomLeftRadius: "6px",
    borderBottomRightRadius: "6px",
    paddingBottom: "0.35rem !important",
  },
  ".cm-md-fence-label": {
    float: "right",
    fontFamily: "var(--font-body)",
    fontSize: "0.6875rem",
    textTransform: "uppercase",
    letterSpacing: "0.06em",
    color: "var(--text-faint)",
  },
  ".cm-md-source": { fontFamily: "var(--font-mono)", fontSize: "0.8125rem" },

  // Images
  ".cm-md-image": {
    position: "relative",
    display: "inline-block",
    maxWidth: "100%",
    verticalAlign: "bottom",
    lineHeight: "0",
  },
  ".cm-md-image--block": { display: "inline-block", margin: "0.25rem 0" },
  // An image alone on its line, caret elsewhere: the line is just the image,
  // with no text strut adding space under it.
  ".cm-line.cm-md-image-line": { lineHeight: "0" },
  ".cm-md-image--revealed": { marginTop: "0.25rem" },
  ".cm-md-image img": {
    display: "block",
    maxWidth: "100%",
    maxHeight: "32rem",
    height: "auto",
    borderRadius: "0.5rem",
    border: "1px solid var(--border)",
  },
  ".cm-md-image:not(.cm-md-image--block) img": { maxHeight: "12rem" },
  ".cm-md-image__caption": {
    display: "block",
    lineHeight: "1.4",
    paddingTop: "0.375rem",
    color: "var(--text-muted)",
    fontSize: "var(--text-caption, 0.75rem)",
  },
  ".cm-md-image__open": {
    position: "absolute",
    top: "0.4rem",
    right: "0.4rem",
    display: "inline-flex",
    alignItems: "center",
    justifyContent: "center",
    width: "1.75rem",
    height: "1.75rem",
    borderRadius: "0.375rem",
    border: "1px solid var(--border)",
    background: "color-mix(in srgb, var(--surface) 88%, transparent)",
    color: "var(--text-muted)",
    cursor: "pointer",
    opacity: "0",
    transition: "opacity 0.15s",
  },
  ".cm-md-image:hover .cm-md-image__open, .cm-md-image__open:focus-visible": { opacity: "1" },
  "@media (hover: none)": {
    ".cm-md-image__open": { opacity: "1" },
  },
  ".cm-md-image-src": { fontFamily: "var(--font-mono)", fontSize: "0.8125em" },

  // Rendered blocks (tables, diagrams, raw HTML)
  // Rendered HTML must not inherit the editor's pre-wrap whitespace, or
  // the newlines between table rows show up as gaps.
  ".cm-md-block": { cursor: "text", padding: "0.1rem 0", whiteSpace: "normal" },
  ".cm-md-block .prose > :first-child": { marginTop: "0" },
  ".cm-md-block .prose > :last-child": { marginBottom: "0" },

  // Autocomplete and slash menu
  ".cm-tooltip": {
    border: "1px solid var(--border)",
    background: "var(--surface)",
    borderRadius: "0.5rem",
    boxShadow: "0 12px 32px rgba(0,0,0,0.24)",
    overflow: "hidden",
  },
  ".cm-tooltip.cm-tooltip-autocomplete > ul": {
    fontFamily: "var(--font-body)",
    maxHeight: "18rem",
    minWidth: "17rem",
    maxWidth: "min(24rem, calc(100vw - 2rem))",
    padding: "0.375rem 0",
  },
  ".cm-tooltip.cm-tooltip-autocomplete > ul > li": {
    display: "flex",
    alignItems: "center",
    gap: "0.5rem",
    padding: "0.3rem 0.75rem",
    lineHeight: "1.3",
    color: "var(--text)",
  },
  ".cm-tooltip.cm-tooltip-autocomplete > ul > li[aria-selected]": {
    background: "var(--accent-subtle)",
    color: "var(--text)",
  },
  ".cm-completionIcon": { display: "none" },
  ".cm-completionLabel": { fontSize: "0.8125rem", flex: "1 1 auto", minWidth: "0" },
  ".cm-completionDetail": {
    marginLeft: "auto",
    fontStyle: "normal",
    fontSize: "0.6875rem",
    color: "var(--text-faint)",
    fontFamily: "var(--font-mono)",
    whiteSpace: "nowrap",
  },
  ".cm-completionMatchedText": { textDecoration: "none", fontWeight: "600" },
  ".cm-md-cmd-icon": {
    display: "inline-flex",
    alignItems: "center",
    justifyContent: "center",
    width: "1.5rem",
    height: "1.5rem",
    flex: "0 0 auto",
    borderRadius: "0.375rem",
    border: "1px solid var(--border)",
    background: "var(--bg)",
    color: "var(--text-muted)",
    fontSize: "0.6875rem",
    fontWeight: "600",
  },
  ".cm-md-ref-title": {
    overflow: "hidden",
    textOverflow: "ellipsis",
    whiteSpace: "nowrap",
  },
});

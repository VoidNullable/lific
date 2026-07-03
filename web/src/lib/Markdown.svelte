<script lang="ts">
  import { marked } from "marked";
  import DOMPurify from "dompurify";
  import IssueHoverCard from "./IssueHoverCard.svelte";
  import { IDENTIFIER_RE, refKind, routeFor, projectCodeOf } from "./references";
  import { openPeek } from "./issues/peek.svelte"; // LIF-248
  import { openContextMenu } from "./contextMenu.svelte"; // LIF-248
  import { PanelRight, ExternalLink } from "lucide-svelte";
  import { linkMentionsInText, type MentionUser } from "./mentions"; // LIF-263

  let {
    content,
    class: className = "",
    // LIF-263: known mention targets (visible members). `@username` tokens
    // that resolve against this set render as chips showing the display
    // name; unknown tokens stay literal text.
    mentions = [],
  }: { content: string; class?: string; mentions?: MentionUser[] } = $props();

  // Lowercased username → display name, for chip rendering.
  let mentionMap = $derived(
    new Map(mentions.map((m) => [m.username.toLowerCase(), m.display_name])),
  );

  // LIF-239: identifiers must never be re-linked (or double-linked)
  // inside an existing <a> (would nest anchors — invalid HTML and
  // double-navigation), inside a fenced code block (<pre><code>...),
  // or inside inline code (<code>...). We walk the rendered HTML as a
  // sequence of tags vs. text runs (same split marked's own output
  // naturally falls into) and track a depth counter for those three
  // tag names; identifier matching only runs on text while that
  // counter is zero.
  const SKIP_LINKING_TAGS = new Set(["a", "code", "pre"]);
  const TAG_NAME_RE = /^<\/?([a-zA-Z][a-zA-Z0-9-]*)\b/;

  // LIF-263: mentions are linked in the same tag-aware pass as identifiers,
  // so `@user` inside <a>/<code>/<pre> is left alone too. The mention map is
  // passed in so this stays a pure function of its inputs.
  function linkIdentifiers(html: string, mentionKnown: Map<string, string>): string {
    let skipDepth = 0;
    return html.replace(/<[^>]+>|[^<]+/g, (token) => {
      if (token[0] === "<") {
        const m = token.match(TAG_NAME_RE);
        if (m && SKIP_LINKING_TAGS.has(m[1].toLowerCase())) {
          const isClosing = token[1] === "/";
          const isSelfClosing = token.endsWith("/>");
          if (isClosing) skipDepth = Math.max(0, skipDepth - 1);
          else if (!isSelfClosing) skipDepth += 1;
        }
        return token; // tags themselves are never rewritten
      }
      if (skipDepth > 0) return token; // inside <a>/<code>/<pre> — leave prose alone
      let out = token.replace(IDENTIFIER_RE, (full, code, kindMarker, num) => {
        const kind = refKind(kindMarker);
        const identifier = kindMarker ? `${code}-${kindMarker}${num}` : `${code}-${num}`;
        const href = routeFor(code, kind, identifier);
        // data-issue-ident is how the hover-card effect below finds
        // issue (not page/plan) links to decorate; DOMPurify's ADD_ATTR
        // list further down must keep allowing it through.
        const dataAttr = kind === "issue" ? ` data-issue-ident="${identifier}"` : "";
        return `<a href="${href}" class="identifier-link"${dataAttr}>${identifier}</a>`;
      });
      if (mentionKnown.size > 0) out = linkMentionsInText(out, mentionKnown);
      return out;
    });
  }

  // LIF-107: intercept fenced code. ```mermaid blocks become a placeholder
  // div carrying the (encoded) source; a post-render effect swaps in the
  // SVG. All other code passes through marked's default renderer.
  const renderer = new marked.Renderer();
  const origCode = renderer.code.bind(renderer);
  // eslint-disable-next-line @typescript-eslint/no-explicit-any
  renderer.code = function (token: any): string {
    if (token?.lang === "mermaid") {
      return `<div class="mermaid-block" data-mermaid="${encodeURIComponent(token.text)}"></div>`;
    }
    // LIF-110: wrap real code blocks so a copy button can latch on.
    const inner = origCode(token);
    const lang = (token?.lang ?? "").toLowerCase();
    return `<div class="code-block-wrapper" data-lang="${lang}">${inner}</div>`;
  };

  // LIF-110: tiny inline icons (lucide copy / check / x) for the copy button.
  const COPY_SVG =
    '<svg xmlns="http://www.w3.org/2000/svg" width="14" height="14" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"><rect width="14" height="14" x="8" y="8" rx="2" ry="2"/><path d="M4 16c-1.1 0-2-.9-2-2V4c0-1.1.9-2 2-2h10c1.1 0 2 .9 2 2"/></svg>';
  const CHECK_SVG =
    '<svg xmlns="http://www.w3.org/2000/svg" width="14" height="14" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"><path d="M20 6 9 17l-5-5"/></svg>';
  const X_SVG =
    '<svg xmlns="http://www.w3.org/2000/svg" width="14" height="14" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"><path d="M18 6 6 18"/><path d="m6 6 12 12"/></svg>';

  // Normalize literal \n sequences left over from the escaped-newline bug (LIF-10).
  let normalized = $derived(content.replace(/\\n/g, "\n"));

  let rendered = $derived(
    marked.parse(normalized, { breaks: true, gfm: true, renderer }) as string
  );

  // SECURITY (stored XSS): `marked` passes raw inline HTML through verbatim —
  // it does NOT sanitize. Markdown bodies and comments are authored by users
  // and by MCP agents (which can be prompt-injected), so unsanitized output fed
  // to `{@html}` lets `<img onerror>`, `<svg onload>`, `javascript:` hrefs, etc.
  // run in a viewer's session — and since the SPA keeps its bearer token in
  // localStorage, that is full account takeover. DOMPurify strips event
  // handlers, scripts, and dangerous URL schemes while preserving the markup we
  // generate (identifier <a href="#/...">, the mermaid/code wrapper <div>s with
  // their class + data-* attributes, GFM tables, task-list checkboxes).
  let html = $derived(
    DOMPurify.sanitize(linkIdentifiers(rendered, mentionMap), {
      // Keep the data-mermaid / data-lang / data-issue-ident / data-mention
      // hooks the post-render effects (and mention chips) read.
      ADD_ATTR: ["data-mermaid", "data-lang", "data-issue-ident", "data-mention"],
    })
  );

  let containerEl = $state<HTMLDivElement | null>(null);

  // LIF-239: hover card for auto-linked issue identifiers. Timing is
  // owned here (not by IssueHoverCard) because a single container can
  // hold many identifier links and the show/hide state machine needs to
  // be shared across all of them — e.g. gliding the mouse from one
  // identifier straight to another shouldn't flicker the card closed
  // and reopened.
  let hoverIdent = $state<string | null>(null);
  let hoverAnchor = $state<HTMLElement | null>(null);
  let hoverShowTimer: ReturnType<typeof setTimeout> | null = null;
  let hoverHideTimer: ReturnType<typeof setTimeout> | null = null;
  const HOVER_SHOW_DELAY = 350;
  const HOVER_HIDE_GRACE = 200;

  function scheduleHoverShow(el: HTMLElement, ident: string) {
    if (hoverHideTimer) {
      clearTimeout(hoverHideTimer);
      hoverHideTimer = null;
    }
    if (hoverShowTimer) clearTimeout(hoverShowTimer);
    hoverShowTimer = setTimeout(() => {
      hoverAnchor = el;
      hoverIdent = ident;
    }, HOVER_SHOW_DELAY);
  }

  function scheduleHoverHide() {
    if (hoverShowTimer) {
      clearTimeout(hoverShowTimer);
      hoverShowTimer = null;
    }
    if (hoverHideTimer) clearTimeout(hoverHideTimer);
    hoverHideTimer = setTimeout(() => {
      hoverAnchor = null;
      hoverIdent = null;
    }, HOVER_HIDE_GRACE);
  }

  function cancelHoverHide() {
    if (hoverHideTimer) {
      clearTimeout(hoverHideTimer);
      hoverHideTimer = null;
    }
  }

  // Content changed out from under any open card (e.g. a live edit) —
  // drop it rather than let it point at a detached element.
  $effect(() => {
    html;
    hoverIdent = null;
    hoverAnchor = null;
  });

  $effect(() => {
    return () => {
      if (hoverShowTimer) clearTimeout(hoverShowTimer);
      if (hoverHideTimer) clearTimeout(hoverHideTimer);
    };
  });

  // Wire hover/focus/click/context-menu listeners onto every not-yet-
  // decorated issue link. Mirrors the code-copy-button effect below:
  // direct DOM listeners (not Svelte event bindings) because the anchors
  // come from raw `{@html}` markup, not the component's own template.
  $effect(() => {
    html; // re-run when the rendered markdown changes
    const root = containerEl;
    if (!root) return;
    const links = root.querySelectorAll<HTMLAnchorElement>(
      "a.identifier-link[data-issue-ident]:not([data-hover-decorated])"
    );
    for (const link of Array.from(links)) {
      link.dataset.hoverDecorated = "true";
      const ident = link.dataset.issueIdent as string;
      link.addEventListener("mouseenter", () => scheduleHoverShow(link, ident));
      link.addEventListener("mouseleave", scheduleHoverHide);
      link.addEventListener("focus", () => scheduleHoverShow(link, ident));
      link.addEventListener("blur", scheduleHoverHide);

      // LIF-248: shift-click opens the peek panel instead of navigating —
      // same convention as IssueRow/IssueCard. A plain click keeps the
      // link's normal `href="#/..."` navigation (nothing to intercept:
      // this is a real <a>, not a synthetic click target). Peeking an
      // identifier that's inside the peek panel's OWN rendered
      // description (peek-in-peek) just swaps `peekState.identifier` —
      // PeekPanel's effect re-fetches in place, same as any other
      // re-target.
      link.addEventListener("click", (e) => {
        if (!e.shiftKey) return;
        e.preventDefault();
        openPeek(ident);
      });

      // Right-click: same two actions as IssueRow/IssueCard's context
      // menu. stopPropagation so ContextMenu.svelte's window-level
      // "close on any other contextmenu" listener doesn't immediately
      // close the menu this just opened (see that component's handler
      // for the full ordering argument).
      link.addEventListener("contextmenu", (e) => {
        e.preventDefault();
        e.stopPropagation();
        // Right-clicking doesn't fire mouseleave, so a hover card already
        // showing on this link would otherwise linger on top of the menu
        // (IssueHoverCard renders at a higher z-index than ContextMenu —
        // it needs to win against ordinary scroll/overflow contexts
        // everywhere else it's used). Dismiss it immediately rather than
        // waiting out the mouseleave grace period.
        if (hoverShowTimer) { clearTimeout(hoverShowTimer); hoverShowTimer = null; }
        if (hoverHideTimer) { clearTimeout(hoverHideTimer); hoverHideTimer = null; }
        hoverAnchor = null;
        hoverIdent = null;
        openContextMenu(e.clientX, e.clientY, [
          { label: "Open preview", icon: PanelRight, action: () => openPeek(ident) },
          {
            label: "Open in new tab",
            icon: ExternalLink,
            action: () =>
              window.open(
                `${location.origin}/#/${projectCodeOf(ident)}/issues/${ident}`,
                "_blank",
                "noopener",
              ),
          },
        ]);
      });
    }
  });

  // LIF-107: render mermaid blocks after the HTML lands. Mermaid (~600KB)
  // is dynamically imported so pages without a diagram never pay for it.
  $effect(() => {
    html; // re-run when the rendered markdown changes
    const root = containerEl;
    if (!root) return;
    const blocks = root.querySelectorAll<HTMLDivElement>(
      ".mermaid-block:not([data-rendered])"
    );
    if (blocks.length === 0) return;

    let cancelled = false;
    (async () => {
      const mermaid = (await import("mermaid")).default;
      mermaid.initialize({
        startOnLoad: false,
        theme: document.documentElement.classList.contains("dark")
          ? "dark"
          : "default",
        securityLevel: "strict",
      });
      for (const block of Array.from(blocks)) {
        if (cancelled) return;
        const src = decodeURIComponent(block.dataset.mermaid ?? "");
        try {
          const id = `mmd-${Math.random().toString(36).slice(2)}`;
          const { svg } = await mermaid.render(id, src);
          block.innerHTML = svg;
          block.dataset.rendered = "true";
        } catch (err) {
          block.innerHTML = `<pre style="color:var(--error);white-space:pre-wrap;margin:0;">Mermaid error: ${String(err)}</pre>`;
          block.dataset.rendered = "error";
        }
      }
    })();

    return () => {
      cancelled = true;
    };
  });

  // LIF-110: attach a copy button to each rendered code block. Done in an
  // effect (not the renderer) so the click handler binds to a live node.
  $effect(() => {
    html; // re-run when the rendered markdown changes
    const root = containerEl;
    if (!root) return;
    const wrappers = root.querySelectorAll<HTMLDivElement>(
      ".code-block-wrapper:not([data-decorated])"
    );
    for (const wrapper of Array.from(wrappers)) {
      wrapper.dataset.decorated = "true";
      const btn = document.createElement("button");
      btn.type = "button";
      btn.className = "code-copy-btn";
      btn.setAttribute("aria-label", "Copy code");
      btn.innerHTML = COPY_SVG;
      btn.addEventListener("click", async () => {
        const code = wrapper.querySelector("code")?.textContent ?? "";
        try {
          await navigator.clipboard.writeText(code);
          btn.innerHTML = CHECK_SVG;
        } catch {
          btn.innerHTML = X_SVG;
        }
        setTimeout(() => {
          btn.innerHTML = COPY_SVG;
        }, 1400);
      });
      wrapper.appendChild(btn);
    }
  });
</script>

<div class="prose {className}" bind:this={containerEl}>
  {@html html}
</div>

{#if hoverIdent && hoverAnchor}
  <IssueHoverCard
    identifier={hoverIdent}
    anchorEl={hoverAnchor}
    onEnter={cancelHoverHide}
    onLeave={scheduleHoverHide}
  />
{/if}

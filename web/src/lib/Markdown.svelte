<script lang="ts">
  import { marked } from "marked";

  let { content, class: className = "" }: { content: string; class?: string } =
    $props();

  // Matches issue identifiers (LIF-42) and page identifiers (LIF-DOC-3).
  // Only matches uppercase project codes 1-5 chars followed by -NUMBER or -DOC-NUMBER.
  const IDENT_RE = /\b([A-Z][A-Z0-9]{0,4})-(DOC-)?(\d+)\b/g;

  function linkIdentifiers(html: string): string {
    // Don't replace inside HTML tags, href attributes, or <code> blocks.
    // Split on tags, only process text nodes.
    return html.replace(
      /(<[^>]*>)|(\b[A-Z][A-Z0-9]{0,4}-(?:DOC-)?\d+\b)/g,
      (match, tag, ident) => {
        if (tag) return tag; // HTML tag, leave alone
        if (!ident) return match;
        const isDoc = ident.includes("-DOC-");
        const project = ident.split("-")[0];
        if (isDoc) {
          return `<a href="#/${project}/pages" class="identifier-link">${ident}</a>`;
        }
        return `<a href="#/${project}/issues/${ident}" class="identifier-link">${ident}</a>`;
      }
    );
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

  let html = $derived(linkIdentifiers(rendered));

  let containerEl = $state<HTMLDivElement | null>(null);

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

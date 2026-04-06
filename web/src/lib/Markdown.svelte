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

  // Normalize literal \n sequences left over from the escaped-newline bug (LIF-10).
  let normalized = $derived(content.replace(/\\n/g, "\n"));

  let rendered = $derived(
    marked.parse(normalized, { breaks: true, gfm: true }) as string
  );

  let html = $derived(linkIdentifiers(rendered));
</script>

<div class="prose {className}">
  {@html html}
</div>

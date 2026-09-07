<script lang="ts">
  // LIF-465: the Markdown renderer for the anonymous project view.
  //
  // Separate from lib/Markdown.svelte because that component auto-links issue
  // identifiers and then fetches each one from /api/issues/resolve, mounts
  // AttachmentView against /api/attachments, swaps image sources to the
  // authenticated thumbnail route, and renders mermaid from untrusted source.
  // All four are credentialed or unbounded work on a page a stranger can load,
  // and the first is the contract's "do not auto-follow references to private
  // data" verbatim.
  //
  // This component is the Markdown step and the props. The safety argument
  // lives in sanitizePublicHtml.ts.
  import { marked } from "marked";
  import { publicAttachmentUrl, type PublicAttachment } from "../api";
  import { sanitizePublicHtml } from "./sanitizePublicHtml";

  let {
    content,
    project,
    attachments = [],
    class: className = "",
  }: {
    content: string;
    /** The published project this body belongs to. Attachment URLs and issue
     *  links are both scoped to it, and the server re-checks that scope on
     *  every download. */
    project: string;
    /** The only attachments this body may reference. */
    attachments?: PublicAttachment[];
    class?: string;
  } = $props();

  let byId = $derived(new Map(attachments.map((a) => [a.id, a])));

  let policy = $derived({
    project,
    allowedAttachments: new Set(attachments.map((a) => a.id)),
    attachmentUrl: (id: number) => publicAttachmentUrl(project, id),
    filenameFor: (id: number) => byId.get(id)?.filename,
    altFor: (id: number) => byId.get(id)?.alt_text ?? undefined,
  });

  let rendered = $derived(
    marked.parse(content.replace(/\\n/g, "\n"), {
      breaks: true,
      gfm: true,
    }) as string,
  );

  let html = $derived(sanitizePublicHtml(rendered, policy));
</script>

<div class="prose public-md {className}">
  {@html html}
</div>

<style>
  :global(.public-md img.public-md__image) {
    max-width: 100%;
    max-height: 32rem;
    height: auto;
    border-radius: 0.5rem;
    border: 1px solid var(--border);
  }

  /* An image we refused to load. Reads as the alt text it replaced, muted and
     outlined so it is obviously a placeholder rather than the body's prose. */
  :global(.public-md .public-md__blocked-image) {
    display: inline-block;
    padding: 0.125rem 0.5rem;
    border: 1px dashed var(--border);
    border-radius: 0.375rem;
    color: var(--text-faint);
    font-size: 0.8125rem;
  }

  :global(.public-md a.public-md__file) {
    display: inline-flex;
    align-items: center;
    gap: 0.4375rem;
    max-width: 100%;
    padding: 0.25rem 0.5rem;
    border: 1px solid var(--border);
    border-radius: 0.5rem;
    background: var(--bg-subtle);
    color: var(--text);
    font-size: 0.8125rem;
    line-height: 1.2;
    text-decoration: none;
    vertical-align: middle;
  }
  :global(.public-md a.public-md__file:hover) {
    border-color: var(--accent);
    background: var(--surface);
  }

  /* A link to another issue in the same published project. */
  :global(.public-md a.public-md__issue) {
    color: var(--accent);
    text-decoration: none;
  }
  :global(.public-md a.public-md__issue:hover) {
    text-decoration: underline;
  }
</style>

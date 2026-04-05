<script lang="ts">
  import { marked } from "marked";

  let { content, class: className = "" }: { content: string; class?: string } =
    $props();

  // Normalize literal \n sequences left over from the escaped-newline bug (LIF-10).
  // Real newlines are actual characters; these are two-char backslash+n artifacts.
  let normalized = $derived(content.replace(/\\n/g, "\n"));

  let html = $derived(
    marked.parse(normalized, { breaks: true, gfm: true }) as string
  );
</script>

<div class="prose {className}">
  {@html html}
</div>

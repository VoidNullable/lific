export default {
  sanitize(html: string): string {
    return html;
  },
  // Markdown.svelte registers a public-scope hook at module load (LIF-471);
  // the SSR stand-in accepts and ignores it.
  addHook(): void {},
};

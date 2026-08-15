import { source } from "@/lib/source";

// Served as /llms-full.txt: the entire documentation as one plain-text file
// for LLM consumption. The body is assembled by walking source.getPages() and
// reading each page's raw MDX, so it is derived from the content tree rather
// than maintained by hand. Under `output: "export"` this handler runs during
// the build and the result ships as a static asset.
export const revalidate = false;

function stripFrontmatter(text: string): string {
  return text.replace(/^---\n[\s\S]*?\n---\n/, "");
}

export async function GET() {
  const sections = await Promise.all(
    source.getPages().map(async (page) => {
      const raw = await page.data.getText("raw");
      return [
        `# ${page.data.title}`,
        `URL: https://lific.dev${page.url}`,
        page.data.description ?? "",
        "",
        stripFrontmatter(raw).trim(),
      ].join("\n");
    }),
  );

  return new Response(sections.join("\n\n---\n\n"), {
    headers: { "Content-Type": "text/plain; charset=utf-8" },
  });
}

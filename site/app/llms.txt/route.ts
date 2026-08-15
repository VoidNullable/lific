import { source } from "@/lib/source";
import { llms } from "fumadocs-core/source/llms";

// Static export: rendered once at build time, served as /llms.txt.
// An index of the documentation for LLM consumption (llmstxt.org).
export const revalidate = false;

export function GET() {
  const index = llms(source).index();
  const body = [
    "# Lific",
    "",
    "> A free, self-hosted issue tracker built for coding agents. Single Rust binary, SQLite storage, a built-in MCP server with 27 tools, REST API, CLI, and web UI.",
    "",
    "Full documentation content: https://lific.dev/llms-full.txt",
    "Comparison with other MCP issue trackers (beads, Vikunja, Gitea, Plane, Linear): https://lific.dev/compare",
    "",
    index,
  ].join("\n");

  return new Response(body, {
    headers: { "Content-Type": "text/plain; charset=utf-8" },
  });
}

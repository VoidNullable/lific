import type { NextConfig } from "next";
import { createMDX } from "fumadocs-mdx/next";

const withMDX = createMDX();

const nextConfig: NextConfig = {
  // Fully static: no server-side anything at runtime, and production
  // hosting is Cloudflare Workers static assets (see wrangler.jsonc).
  // The /api/search route is a static GET handler; it exports the docs
  // search index as a JSON file at build time.
  output: "export",
};

export default withMDX(nextConfig);

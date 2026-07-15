import type { MetadataRoute } from "next";
import { source } from "@/lib/source";

// Static export: generated once at build time, served as /sitemap.xml.
// Replaces the old hand-written public/sitemap.xml so docs pages are
// included automatically.
export const dynamic = "force-static";

export default function sitemap(): MetadataRoute.Sitemap {
  return [
    {
      url: "https://lific.dev/",
      changeFrequency: "weekly",
    },
    ...source.getPages().map((page) => ({
      url: `https://lific.dev${page.url}`,
      changeFrequency: "weekly" as const,
    })),
  ];
}

import { source } from "@/lib/source";
import { createFromSource } from "fumadocs-core/search/server";

// Static export: this GET handler runs once at build time and emits the
// docs search index as a static JSON file served from /api/search.
export const revalidate = false;

export const { staticGET: GET } = createFromSource(source);

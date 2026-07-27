import { existsSync, readdirSync, readFileSync, statSync } from "node:fs";
import { join, relative } from "node:path";

const root = process.cwd();
const docsRoot = join(root, "site", "content", "docs");
const errors = [];

function read(path) {
  return readFileSync(path, "utf8");
}

function checkSiteNavigation(directory, label = "docs") {
  const metaPath = join(directory, "meta.json");
  if (!existsSync(metaPath)) {
    errors.push(`${label}: missing meta.json`);
    return;
  }
  const meta = JSON.parse(read(metaPath));
  for (const page of meta.pages ?? []) {
    const file = join(directory, `${page}.mdx`);
    const folder = join(directory, page);
    if (existsSync(file)) continue;
    if (existsSync(join(folder, "meta.json"))) {
      checkSiteNavigation(folder, `${label}/${page}`);
      continue;
    }
    errors.push(`${label}: navigation entry ${page} has no MDX page or child meta.json`);
  }
}

function docsFiles(directory) {
  return readdirSync(directory, { withFileTypes: true }).flatMap((entry) => {
    const path = join(directory, entry.name);
    if (entry.isDirectory()) return docsFiles(path);
    return entry.name.endsWith(".mdx") || entry.name.endsWith(".md") ? [path] : [];
  });
}

function checkLocalLinks() {
  for (const file of docsFiles(docsRoot)) {
    const content = read(file);
    const pattern = /\]\((\/docs(?:\/[^)#?]*)?)(?:[#?][^)]*)?\)/g;
    for (const match of content.matchAll(pattern)) {
      const route = match[1].replace(/\/$/, "");
      const relativeRoute = route === "/docs" ? "index" : route.slice("/docs/".length);
      const candidates = [
        join(docsRoot, `${relativeRoute}.mdx`),
        join(docsRoot, relativeRoute, "index.mdx"),
      ];
      if (!candidates.some(existsSync)) {
        errors.push(`${relative(file, root)}: broken local docs link ${route}`);
      }
    }
  }
}

// The docs deliberately state no version number. Pinning one in prose means
// every release edits a file that carries no information a reader needs: the
// releases page, crates.io, and `lific --version` all answer it authoritatively
// and stay correct on their own. Do not reintroduce a version-parity check.

const mcpSource = read(join(root, "src", "mcp", "tools.rs"));
const mcpDocs = read(join(docsRoot, "mcp", "tools.mdx"));
const sourceToolCount = (mcpSource.match(/^\s*#\[tool\(/gm) ?? []).length;
const docsToolCount = Number(mcpDocs.match(/Lific exposes (\d+) MCP tools/)?.[1]);
if (!Number.isInteger(docsToolCount) || sourceToolCount !== docsToolCount) {
  errors.push(`MCP tool count drift: source=${sourceToolCount}, docs=${docsToolCount}`);
}

checkSiteNavigation(docsRoot);
checkLocalLinks();

if (errors.length) {
  console.error(errors.join("\n"));
  process.exit(1);
}

console.log(`Documentation checks passed: ${sourceToolCount} MCP tools, navigation and local links valid.`);

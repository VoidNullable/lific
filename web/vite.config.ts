import { defineConfig } from "vite";
import { svelte } from "@sveltejs/vite-plugin-svelte";
import tailwindcss from "@tailwindcss/vite";
import { readFileSync } from "node:fs";
import { resolve } from "node:path";

// Where the dev server proxies /api to. Override with VITE_API_TARGET, e.g.
//   VITE_API_TARGET=https://lific.example.com bun run dev
// to develop the UI against a remote Lific instance from another machine.
// Defaults to a local lific binary on 127.0.0.1:3456.
const API_TARGET = process.env.VITE_API_TARGET ?? "http://127.0.0.1:3456";
const PROXY_SECURE = process.env.VITE_API_INSECURE !== "1";

// Pull the canonical version from Cargo.toml so the UI never drifts from the
// binary. Cargo.toml is the single source of truth (see AGENTS.md).
function readCargoVersion(): string {
  try {
    const cargoToml = readFileSync(resolve(__dirname, "../Cargo.toml"), "utf8");
    const match = cargoToml.match(/^version\s*=\s*"([^"]+)"/m);
    return match?.[1] ?? "0.0.0";
  } catch {
    return "0.0.0";
  }
}
const APP_VERSION = readCargoVersion();

export default defineConfig({
  plugins: [tailwindcss(), svelte()],
  define: {
    __APP_VERSION__: JSON.stringify(APP_VERSION),
  },
  build: {
    outDir: "dist",
    emptyOutDir: true,
  },
  server: {
    // Bind on all interfaces so other machines on your LAN/VPN can reach the
    // dev server. Without this vite only listens on 127.0.0.1.
    host: true,
    // Vite rejects Host headers it doesn't recognize (SSRF guard). Opt extra
    // hostnames in via VITE_ALLOWED_HOSTS (comma-separated; a leading dot
    // matches all subdomains, e.g. ".your-tailnet.ts.net").
    allowedHosts: process.env.VITE_ALLOWED_HOSTS?.split(",") ?? [],
    // If 5173 is taken, fail fast instead of switching ports (avoids "module load failed"
    // when the browser tab still points at the old URL).
    port: 5173,
    strictPort: true,
    proxy: {
      "/api": {
        target: API_TARGET,
        changeOrigin: true,
        secure: PROXY_SECURE,
      },
    },
  },
});

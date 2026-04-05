import { defineConfig } from "vite";
import { svelte } from "@sveltejs/vite-plugin-svelte";
import tailwindcss from "@tailwindcss/vite";

export default defineConfig({
  plugins: [tailwindcss(), svelte()],
  build: {
    outDir: "dist",
    emptyOutDir: true,
  },
  server: {
    // If 5173 is taken, fail fast instead of switching ports (avoids “module load failed”
    // when the browser tab still points at the old URL).
    port: 5173,
    strictPort: true,
    // Use 127.0.0.1 (not "localhost") so Node's proxy doesn't prefer IPv6 ::1 while
    // the API is only listening on IPv4 — a common cause of silent fetch failures in dev.
    proxy: {
      "/api": {
        target: "http://127.0.0.1:3456",
        changeOrigin: true,
      },
    },
  },
});

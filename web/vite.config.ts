import { defineConfig } from "vite";
import { svelte } from "@sveltejs/vite-plugin-svelte";

export default defineConfig({
  plugins: [svelte()],
  build: {
    outDir: "dist",
    emptyOutDir: true,
  },
  server: {
    // Dev mode: proxy API calls to the running Lific server
    proxy: {
      "/api": "http://localhost:3456",
    },
  },
});

import { defineConfig } from "vite";
import { svelte } from "@sveltejs/vite-plugin-svelte";

export default defineConfig({
  plugins: [svelte()],
  build: {
    // What `vc-api` serves. Nothing else writes here.
    outDir: "dist",
    emptyOutDir: true,
  },
  server: {
    // The SPA talks to the same origin it is served from, so in development it
    // needs the API in front of it rather than a second origin to configure.
    proxy: {
      "/api": "http://localhost:8080",
      "/oauth2": "http://localhost:8080",
      "/invite": "http://localhost:8080",
    },
  },
});

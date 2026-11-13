import { defineConfig } from "vite";
import solid from "vite-plugin-solid";
import tailwindcss from "@tailwindcss/vite";

// The demo build is what `studio.besu.specta.dev` serves: a static bundle with
// fixture data instead of the Rust API. Everything else proxies `/api` to the
// local server so `pnpm dev` is a normal Vite workflow.
export default defineConfig(({ mode }) => ({
  plugins: [solid(), tailwindcss()],
  define: {
    __DEMO__: JSON.stringify(mode === "demo"),
  },
  server: {
    port: 5173,
    proxy: {
      "/api": "http://127.0.0.1:4983",
    },
  },
  build: {
    outDir: "dist",
    target: "es2022",
  },
}));

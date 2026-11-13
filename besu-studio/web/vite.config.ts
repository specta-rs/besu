import { defineConfig } from "vite";
import solid from "vite-plugin-solid";
import tailwindcss from "@tailwindcss/vite";

// One bundle serves every deployment: the hosted page connects to a local
// `besu studio` over HTTP, and the binary serves the same files from its own
// origin. `pnpm dev` proxies `/api` so a local server is one command away.
export default defineConfig(() => ({
  plugins: [solid(), tailwindcss()],
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

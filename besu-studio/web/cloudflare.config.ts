import { defineConfig } from "cf/config";

/**
 * The hosted copy of the studio: the same bundle the binary embeds, with no
 * Rust server behind it. `wrangler.config.ts` keeps that bundle in `dist`, and
 * this Worker serves it as static assets only.
 */
export default defineConfig({
  worker: {
    name: "besu-studio",
    compatibilityDate: "2025-12-31",
    assets: {
      notFoundHandling: "single-page-application",
    },
    domains: ["studio.besu.specta.dev"],
  },
});

import { defineConfig } from "cf/config";

/**
 * The hosted copy of the studio: the same bundle the binary embeds, with no
 * Rust server behind it. `cf deploy ./dist` publishes that prebuilt bundle as
 * static assets, using the settings here.
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

import { httpSource } from "./api";
import { demoSource } from "../demo/source";
import type { DataSource } from "./types";

/**
 * One switch for the whole app. The demo build (what Cloudflare serves at
 * `studio.besu.specta.dev`) talks to fixtures; every other build talks to the
 * Rust server it was served from.
 */
export const isDemo = __DEMO__;

export const source: DataSource = isDemo ? demoSource : httpSource;

import { readFileSync } from "node:fs";
import { registerHooks } from "node:module";
import { extname } from "node:path";

// Node strips TypeScript itself, but Vite's extensionless imports and JSON
// fixtures need the same resolution rules as the browser build.
registerHooks({
  resolve(specifier, context, nextResolve) {
    try {
      return nextResolve(specifier, context);
    } catch (error) {
      if (/^\.\.?\//.test(specifier) && extname(specifier) === "") {
        for (const suffix of [".ts", ".tsx", ".js"]) {
          try {
            return nextResolve(`${specifier}${suffix}`, context);
          } catch {
            // Try the next extension Vite would resolve.
          }
        }
      }
      throw error;
    }
  },
  load(url, context, nextLoad) {
    if (url.endsWith(".json")) {
      return {
        format: "module",
        source: `export default ${readFileSync(new URL(url), "utf8")};`,
        shortCircuit: true,
      };
    }
    return nextLoad(url, context);
  },
});

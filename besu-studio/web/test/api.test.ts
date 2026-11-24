import assert from "node:assert/strict";
import test from "node:test";
import { httpSource } from "../src/lib/api.ts";

test("SQL requests pass the Stop signal through to fetch", async () => {
  const controller = new AbortController();
  let observed: AbortSignal | null | undefined;
  globalThis.fetch = async (_input, init) => {
    observed = init?.signal;
    return new Response(
      JSON.stringify({ columns: [], rows: [], rows_affected: 0 }),
      { headers: { "content-type": "application/json" } },
    );
  };

  await httpSource().query("SELECT 1", { signal: controller.signal });
  assert.equal(observed, controller.signal);
});

test("an aborted SQL request rejects instead of returning a result", async () => {
  const controller = new AbortController();
  controller.abort();
  globalThis.fetch = async (_input, init) => {
    init?.signal?.throwIfAborted();
    throw new Error("fetch should not continue");
  };

  await assert.rejects(() =>
    httpSource().query("SELECT 1", { signal: controller.signal }),
  );
});

import assert from "node:assert/strict";
import test from "node:test";
import { createConnection } from "../src/lib/connection.ts";
import { demoSource } from "../src/demo/source.ts";

class MemoryStorage {
  private values = new Map<string, string>();

  getItem(key: string): string | null {
    return this.values.get(key) ?? null;
  }

  setItem(key: string, value: string): void {
    this.values.set(key, value);
  }

  removeItem(key: string): void {
    this.values.delete(key);
  }
}

const flush = () => new Promise<void>((resolve) => setImmediate(resolve));

test("a pending startup probe cannot replace an explicit demo choice", async () => {
  const storage = new MemoryStorage();
  Object.defineProperty(globalThis, "localStorage", {
    configurable: true,
    value: storage,
  });
  Object.defineProperty(globalThis, "window", {
    configurable: true,
    value: {
      location: {
        origin: "https://studio.example.invalid",
        protocol: "https:",
      },
    },
  });

  const deferred = Promise.withResolvers<Response>();
  const requests: Array<{ url: string; init?: RequestInit }> = [];
  globalThis.fetch = async (input, init) => {
    requests.push({ url: String(input), init });
    if (String(input) === "/api/info") return deferred.promise;
    return new Response(JSON.stringify({ inserted: 1 }), {
      headers: { "content-type": "application/json" },
    });
  };

  const connection = createConnection();
  const starting = connection.start();
  await flush();
  assert.equal(connection.state().kind, "probing");

  connection.useDemo();
  await flush();
  assert.equal(connection.state().kind, "demo");
  assert.equal(connection.source(), demoSource);
  assert.equal(
    JSON.parse(storage.getItem("besu-studio-connection") ?? "{}").mode,
    "demo",
  );
  assert.equal(requests[0]?.init?.signal?.aborted, true);

  deferred.resolve(
    new Response(
      JSON.stringify({
        dialect: "sqlite",
        database: "unexpected-live.db",
        version: "0.1.0",
        read_only: false,
      }),
      { headers: { "content-type": "application/json" } },
    ),
  );
  await starting;
  await flush();

  assert.equal(connection.state().kind, "demo");
  assert.equal(connection.source(), demoSource);
  assert.equal(
    JSON.parse(storage.getItem("besu-studio-connection") ?? "{}").mode,
    "demo",
  );

  await assert.rejects(() =>
    connection.source()!.insert("dummy_table", { id: 1, value: "fixture" }),
  );
  assert.equal(requests.length, 1, "demo writes must not reach the old live target");
});

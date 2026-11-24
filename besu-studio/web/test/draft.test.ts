import assert from "node:assert/strict";
import test from "node:test";
import {
  draftFrom,
  parseDraft,
  sameJson,
} from "../src/components/grid/draft.ts";
import type { Column } from "../src/lib/types.ts";

const integer: Column = {
  name: "id",
  ty: "Int8",
  nullable: false,
  primary_key: true,
};

test("unsafe 64-bit integers stay exact through JSON and row drafts", () => {
  const first = 9007199254740992;
  const second = "9007199254740993";
  const page = JSON.parse(
    JSON.stringify({ rows: [[first, "first"], [second, "second"]] }),
  ) as { rows: Array<[number | string, string]> };

  assert.equal(page.rows[1]?.[0], second);
  const draft = draftFrom(integer, page.rows[1]?.[0] ?? null);
  assert.equal(draft.text, second);
  const parsed = parseDraft(integer, draft);
  assert.equal(parsed.ok, true);
  if (parsed.ok) assert.equal(parsed.value, second);
  assert.equal(sameJson(second, parsed.ok ? parsed.value : null), true);
});

test("integer drafts reject fractions and exponents", () => {
  for (const text of ["1.5", "1e3"]) {
    const parsed = parseDraft(integer, { text, isNull: false });
    assert.equal(parsed.ok, false);
  }
});

import {
  isBoolean,
  isJson,
  isNumeric,
} from "../../lib/format";
import type { Column, Json } from "../../lib/types";

/** An editable value: the text in the control, plus an explicit NULL flag. */
export interface Draft {
  text: string;
  isNull: boolean;
}

export function draftFrom(column: Column, value: Json): Draft {
  if (value === null || value === undefined) {
    return { text: isBoolean(column) ? "false" : "", isNull: true };
  }
  if (isBoolean(column)) {
    return { text: value ? "true" : "false", isNull: false };
  }
  if (typeof value === "object") {
    return { text: JSON.stringify(value), isNull: false };
  }
  return { text: String(value), isNull: false };
}

export type ParseResult =
  | { ok: true; value: Json }
  | { ok: false; error: string };

function isInteger(column: Column): boolean {
  return /^(Int|UInt)/.test(
    typeof column.ty === "string" ? column.ty : Object.keys(column.ty)[0] ?? "",
  );
}

/**
 * Turn a draft back into a wire value. `required` rejects emptiness (used when
 * inserting a row); `emptyAsNull` maps a blank optional field to NULL rather
 * than an empty string.
 */
export function parseDraft(
  column: Column,
  draft: Draft,
  options?: { required?: boolean; emptyAsNull?: boolean },
): ParseResult {
  if (draft.isNull) {
    return column.nullable
      ? { ok: true, value: null }
      : { ok: false, error: `${column.name} can't be NULL` };
  }

  const raw = draft.text;
  const text = raw.trim();

  if (text === "") {
    if (options?.required) {
      return { ok: false, error: `${column.name} is required` };
    }
    if (isBoolean(column)) return { ok: true, value: false };
    if (options?.emptyAsNull && column.nullable) {
      return { ok: true, value: null };
    }
    if (isNumeric(column)) {
      return { ok: false, error: `${column.name} must be a number` };
    }
    if (isJson(column)) {
      return { ok: false, error: `${column.name} must be valid JSON` };
    }
    return { ok: true, value: raw };
  }

  if (isBoolean(column)) {
    return { ok: true, value: text === "true" || text === "1" };
  }

  if (isNumeric(column)) {
    if (isInteger(column)) {
      if (!/^[+-]?\d+$/.test(text)) {
        return { ok: false, error: `${column.name} must be an integer` };
      }
      const number = Number(text);
      // Unsafe integers stay decimal strings all the way to the API. Turning
      // one into a Number here would silently change a primary key.
      return Number.isSafeInteger(number)
        ? { ok: true, value: number }
        : { ok: true, value: text };
    }

    const number = Number(text);
    return Number.isFinite(number)
      ? { ok: true, value: number }
      : { ok: false, error: `${column.name} must be a number` };
  }

  if (isJson(column)) {
    try {
      return { ok: true, value: JSON.parse(text) as Json };
    } catch {
      return { ok: false, error: `${column.name} isn't valid JSON` };
    }
  }

  return { ok: true, value: raw };
}

export function sameDraft(a: Draft, b: Draft): boolean {
  return a.text === b.text && a.isNull === b.isNull;
}

export function sameJson(a: Json, b: Json): boolean {
  return JSON.stringify(a) === JSON.stringify(b);
}

import type { Filter, FilterOp, Sort } from "../../lib/types";

/**
 * The browsing state of one table. Filters, sorts, paging and column visibility
 * survive navigating away and back for the length of the session, but are not
 * written anywhere — a reload starts fresh.
 */
export interface GridSession {
  filters: Filter[];
  sorts: Sort[];
  offset: number;
  limit: number;
  hidden: string[];
}

const sessions = new Map<string, GridSession>();

function clone(session: GridSession): GridSession {
  return {
    filters: session.filters.map((filter) => ({ ...filter })),
    sorts: session.sorts.map((sort) => ({ ...sort })),
    offset: session.offset,
    limit: session.limit,
    hidden: [...session.hidden],
  };
}

export function readSession(table: string): GridSession {
  const existing = sessions.get(table);
  return existing
    ? clone(existing)
    : { filters: [], sorts: [], offset: 0, limit: 50, hidden: [] };
}

export function writeSession(table: string, session: GridSession): void {
  sessions.set(table, clone(session));
}

export interface FilterOpInfo {
  value: FilterOp;
  label: string;
  needsValue: boolean;
}

export const FILTER_OPS: FilterOpInfo[] = [
  { value: "eq", label: "=", needsValue: true },
  { value: "neq", label: "≠", needsValue: true },
  { value: "gt", label: ">", needsValue: true },
  { value: "gte", label: "≥", needsValue: true },
  { value: "lt", label: "<", needsValue: true },
  { value: "lte", label: "≤", needsValue: true },
  { value: "contains", label: "contains", needsValue: true },
  { value: "starts_with", label: "starts with", needsValue: true },
  { value: "is_null", label: "is null", needsValue: false },
  { value: "is_not_null", label: "is not null", needsValue: false },
];

export function filterOpInfo(op: FilterOp): FilterOpInfo {
  return (
    FILTER_OPS.find((candidate) => candidate.value === op) ?? {
      value: op,
      label: op,
      needsValue: true,
    }
  );
}

export function needsValue(op: FilterOp): boolean {
  return filterOpInfo(op).needsValue;
}

export function opLabel(op: FilterOp): string {
  return filterOpInfo(op).label;
}

/** A safe file stem for exports: `app.users` stays readable, odd names do not. */
export function fileStem(name: string): string {
  const cleaned = name.replace(/[^\w.-]+/g, "-").replace(/^-+|-+$/g, "");
  return cleaned || "rows";
}

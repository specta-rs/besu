/** A JSON value as it comes off the wire. */
export type Json =
  | null
  | boolean
  | number
  | string
  | Json[]
  | { [key: string]: Json };

/**
 * Besu's canonical type, serialised from Rust's externally tagged enum: unit
 * variants arrive as `"Bool"`, parameterised ones as `{ "VarChar": 255 }`.
 */
export type TypeDef = string | { [key: string]: unknown };

export interface Column {
  name: string;
  ty: TypeDef;
  nullable?: boolean;
  auto_increment?: boolean;
  primary_key?: boolean;
  unique?: boolean;
  default?: string;
  generated?: { expression: string; stored?: boolean };
  comment?: string;
}

/**
 * An index key part. A plain column is serialised as a bare string for backward
 * compatibility; an expression or an operator class is an object.
 */
export type IndexColumn =
  | string
  | { column: string; opclass?: string }
  | { expression: string; opclass?: string };

/** The readable target of an index key part, without quoting. */
export function indexColumnLabel(column: IndexColumn): string {
  if (typeof column === "string") return column;
  return "expression" in column ? column.expression : column.column;
}

export interface Index {
  name: string;
  columns: IndexColumn[];
  unique?: boolean;
  kind?: string;
  predicate?: string;
}

export interface Unique {
  name: string;
  columns: string[];
}

export interface ForeignKey {
  name: string;
  columns: string[];
  references_table: string;
  references_columns: string[];
  on_delete?: string;
  on_update?: string;
}

export interface Check {
  name: string;
  expression: string;
}

export interface Table {
  name: string;
  schema?: string;
  columns: Column[];
  indexes?: Index[];
  uniques?: Unique[];
  foreign_keys?: ForeignKey[];
  checks?: Check[];
  comment?: string;
}

export interface View {
  name: string;
  definition: string;
  materialised?: boolean;
}

export interface EnumType {
  name: string;
  values: string[];
}

export interface Sequence {
  name: string;
  schema?: string;
  data_type?: string;
  start?: number;
  increment?: number;
}

export interface Schema {
  tables: Table[];
  enums?: EnumType[];
  views?: View[];
  sequences?: Sequence[];
}

export interface ConnectionInfo {
  dialect: "sqlite" | "postgres" | "mysql" | string;
  database: string;
  version: string;
  read_only?: boolean;
}

export type FilterOp =
  | "eq"
  | "neq"
  | "gt"
  | "gte"
  | "lt"
  | "lte"
  | "contains"
  | "starts_with"
  | "is_null"
  | "is_not_null";

export interface Filter {
  column: string;
  op: FilterOp;
  value?: string;
}

export interface Sort {
  column: string;
  dir: "asc" | "desc";
}

export interface RowQuery {
  limit: number;
  offset: number;
  sort?: Sort[];
  filters?: Filter[];
}

export interface RowsPage {
  table: string;
  columns: Column[];
  rows: Json[][];
  total: number;
  limit: number;
  offset: number;
  primary_key: string[];
  duration_ms?: number;
}

export interface QueryResult {
  columns: string[];
  rows: Json[][];
  rows_affected?: number;
  last_insert_id?: number | null;
  duration_ms?: number;
}

export interface WriteResult {
  inserted?: number;
  updated?: number;
  deleted?: number;
}

/**
 * The studio's whole backend surface. The app only talks to this, so the
 * Cloudflare demo can swap the HTTP client for fixture data without touching a
 * component.
 */
export interface DataSource {
  info(): Promise<ConnectionInfo>;
  schema(): Promise<Schema>;
  rows(table: string, query: RowQuery): Promise<RowsPage>;
  insert(table: string, values: Record<string, Json>): Promise<WriteResult>;
  update(
    table: string,
    key: Record<string, Json>,
    values: Record<string, Json>,
  ): Promise<WriteResult>;
  remove(table: string, key: Record<string, Json>): Promise<WriteResult>;
  query(sql: string): Promise<QueryResult>;
}

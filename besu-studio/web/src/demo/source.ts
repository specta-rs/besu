import type {
  Column,
  ConnectionInfo,
  DataSource,
  Filter,
  Json,
  QueryResult,
  RowsPage,
  RowQuery,
  Schema,
  WriteResult,
} from "../lib/types";
import schemaJson from "./schema.json";
import users from "./users.json";
import posts from "./posts.json";
import comments from "./comments.json";

/**
 * The hosted demo at `studio.besu.specta.dev` has no server, so the same
 * interface is backed by fixtures here. Writes mutate the in-memory copy, which
 * is enough to try the editors; a reload puts everything back.
 */
const schema = schemaJson as unknown as Schema;

const tables: Record<string, Record<string, Json>[]> = {
  users: users as unknown as Record<string, Json>[],
  posts: posts as unknown as Record<string, Json>[],
  comments: comments as unknown as Record<string, Json>[],
};

const nextId = (rows: Record<string, Json>[]) =>
  rows.reduce((max, row) => Math.max(max, Number(row.id ?? 0)), 0) + 1;

const tableFor = (name: string) => {
  const table = schema.tables.find(
    (candidate) => candidate.name === name || `${candidate.schema}.${candidate.name}` === name,
  );
  if (!table) throw new Error(`no table named \`${name}\``);
  return table;
};

const columnFor = (table: Schema["tables"][number], name: string): Column => {
  const column = table.columns.find((candidate) => candidate.name === name);
  if (!column) throw new Error(`no column named \`${name}\``);
  return column;
};

const isNumeric = (column: Column) => {
  const ty = typeof column.ty === "string" ? column.ty : Object.keys(column.ty)[0];
  return /^(Int|UInt|Float)/.test(ty ?? "");
};

/** Case, quoting, whitespace and a trailing semicolon should not matter. */
const normalizeQuery = (sql: string) =>
  sql
    .trim()
    .replace(/;+\s*$/, "")
    .replace(/["`]/g, "")
    .replace(/\s+/g, " ")
    .toLowerCase();

/**
 * The console has no engine behind it in the demo, so a handful of statements
 * are answered from the fixtures. They read the live in-memory rows, so a row
 * inserted in the grid shows up here too.
 */
interface CannedQuery {
  sql: string;
  duration_ms: number;
  columns: () => string[];
  rows: () => Json[][];
}

const cannedQueries: CannedQuery[] = [
  {
    sql: "select * from users",
    duration_ms: 2,
    columns: () => tableFor("users").columns.map((column) => column.name),
    rows: () => {
      const columns = tableFor("users").columns.map((column) => column.name);
      return (tables.users ?? []).map((row) => columns.map((name) => row[name] ?? null));
    },
  },
  {
    sql: "select title, views from posts order by views desc",
    duration_ms: 3,
    columns: () => ["title", "views"],
    rows: () =>
      [...(tables.posts ?? [])]
        .sort((a, b) => Number(b.views ?? 0) - Number(a.views ?? 0))
        .map((row) => [row.title ?? null, row.views ?? null]),
  },
  {
    sql: "select role, count(*) as people from users group by role order by people desc",
    duration_ms: 4,
    columns: () => ["role", "people"],
    rows: () => {
      const counts = new Map<string, number>();
      for (const row of tables.users ?? []) {
        const role = String(row.role ?? "unknown");
        counts.set(role, (counts.get(role) ?? 0) + 1);
      }
      return [...counts.entries()]
        .sort((a, b) => b[1] - a[1])
        .map(([role, people]) => [role, people]);
    },
  },
];

const compare = (column: Column, a: Json, b: Json): number => {
  if (a === null || a === undefined) return -1;
  if (b === null || b === undefined) return 1;
  if (isNumeric(column)) return Number(a) - Number(b);
  return String(a).localeCompare(String(b));
};

const matches = (column: Column, value: Json, filter: Filter): boolean => {
  const needle = filter.value ?? "";
  const text = value === null ? "" : String(value);
  switch (filter.op) {
    case "is_null":
      return value === null;
    case "is_not_null":
      return value !== null;
    case "eq":
      return isNumeric(column) ? Number(value) === Number(needle) : text === needle;
    case "neq":
      return isNumeric(column) ? Number(value) !== Number(needle) : text !== needle;
    case "gt":
      return compare(column, value, needle) > 0;
    case "gte":
      return compare(column, value, needle) >= 0;
    case "lt":
      return compare(column, value, needle) < 0;
    case "lte":
      return compare(column, value, needle) <= 0;
    case "contains":
      return text.toLowerCase().includes(needle.toLowerCase());
    case "starts_with":
      return text.toLowerCase().startsWith(needle.toLowerCase());
  }
};

export const demoSource: DataSource = {
  async info(): Promise<ConnectionInfo> {
    return {
      dialect: "sqlite",
      database: "demo.db",
      version: "0.1.0",
      read_only: false,
    };
  },

  async schema() {
    return schema;
  },

  async rows(name: string, query: RowQuery): Promise<RowsPage> {
    const table = tableFor(name);
    let rows = [...(tables[table.name] ?? [])];

    for (const filter of query.filters ?? []) {
      const column = columnFor(table, filter.column);
      rows = rows.filter((row) => matches(column, row[filter.column] ?? null, filter));
    }

    const sorts = query.sort ?? [];
    if (sorts.length > 0) {
      rows.sort((a, b) => {
        for (const sort of sorts) {
          const column = columnFor(table, sort.column);
          const result = compare(column, a[sort.column] ?? null, b[sort.column] ?? null);
          if (result !== 0) return sort.dir === "asc" ? result : -result;
        }
        return 0;
      });
    }

    const total = rows.length;
    const page = rows.slice(query.offset, query.offset + query.limit);

    return {
      table: table.name,
      columns: table.columns,
      rows: page.map((row) => table.columns.map((column) => row[column.name] ?? null)),
      total,
      limit: query.limit,
      offset: query.offset,
      primary_key: table.columns.filter((column) => column.primary_key).map((column) => column.name),
      duration_ms: 0,
    };
  },

  async insert(name: string, values: Record<string, Json>): Promise<WriteResult> {
    const table = tableFor(name);
    const rows = (tables[table.name] ??= []);
    const idColumn = table.columns.find((column) => column.primary_key);
    const row: Record<string, Json> = { ...values };
    if (idColumn && (row[idColumn.name] === undefined || row[idColumn.name] === null)) {
      row[idColumn.name] = nextId(rows);
    }
    for (const column of table.columns) {
      if (row[column.name] === undefined) row[column.name] = column.nullable ? null : "";
    }
    rows.push(row);
    return { inserted: 1 };
  },

  async update(
    name: string,
    key: Record<string, Json>,
    values: Record<string, Json>,
  ): Promise<WriteResult> {
    const table = tableFor(name);
    const rows = tables[table.name] ?? [];
    let updated = 0;
    for (const row of rows) {
      if (Object.entries(key).every(([column, value]) => row[column] === value)) {
        Object.assign(row, values);
        updated += 1;
      }
    }
    return { updated };
  },

  async remove(name: string, key: Record<string, Json>): Promise<WriteResult> {
    const table = tableFor(name);
    const rows = tables[table.name] ?? [];
    const kept = rows.filter(
      (row) => !Object.entries(key).every(([column, value]) => row[column] === value),
    );
    tables[table.name] = kept;
    return { deleted: rows.length - kept.length };
  },

  async query(sql: string): Promise<QueryResult> {
    const statement = normalizeQuery(sql);
    const canned = cannedQueries.find((entry) => normalizeQuery(entry.sql) === statement);

    if (!canned) {
      throw new Error(
        "The hosted demo only runs a few canned queries. Run `besu studio --database …` locally to run your own SQL.",
      );
    }

    const columns = canned.columns();
    return {
      columns,
      rows: canned.rows(),
      rows_affected: 0,
      last_insert_id: null,
      duration_ms: canned.duration_ms,
    };
  },
};

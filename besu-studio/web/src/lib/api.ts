import type {
  DataSource,
  Filter,
  Json,
  QueryResult,
  RowsPage,
  RowQuery,
  Schema,
  WriteResult,
} from "./types";

/** An error the server reported, with its HTTP status when there is one. */
export class ApiError extends Error {
  status: number;

  constructor(message: string, status = 0) {
    super(message);
    this.status = status;
  }
}

async function request<T>(path: string, init?: RequestInit): Promise<T> {
  const response = await fetch(path, {
    ...init,
    headers: {
      "content-type": "application/json",
      ...(init?.headers ?? {}),
    },
  });

  const body = await response.text();
  const parsed: unknown = body ? JSON.parse(body) : null;

  if (!response.ok) {
    const message =
      parsed && typeof parsed === "object" && "error" in parsed
        ? String((parsed as { error: unknown }).error)
        : `${response.status} ${response.statusText}`;
    throw new ApiError(message, response.status);
  }

  return parsed as T;
}

function queryString(query: RowQuery): string {
  const params = new URLSearchParams();
  params.set("limit", String(query.limit));
  params.set("offset", String(query.offset));
  if (query.sort?.length) {
    params.set(
      "sort",
      query.sort.map((s) => `${s.column}:${s.dir}`).join(","),
    );
  }
  if (query.filters?.length) params.set("filter", JSON.stringify(query.filters));

  return params.toString();
}

/** The client for a live `besu studio` server. */
export const httpSource: DataSource = {
  info: () => request("/api/info"),
  schema: () => request<Schema>("/api/schema"),

  rows: (table, query) =>
    request<RowsPage>(
      `/api/tables/${encodeURIComponent(table)}/rows?${queryString(query)}`,
    ),

  insert: (table, values) =>
    request<WriteResult>(`/api/tables/${encodeURIComponent(table)}/rows`, {
      method: "POST",
      body: JSON.stringify({ values }),
    }),

  update: (table, key, values) =>
    request<WriteResult>(`/api/tables/${encodeURIComponent(table)}/rows`, {
      method: "PATCH",
      body: JSON.stringify({ key, values }),
    }),

  remove: (table, key) =>
    request<WriteResult>(`/api/tables/${encodeURIComponent(table)}/rows`, {
      method: "DELETE",
      body: JSON.stringify({ key }),
    }),

  query: (sql) =>
    request<QueryResult>("/api/query", {
      method: "POST",
      body: JSON.stringify({ sql }),
    }),
};

export type { DataSource, Filter, Json, QueryResult, RowsPage, RowQuery, Schema };

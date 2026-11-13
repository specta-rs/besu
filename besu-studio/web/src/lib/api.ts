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

async function request<T>(base: string, path: string, init?: RequestInit): Promise<T> {
  const response = await fetch(`${base}${path}`, {
    ...init,
    headers: {
      "content-type": "application/json",
      ...(init?.headers ?? {}),
    },
  });

  const body = await response.text();
  let parsed: unknown = null;
  try {
    parsed = body ? JSON.parse(body) : null;
  } catch {
    // A studio server always answers with JSON. Anything else is a proxy, a
    // stale service worker or the static host's SPA fallback, and the caller
    // deserves to hear that rather than a parser message.
    throw new ApiError(
      `${path} did not answer with JSON — is that address a besu studio server?`,
      response.status,
    );
  }

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

/**
 * A client for one `besu studio` server.
 *
 * `base` is empty when the app was served by the server itself, and an origin
 * such as `http://127.0.0.1:4983` when the hosted page connects to a local
 * process.
 */
export function httpSource(base = ""): DataSource {
  return {
    info: () => request(base, "/api/info"),
    schema: () => request<Schema>(base, "/api/schema"),

    rows: (table, query) =>
      request<RowsPage>(
        base,
        `/api/tables/${encodeURIComponent(table)}/rows?${queryString(query)}`,
      ),

    insert: (table, values) =>
      request<WriteResult>(base, `/api/tables/${encodeURIComponent(table)}/rows`, {
        method: "POST",
        body: JSON.stringify({ values }),
      }),

    update: (table, key, values) =>
      request<WriteResult>(base, `/api/tables/${encodeURIComponent(table)}/rows`, {
        method: "PATCH",
        body: JSON.stringify({ key, values }),
      }),

    remove: (table, key) =>
      request<WriteResult>(base, `/api/tables/${encodeURIComponent(table)}/rows`, {
        method: "DELETE",
        body: JSON.stringify({ key }),
      }),

    query: (sql) =>
      request<QueryResult>(base, "/api/query", {
        method: "POST",
        body: JSON.stringify({ sql }),
      }),
  };
}

export type { DataSource, Filter, Json, QueryResult, RowsPage, RowQuery, Schema };

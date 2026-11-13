import {
  Show,
  createEffect,
  createMemo,
  createResource,
  createSignal,
  onCleanup,
  onMount,
  type JSX,
} from "solid-js";
import { AlertTriangle, Plus } from "lucide-solid";
import { AddRowModal } from "../components/grid/add-row";
import { FilterBar } from "../components/grid/filters";
import { DataGrid } from "../components/grid/grid";
import { Pagination } from "../components/grid/pagination";
import { RowDrawer } from "../components/grid/row-drawer";
import { GridToolbar } from "../components/grid/toolbar";
import {
  fileStem,
  readSession,
  writeSession,
} from "../components/grid/session";
import { download, toCsv } from "../lib/format";
import { source } from "../lib/source";
import { useStudio } from "../lib/store";
import type { Column, Filter, Json, Sort } from "../lib/types";
import { EmptyState, Spinner } from "../lib/ui";

/**
 * Owned by the data-grid task. Renders the whole table browser for one table:
 * toolbar (filter, sort, columns, refresh, export, add row), the grid, the row
 * detail drawer and the pagination footer.
 */
export function DataPage(props: { table: string }): JSX.Element {
  return (
    <Show
      when={props.table}
      keyed
      fallback={<div class="p-6 text-[13px] text-ink-3">No table selected.</div>}
    >
      {(table) => <TableBrowser table={table} />}
    </Show>
  );
}

function TableBrowser(props: { table: string }): JSX.Element {
  const studio = useStudio();
  const session = readSession(props.table);

  const [filters, setFilters] = createSignal<Filter[]>(session.filters);
  const [sorts, setSorts] = createSignal<Sort[]>(session.sorts);
  const [offset, setOffset] = createSignal(session.offset);
  const [limit, setLimit] = createSignal(session.limit);
  const [hidden, setHidden] = createSignal<string[]>(session.hidden);
  const [selected, setSelected] = createSignal(-1);
  const [openRow, setOpenRow] = createSignal<{
    row: Json[];
  } | null>(null);
  const [adding, setAdding] = createSignal(false);

  // Session-only memory, so coming back to this table restores the view.
  createEffect(() => {
    writeSession(props.table, {
      filters: filters(),
      sorts: sorts(),
      offset: offset(),
      limit: limit(),
      hidden: hidden(),
    });
  });

  const [page, { refetch }] = createResource(
    () => ({
      table: props.table,
      limit: limit(),
      offset: offset(),
      sort: sorts(),
      filters: filters(),
      revision: studio.revision(),
    }),
    (input) =>
      source.rows(input.table, {
        limit: input.limit,
        offset: input.offset,
        sort: input.sort.length > 0 ? input.sort : undefined,
        filters: input.filters.length > 0 ? input.filters : undefined,
      }),
  );

  // During a refetch the resource keeps the previous value; on an error it
  // throws, so check the error first and let the error state own the pane.
  const data = createMemo(() => (page.error ? undefined : page()));

  const tableDef = createMemo(() =>
    (studio.schema()?.tables ?? []).find(
      (table) =>
        table.name === props.table ||
        `${table.schema ?? ""}.${table.name}` === props.table,
    ),
  );

  const identity = createMemo(() => {
    const definition = tableDef();
    if (definition?.schema) {
      return { schema: definition.schema, name: definition.name };
    }
    const dot = props.table.indexOf(".");
    if (dot > 0) {
      return {
        schema: props.table.slice(0, dot),
        name: props.table.slice(dot + 1),
      };
    }
    return { schema: undefined, name: props.table };
  });

  const columns = (): Column[] =>
    data()?.columns ?? tableDef()?.columns ?? [];
  const primaryKey = (): string[] =>
    data()?.primary_key ??
    (tableDef()?.columns ?? [])
      .filter((column) => column.primary_key)
      .map((column) => column.name);
  const rows = () => data()?.rows ?? [];
  const total = () => data()?.total ?? 0;

  const readOnly = () => Boolean(studio.info()?.read_only);
  const canAdd = () =>
    !readOnly() &&
    columns().some(
      (column) => !column.auto_increment && !column.generated,
    );
  const visibleColumnCount = () =>
    columns().filter((column) => !hidden().includes(column.name)).length;

  const errorMessage = () =>
    page.error instanceof Error
      ? page.error.message
      : String(page.error ?? "The rows could not be loaded.");

  const updateFilters = (next: Filter[]) => {
    setFilters(next);
    setOffset(0);
  };

  const toggleSort = (column: string, secondary: boolean) => {
    setSorts((current) => {
      const existing = current.find((sort) => sort.column === column);

      if (!secondary) {
        if (!existing) return [{ column, dir: "asc" }];
        if (existing.dir === "asc") return [{ column, dir: "desc" }];
        return [];
      }

      if (!existing) return [...current, { column, dir: "asc" }];
      if (existing.dir === "asc") {
        return current.map((sort) =>
          sort.column === column ? { ...sort, dir: "desc" } : sort,
        );
      }
      return current.filter((sort) => sort.column !== column);
    });
    setOffset(0);
  };

  const toggleColumn = (name: string) => {
    setHidden((current) =>
      current.includes(name)
        ? current.filter((candidate) => candidate !== name)
        : [...current, name],
    );
  };

  // If filtering or deleting shrinks the result set, do not strand the user on
  // a page that no longer exists.
  createEffect(() => {
    const current = data();
    if (!current) return;
    const start = offset();
    if (current.total === 0) {
      if (start !== 0) setOffset(0);
      return;
    }
    if (start >= current.total) {
      setOffset(Math.max(0, Math.floor((current.total - 1) / limit()) * limit()));
    }
  });

  createEffect(() => {
    const count = rows().length;
    setSelected((current) =>
      current >= count ? count - 1 : current < -1 ? -1 : current,
    );
  });

  const openAt = (index: number) => {
    const row = rows()[index];
    if (!row) return;
    setSelected(index);
    setOpenRow({ row });
  };

  const move = (delta: number) => {
    const count = rows().length;
    if (count === 0) return;
    setSelected((current) => {
      if (current < 0) return delta > 0 ? 0 : count - 1;
      return Math.min(count - 1, Math.max(0, current + delta));
    });
  };

  onMount(() => {
    const onKey = (event: KeyboardEvent) => {
      if (
        event.defaultPrevented ||
        event.metaKey ||
        event.ctrlKey ||
        event.altKey
      ) {
        return;
      }
      if (studio.paletteOpen() || adding() || openRow()) return;

      const target = event.target as HTMLElement | null;
      if (
        target &&
        (target.isContentEditable ||
          /^(INPUT|TEXTAREA|SELECT)$/.test(target.tagName))
      ) {
        return;
      }

      if (rows().length === 0) return;

      if (event.key === "j" || event.key === "ArrowDown") {
        event.preventDefault();
        move(1);
      } else if (event.key === "k" || event.key === "ArrowUp") {
        event.preventDefault();
        move(-1);
      } else if (event.key === "Enter") {
        event.preventDefault();
        const index = selected() < 0 ? 0 : selected();
        openAt(index);
      }
    };

    window.addEventListener("keydown", onKey);
    onCleanup(() => window.removeEventListener("keydown", onKey));
  });

  const exportPage = (format: "csv" | "json") => {
    const current = data();
    if (!current) return;

    const visible = current.columns
      .map((column, index) => ({ column, index }))
      .filter((entry) => !hidden().includes(entry.column.name));
    const names = visible.map((entry) => entry.column.name);
    const exported = current.rows.map((row) =>
      visible.map((entry) => row[entry.index] ?? null),
    );
    const stem = fileStem(
      identity().schema
        ? `${identity().schema}.${identity().name}`
        : identity().name,
    );

    if (format === "csv") {
      download(`${stem}.csv`, toCsv(names, exported), "text/csv;charset=utf-8");
      return;
    }

    const records = exported.map((row) =>
      Object.fromEntries(names.map((name, index) => [name, row[index] ?? null])),
    );
    download(
      `${stem}.json`,
      JSON.stringify(records, null, 2),
      "application/json",
    );
  };

  return (
    <div class="flex h-full min-h-0 flex-col bg-bg">
      <GridToolbar
        name={identity().name}
        schemaName={identity().schema}
        total={total()}
        durationMs={data()?.duration_ms}
        columnCount={columns().length}
        loading={page.loading}
        columns={columns()}
        hidden={hidden()}
        canAdd={canAdd()}
        onRefresh={() => void refetch()}
        onExport={exportPage}
        onToggleColumn={toggleColumn}
        onShowAllColumns={() => setHidden([])}
        onAddRow={() => setAdding(true)}
      />

      <FilterBar
        columns={columns()}
        filters={filters()}
        onChange={updateFilters}
      />

      <div class="relative flex min-h-0 flex-1 flex-col">
        <Show when={page.error}>
          <div class="flex flex-1 flex-col items-center justify-center gap-3 p-8 text-center">
            <AlertTriangle size={18} class="text-err" />
            <p class="max-w-[420px] text-[12.5px] leading-relaxed text-ink">
              {errorMessage()}
            </p>
            <button
              type="button"
              class="btn"
              onClick={() => void refetch()}
            >
              Retry
            </button>
          </div>
        </Show>

        <Show when={!page.error && !data()}>
          <div class="flex flex-1 items-center justify-center">
            <Spinner label="Loading rows…" />
          </div>
        </Show>

        <Show when={!page.error && data()}>
          <Show
            when={rows().length > 0}
            fallback={
              <EmptyState
                title={
                  filters().length > 0
                    ? "No rows match these filters"
                    : "This table is empty"
                }
                detail={
                  filters().length > 0
                    ? "Adjust or clear the filters to see the rest of the table."
                    : "Add the first row and it will show up here."
                }
              >
                <div class="mt-1 flex items-center gap-2">
                  <Show when={filters().length > 0}>
                    <button
                      type="button"
                      class="btn"
                      onClick={() => updateFilters([])}
                    >
                      Clear filters
                    </button>
                  </Show>
                  <Show when={canAdd()}>
                    <button
                      type="button"
                      class="btn btn-primary"
                      onClick={() => setAdding(true)}
                    >
                      <Plus size={12} />
                      Add row
                    </button>
                  </Show>
                </div>
              </EmptyState>
            }
          >
            <Show
              when={visibleColumnCount() > 0}
              fallback={
                <EmptyState
                  title="All columns are hidden"
                  detail="The rows are still here; bring a column back to see them."
                >
                  <button
                    type="button"
                    class="btn mt-1"
                    onClick={() => setHidden([])}
                  >
                    Show all columns
                  </button>
                </EmptyState>
              }
            >
              <DataGrid
                columns={columns()}
                rows={rows()}
                sorts={sorts()}
                primaryKey={primaryKey()}
                hidden={hidden()}
                selected={selected()}
                onSort={toggleSort}
                onSelect={setSelected}
                onOpen={openAt}
              />
            </Show>
          </Show>
        </Show>

        <Show when={page.loading && data()}>
          <div class="pointer-events-none absolute inset-0 z-40 flex items-start justify-center bg-bg/40 pt-4">
            <div class="panel flex items-center gap-2 px-2.5 py-1 text-[11px] text-ink-2">
              <Spinner size={12} label="Refreshing…" />
            </div>
          </div>
        </Show>
      </div>

      <Pagination
        total={total()}
        offset={offset()}
        limit={limit()}
        onChange={(patch) => {
          if (patch.limit !== undefined) {
            setLimit(patch.limit);
            setOffset(0);
            return;
          }
          if (patch.offset !== undefined) setOffset(Math.max(0, patch.offset));
        }}
      />

      <Show when={openRow()} keyed>
        {(snapshot) => (
          <RowDrawer
            table={props.table}
            columns={columns()}
            row={snapshot.row}
            primaryKey={primaryKey()}
            onClose={() => setOpenRow(null)}
          />
        )}
      </Show>

      <Show when={adding()}>
        <AddRowModal
          table={props.table}
          columns={columns()}
          onClose={() => setAdding(false)}
        />
      </Show>
    </div>
  );
}

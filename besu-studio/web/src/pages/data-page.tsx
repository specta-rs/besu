import {
  Errored,
  Loading,
  Show,
  createEffect,
  createMemo,
  createSignal,
  isPending,
  onSettled,
  refresh,
} from "solid-js";
import type { JSX } from "@solidjs/web";
import { AlertTriangle, Plus } from "../lib/icons";
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

function errorText(error: unknown): string {
  return error instanceof Error ? error.message : String(error);
}

/**
 * The page's async window: the rows are an async memo, so the first read of a
 * table sits inside `<Loading>` and a failed read inside `<Errored>`. Every
 * reader of `page()` lives below both boundaries, which is what lets the
 * fallbacks show: an outside reader would hold the frame instead.
 */
function TableBrowser(props: { table: string }): JSX.Element {
  return (
    <Errored
      fallback={(error, reset) => (
        <div class="flex h-full flex-col items-center justify-center gap-3 p-8 text-center">
          <AlertTriangle size={18} class="text-err" />
          <p class="max-w-[420px] text-[12.5px] leading-relaxed text-ink">
            {errorText(error())}
          </p>
          <button type="button" class="btn" onClick={reset}>
            Retry
          </button>
        </div>
      )}
    >
      <Loading
        fallback={
          <div class="flex h-full items-center justify-center">
            <Spinner label="Loading rows…" />
          </div>
        }
      >
        <TableRows table={props.table} />
      </Loading>
    </Errored>
  );
}

function TableRows(props: { table: string }): JSX.Element {
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
  createEffect(
    () => [props.table, filters(), sorts(), offset(), limit(), hidden()] as const,
    ([table, current, order, start, size, columns]) => {
      writeSession(table, {
        filters: current,
        sorts: order,
        offset: start,
        limit: size,
        hidden: columns,
      });
    },
  );

  const page = createMemo(() => {
    // A write bumps the revision; every table view re-asks when it changes.
    studio.revision();
    return studio.client().rows(props.table, {
      limit: limit(),
      offset: offset(),
      sort: sorts().length > 0 ? sorts() : undefined,
      filters: filters().length > 0 ? filters() : undefined,
    });
  });

  // A reload keeps the previous rows on screen; this is what says it is stale.
  const reloading = createMemo(() => isPending(() => page()));

  const refetch = () => {
    void refresh(page);
  };

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
    page().columns ?? tableDef()?.columns ?? [];
  const primaryKey = (): string[] =>
    page().primary_key ??
    (tableDef()?.columns ?? [])
      .filter((column) => column.primary_key)
      .map((column) => column.name);
  const rows = () => page().rows ?? [];
  const total = () => page().total ?? 0;

  const readOnly = () => Boolean(studio.info()?.read_only);
  const canAdd = () =>
    !readOnly() &&
    columns().some(
      (column) => !column.auto_increment && !column.generated,
    );
  const visibleColumnCount = () =>
    columns().filter((column) => !hidden().includes(column.name)).length;

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
  createEffect(
    () => [page().total, offset(), limit()] as const,
    ([count, start, size]) => {
      if (count === 0) {
        if (start !== 0) setOffset(0);
        return;
      }
      if (start >= count) {
        setOffset(Math.max(0, Math.floor((count - 1) / size) * size));
      }
    },
  );

  createEffect(
    () => rows().length,
    (count) => {
      setSelected((current) =>
        current >= count ? count - 1 : current < -1 ? -1 : current,
      );
    },
  );

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

  onSettled(() => {
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
    return () => window.removeEventListener("keydown", onKey);
  });

  const exportPage = (format: "csv" | "json") => {
    const current = page();

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
        durationMs={page().duration_ms}
        columnCount={columns().length}
        loading={reloading()}
        columns={columns()}
        hidden={hidden()}
        canAdd={canAdd()}
        onRefresh={refetch}
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

        <Show when={reloading()}>
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

import { For, Show, createSignal, type JSX } from "solid-js";
import {
  Check,
  Columns3,
  Download,
  Plus,
  RefreshCw,
} from "lucide-solid";
import { compactNumber, displayType, duration } from "../../lib/format";
import type { Column } from "../../lib/types";
import { Popover } from "./popover";

export interface GridToolbarProps {
  name: string;
  schemaName?: string;
  total: number;
  durationMs?: number;
  columnCount: number;
  loading: boolean;
  columns: Column[];
  hidden: string[];
  canAdd: boolean;
  onRefresh: () => void;
  onExport: (format: "csv" | "json") => void;
  onToggleColumn: (name: string) => void;
  onShowAllColumns: () => void;
  onAddRow: () => void;
}

/** The table's title bar: identity, counts and the row-level actions. */
export function GridToolbar(props: GridToolbarProps): JSX.Element {
  const [menu, setMenu] = createSignal<"export" | "columns" | null>(null);
  let exportButton: HTMLButtonElement | undefined;
  let columnsButton: HTMLButtonElement | undefined;

  return (
    <header class="flex shrink-0 flex-wrap items-center gap-x-3 gap-y-2 border-b border-line bg-panel px-3 py-2.5">
      <div class="min-w-0 flex-1">
        <div class="flex min-w-0 items-center gap-1.5 text-[13px]">
          <Show when={props.schemaName}>
            <span class="shrink-0 text-ink-3">{props.schemaName}</span>
            <span class="shrink-0 text-ink-3">/</span>
          </Show>
          <h1 class="truncate text-[14px] font-semibold tracking-[-0.01em] text-ink">
            {props.name}
          </h1>
        </div>
        <div class="mt-1 flex flex-wrap items-center gap-2 text-[11px] text-ink-3">
          <span class="inline-flex items-center rounded-full bg-accent-soft px-1.5 py-0.5 font-mono text-[10.5px] text-accent">
            {compactNumber(props.total)} rows
          </span>
          <Show when={props.durationMs !== undefined}>
            <span class="font-mono">{duration(props.durationMs)}</span>
          </Show>
          <span class="font-mono">{props.columnCount} cols</span>
        </div>
      </div>

      <div class="flex items-center gap-1.5">
        <button
          type="button"
          class="btn"
          onClick={props.onRefresh}
          disabled={props.loading}
          title="Refresh rows"
        >
          <RefreshCw
            size={12}
            class={props.loading ? "animate-spin" : undefined}
          />
          Refresh
        </button>

        <button
          ref={exportButton}
          type="button"
          class="btn"
          onClick={() =>
            setMenu((current) => (current === "export" ? null : "export"))
          }
          disabled={props.columns.length === 0}
          aria-haspopup="menu"
          aria-expanded={menu() === "export"}
          title="Export the current page"
        >
          <Download size={12} />
          Export
        </button>
        <Popover
          open={menu() === "export"}
          anchor={exportButton}
          onClose={() => setMenu(null)}
          width={190}
          align="end"
        >
          <div role="menu" class="grid gap-0.5">
            <button
              type="button"
              role="menuitem"
              class="btn btn-ghost h-7 justify-start font-normal"
              onClick={() => {
                setMenu(null);
                props.onExport("csv");
              }}
            >
              Export page as CSV
            </button>
            <button
              type="button"
              role="menuitem"
              class="btn btn-ghost h-7 justify-start font-normal"
              onClick={() => {
                setMenu(null);
                props.onExport("json");
              }}
            >
              Export page as JSON
            </button>
          </div>
        </Popover>

        <button
          ref={columnsButton}
          type="button"
          class="btn"
          onClick={() =>
            setMenu((current) => (current === "columns" ? null : "columns"))
          }
          disabled={props.columns.length === 0}
          aria-haspopup="menu"
          aria-expanded={menu() === "columns"}
          title="Show and hide columns"
        >
          <Columns3 size={12} />
          Columns
        </button>
        <Popover
          open={menu() === "columns"}
          anchor={columnsButton}
          onClose={() => setMenu(null)}
          width={260}
          align="end"
        >
          <div class="mb-1 flex items-center justify-between gap-2 px-1">
            <span class="text-[10px] font-medium tracking-wide text-ink-3 uppercase">
              Columns
            </span>
            <button
              type="button"
              class="btn btn-ghost h-5 px-1 text-[11px]"
              onClick={props.onShowAllColumns}
            >
              Show all
            </button>
          </div>
          <div class="grid max-h-[320px] overflow-auto">
            <For each={props.columns}>
              {(column) => {
                const visible = () => !props.hidden.includes(column.name);
                return (
                  <label class="flex cursor-pointer items-center gap-2 rounded px-1.5 py-1 hover:bg-panel-2">
                    <span
                      class={`flex size-3.5 shrink-0 items-center justify-center rounded-[3px] border ${
                        visible()
                          ? "border-accent bg-accent text-accent-ink"
                          : "border-line-strong"
                      }`}
                    >
                      <Show when={visible()}>
                        <Check size={10} stroke-width={3} />
                      </Show>
                    </span>
                    <input
                      type="checkbox"
                      class="sr-only"
                      checked={visible()}
                      onChange={() => props.onToggleColumn(column.name)}
                    />
                    <span class="min-w-0 flex-1 truncate font-mono text-[11.5px] text-ink">
                      {column.name}
                    </span>
                    <span class="shrink-0 font-mono text-[10px] text-ink-3">
                      {displayType(column.ty)}
                    </span>
                  </label>
                );
              }}
            </For>
          </div>
        </Popover>

        <button
          type="button"
          class="btn btn-primary"
          onClick={props.onAddRow}
          disabled={!props.canAdd}
          title={props.canAdd ? "Add a row" : "This connection is read-only"}
        >
          <Plus size={13} />
          Add row
        </button>
      </div>
    </header>
  );
}

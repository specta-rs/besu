import { For, Show, createEffect, createMemo, type JSX } from "solid-js";
import { ArrowDown, ArrowUp, KeyRound } from "lucide-solid";
import {
  cellText,
  displayType,
  isBoolean,
  isJson,
  isNumeric,
} from "../../lib/format";
import type { Column, Json, Sort } from "../../lib/types";

export interface DataGridProps {
  columns: Column[];
  rows: Json[][];
  sorts: Sort[];
  primaryKey: string[];
  hidden: string[];
  selected: number;
  onSort: (column: string, secondary: boolean) => void;
  onSelect: (index: number) => void;
  onOpen: (index: number) => void;
}

interface VisibleColumn {
  column: Column;
  index: number;
}

/**
 * The dense table itself. Sticky header, sticky first column on wide tables,
 * 30px rows, hairline separators and a tinted selection rather than an outline.
 */
export function DataGrid(props: DataGridProps): JSX.Element {
  let scroller: HTMLDivElement | undefined;

  const visible = createMemo<VisibleColumn[]>(() =>
    props.columns
      .map((column, index) => ({ column, index }))
      .filter((entry) => !props.hidden.includes(entry.column.name)),
  );

  const stickyFirst = () => visible().length > 5;

  const sortFor = (name: string) =>
    props.sorts.find((sort) => sort.column === name);

  const sortPosition = (name: string) =>
    props.sorts.findIndex((sort) => sort.column === name) + 1;

  createEffect(() => {
    const index = props.selected;
    if (index < 0) return;
    const row = scroller?.querySelector<HTMLTableRowElement>(
      `tr[data-row="${index}"]`,
    );
    row?.scrollIntoView({ block: "nearest" });
  });

  const cellBackground = (
    rowIndex: number,
    isSelected: boolean,
    stickyCell: boolean,
  ) => {
    if (isSelected) {
      // The sticky column must stay opaque; mix the tint over the row colour.
      return stickyCell
        ? "bg-[color-mix(in_srgb,var(--sp-selection),var(--sp-grid-row))]"
        : "bg-[var(--sp-selection)]";
    }
    return rowIndex % 2 === 0
      ? "bg-[var(--sp-grid-row)]"
      : "bg-[var(--sp-grid-row-alt)]";
  };

  return (
    <div
      ref={scroller}
      class="relative min-h-0 flex-1 overflow-auto bg-panel"
      tabIndex={0}
      role="region"
      aria-label="Table rows"
    >
      <table class="min-w-full border-separate border-spacing-0 text-[12px]">
        <thead>
          <tr>
            <For each={visible()}>
              {(entry, visibleIndex) => {
                const sort = () => sortFor(entry.column.name);
                const sticky = () => stickyFirst() && visibleIndex() === 0;
                return (
                  <th
                    scope="col"
                    aria-sort={
                      sort()?.dir === "asc"
                        ? "ascending"
                        : sort()?.dir === "desc"
                          ? "descending"
                          : "none"
                    }
                    class={`sticky top-0 z-20 border-b border-line bg-[var(--sp-grid-head)] p-0 text-left align-top ${
                      sticky() ? "left-0 z-30 border-r" : ""
                    }`}
                  >
                    <button
                      type="button"
                      class="flex w-full flex-col items-start gap-0.5 px-3 py-1.5 text-left hover:bg-panel-2"
                      onClick={(event) =>
                        props.onSort(entry.column.name, event.shiftKey)
                      }
                      title={`Sort by ${entry.column.name}`}
                    >
                      <span class="flex max-w-full items-center gap-1">
                        <span class="truncate text-[12px] font-medium text-ink">
                          {entry.column.name}
                        </span>
                        <Show when={sort()}>
                          {(active) => (
                            <span class="flex shrink-0 items-center gap-0.5 text-accent">
                              <Show
                                when={active().dir === "asc"}
                                fallback={<ArrowDown size={11} />}
                              >
                                <ArrowUp size={11} />
                              </Show>
                              <Show when={props.sorts.length > 1}>
                                <span class="rounded bg-accent-soft px-1 font-mono text-[9px] leading-[13px] text-accent">
                                  {sortPosition(entry.column.name)}
                                </span>
                              </Show>
                            </span>
                          )}
                        </Show>
                        <Show
                          when={
                            entry.column.primary_key ||
                            props.primaryKey.includes(entry.column.name)
                          }
                        >
                          <KeyRound
                            size={10}
                            class="shrink-0 text-ink-3"
                            aria-label="Primary key"
                          />
                        </Show>
                      </span>
                      <span class="font-mono text-[10px] font-normal text-ink-3">
                        {displayType(entry.column.ty)}
                      </span>
                    </button>
                  </th>
                );
              }}
            </For>
          </tr>
        </thead>
        <tbody>
          <For each={props.rows}>
            {(row, rowIndex) => {
              const isSelected = () => rowIndex() === props.selected;
              return (
                <tr
                  data-row={rowIndex()}
                  class="group cursor-pointer"
                  onClick={() => {
                    props.onSelect(rowIndex());
                    props.onOpen(rowIndex());
                  }}
                >
                  <For each={visible()}>
                    {(entry, visibleIndex) => {
                      const sticky = () => stickyFirst() && visibleIndex() === 0;
                      return (
                        <td
                          class={`h-[30px] border-b border-line px-3 align-middle whitespace-nowrap ${
                            sticky() ? "sticky left-0 z-10 border-r" : ""
                          } ${cellBackground(
                            rowIndex(),
                            isSelected(),
                            sticky(),
                          )} ${
                            isSelected() ? "" : "group-hover:bg-panel-2"
                          } ${isNumeric(entry.column) ? "text-right tabular-nums" : ""}`}
                        >
                          <Cell
                            column={entry.column}
                            value={row[entry.index] ?? null}
                          />
                        </td>
                      );
                    }}
                  </For>
                </tr>
              );
            }}
          </For>
        </tbody>
      </table>
    </div>
  );
}

function Cell(props: { column: Column; value: Json }): JSX.Element {
  const text = () => cellText(props.value);

  return (
    <Show
      when={props.value !== null && props.value !== undefined}
      fallback={
        <span class="text-[12px] text-ink-3 italic">NULL</span>
      }
    >
      <Show
        when={!isBoolean(props.column)}
        fallback={
          <span
            class={`inline-flex items-center rounded border px-1.5 py-px font-mono text-[10.5px] leading-[15px] ${
              props.value
                ? "border-ok/40 bg-ok/10 text-ok"
                : "border-line bg-panel-2 text-ink-3"
            }`}
          >
            {props.value ? "true" : "false"}
          </span>
        }
      >
        <Show
          when={!isJson(props.column)}
          fallback={
            <span
              class="block max-w-[340px] truncate font-mono text-[11.5px] text-ink-2"
              title={text()}
            >
              {text()}
            </span>
          }
        >
          <span class="block max-w-[340px] truncate" title={text()}>
            {text()}
          </span>
        </Show>
      </Show>
    </Show>
  );
}

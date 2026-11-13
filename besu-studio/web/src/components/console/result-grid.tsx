import { For, Index, Show, createMemo } from "solid-js";
import { cellText } from "../../lib/format";
import type { Json } from "../../lib/types";

/**
 * The console's own result table. Deliberately local: the data grid belongs to
 * another task, and a result set has no column metadata to drive it anyway.
 * Same visual language — sticky header, hairline rules, monospace cells, a dim
 * italic `NULL`, numbers right-aligned.
 */

function CellValue(props: { value: Json }) {
  const value = props.value;

  if (value === null) {
    return <span class="text-[12px] text-ink-3 italic">NULL</span>;
  }
  if (typeof value === "boolean") {
    return (
      <span class="inline-flex h-[17px] items-center rounded border border-line bg-panel-2 px-1.5 font-mono text-[10px] text-ink-2">
        {value ? "true" : "false"}
      </span>
    );
  }
  if (typeof value === "object") {
    return (
      <span
        class="block max-w-[560px] truncate text-ink-2"
        title={JSON.stringify(value)}
      >
        {cellText(value)}
      </span>
    );
  }

  return (
    <span class="block max-w-[560px] truncate" title={String(value)}>
      {String(value)}
    </span>
  );
}

export function ResultGrid(props: { columns: string[]; rows: Json[][] }) {
  // A column is numeric when every value in it is a number. Empty result sets
  // have no evidence, so they stay left-aligned.
  const numeric = createMemo(() =>
    props.columns.map((_, index) => {
      let seen = false;
      for (const row of props.rows) {
        const value = row[index];
        if (value === null || value === undefined) continue;
        if (typeof value !== "number") return false;
        seen = true;
      }
      return seen;
    }),
  );

  return (
    <div class="h-full overflow-auto">
      <table class="w-full border-collapse font-mono text-[12px]">
        <caption class="sr-only">Query results</caption>
        <thead class="sticky top-0 z-10 bg-panel-2">
          <tr>
            <th
              scope="col"
              class="h-8 w-10 border-b border-line px-2 text-right text-[10px] font-medium text-ink-3"
            >
              #
            </th>
            <Index each={props.columns}>
              {(column, columnIndex) => (
                <th
                  scope="col"
                  class="h-8 border-b border-line px-2.5 text-left text-[11px] font-medium whitespace-nowrap text-ink-2"
                  classList={{ "text-right": numeric()[columnIndex] === true }}
                >
                  {column()}
                </th>
              )}
            </Index>
            <th scope="col" class="h-8 w-full border-b border-line" aria-hidden="true" />
          </tr>
        </thead>
        <tbody>
          <Show
            when={props.rows.length > 0}
            fallback={
              <tr>
                <td
                  class="px-3 py-6 text-center text-[12px] text-ink-3"
                  colSpan={props.columns.length + 2}
                >
                  No rows returned.
                </td>
              </tr>
            }
          >
            <For each={props.rows}>
              {(row, rowIndex) => (
                <tr class="even:bg-panel-2/40 hover:bg-accent-soft">
                  <td class="h-7 border-b border-line/60 px-2 text-right align-middle text-[10px] text-ink-3 tabular-nums">
                    {rowIndex() + 1}
                  </td>
                  <Index each={props.columns}>
                    {(_, columnIndex) => (
                      <td
                        class="h-7 border-b border-line/60 px-2.5 align-middle"
                        classList={{
                          "text-right tabular-nums": numeric()[columnIndex] === true,
                        }}
                      >
                        <CellValue value={row[columnIndex] ?? null} />
                      </td>
                    )}
                  </Index>
                  <td class="border-b border-line/60" aria-hidden="true" />
                </tr>
              )}
            </For>
          </Show>
        </tbody>
      </table>
    </div>
  );
}

import { For, Show, createSignal, type JSX } from "solid-js";
import { Filter as FilterIcon, Plus, X } from "lucide-solid";
import type { Column, Filter, FilterOp } from "../../lib/types";
import { FILTER_OPS, needsValue, opLabel } from "./session";
import { Popover } from "./popover";

export interface FilterBarProps {
  columns: Column[];
  filters: Filter[];
  onChange: (filters: Filter[]) => void;
}

/** The compact strip of active filters above the grid. */
export function FilterBar(props: FilterBarProps): JSX.Element {
  return (
    <div class="flex shrink-0 flex-wrap items-center gap-1.5 border-b border-line bg-panel px-3 py-1.5">
      <span class="mr-0.5 flex items-center gap-1 text-[10px] font-medium tracking-wide text-ink-3 uppercase">
        <FilterIcon size={10} />
        Filters
      </span>

      <Show when={props.filters.length === 0}>
        <span class="text-[11.5px] text-ink-3">No filters</span>
      </Show>

      <For each={props.filters}>
        {(filter, index) => (
          <FilterChip
            filter={filter}
            columns={props.columns}
            onSave={(next) =>
              props.onChange(
                props.filters.map((candidate, position) =>
                  position === index() ? next : candidate,
                ),
              )
            }
            onRemove={() =>
              props.onChange(
                props.filters.filter((_, position) => position !== index()),
              )
            }
          />
        )}
      </For>

      <AddFilter
        columns={props.columns}
        onAdd={(filter) => props.onChange([...props.filters, filter])}
      />
    </div>
  );
}

function FilterChip(props: {
  filter: Filter;
  columns: Column[];
  onSave: (filter: Filter) => void;
  onRemove: () => void;
}): JSX.Element {
  const [open, setOpen] = createSignal(false);
  let trigger: HTMLButtonElement | undefined;

  return (
    <>
      <div class="inline-flex h-7 items-center rounded-md border border-line bg-panel-2 px-0.5 text-[11.5px]">
        <button
          ref={trigger}
          type="button"
          class="flex h-6 max-w-[280px] items-center gap-1 rounded-[5px] px-1.5 hover:bg-panel-3"
          onClick={() => setOpen((current) => !current)}
          aria-label={`Edit filter on ${props.filter.column}`}
          aria-expanded={open()}
        >
          <span class="truncate font-mono text-ink">{props.filter.column}</span>
          <span class="shrink-0 text-ink-3">·</span>
          <span class="shrink-0 text-ink-2">{opLabel(props.filter.op)}</span>
          <Show when={needsValue(props.filter.op)}>
            <span class="shrink-0 text-ink-3">·</span>
            <span
              class="max-w-[140px] truncate font-mono text-accent"
              title={props.filter.value}
            >
              {props.filter.value}
            </span>
          </Show>
        </button>
        <button
          type="button"
          class="btn btn-ghost h-5 w-5 shrink-0 rounded p-0 text-ink-3 hover:text-err"
          onClick={props.onRemove}
          aria-label={`Remove filter on ${props.filter.column}`}
        >
          <X size={11} />
        </button>
      </div>

      <Popover
        open={open()}
        anchor={trigger}
        onClose={() => setOpen(false)}
        width={310}
      >
        <FilterEditor
          columns={props.columns}
          filter={props.filter}
          onApply={(next) => {
            props.onSave(next);
            setOpen(false);
          }}
          onRemove={props.onRemove}
        />
      </Popover>
    </>
  );
}

function AddFilter(props: {
  columns: Column[];
  onAdd: (filter: Filter) => void;
}): JSX.Element {
  const [open, setOpen] = createSignal(false);
  let trigger: HTMLButtonElement | undefined;

  return (
    <>
      <button
        ref={trigger}
        type="button"
        class="btn btn-ghost h-7 border border-dashed border-line px-2 text-[11.5px] text-ink-2"
        onClick={() => setOpen(true)}
        disabled={props.columns.length === 0}
        aria-expanded={open()}
      >
        <Plus size={11} />
        Add filter
      </button>
      <Popover
        open={open()}
        anchor={trigger}
        onClose={() => setOpen(false)}
        width={310}
      >
        <FilterEditor
          columns={props.columns}
          filter={{
            column: props.columns[0]?.name ?? "",
            op: "eq",
            value: "",
          }}
          onApply={(filter) => {
            props.onAdd(filter);
            setOpen(false);
          }}
        />
      </Popover>
    </>
  );
}

function FilterEditor(props: {
  columns: Column[];
  filter: Filter;
  onApply: (filter: Filter) => void;
  onRemove?: () => void;
}): JSX.Element {
  const [column, setColumn] = createSignal(props.filter.column);
  const [op, setOp] = createSignal<FilterOp>(props.filter.op);
  const [value, setValue] = createSignal(props.filter.value ?? "");

  const apply = () => {
    if (!column()) return;
    const filter: Filter = { column: column(), op: op() };
    if (needsValue(op())) filter.value = value();
    props.onApply(filter);
  };

  return (
    <form
      class="grid gap-2"
      onSubmit={(event) => {
        event.preventDefault();
        apply();
      }}
    >
      <div class="grid grid-cols-2 gap-2">
        <label class="grid gap-1">
          <span class="text-[10px] font-medium tracking-wide text-ink-3 uppercase">
            Column
          </span>
          <select
            class="field w-full"
            value={column()}
            onChange={(event) => setColumn(event.currentTarget.value)}
          >
            <For each={props.columns}>
              {(candidate) => <option value={candidate.name}>{candidate.name}</option>}
            </For>
          </select>
        </label>
        <label class="grid gap-1">
          <span class="text-[10px] font-medium tracking-wide text-ink-3 uppercase">
            Operator
          </span>
          <select
            class="field w-full"
            value={op()}
            onChange={(event) => setOp(event.currentTarget.value as FilterOp)}
          >
            <For each={FILTER_OPS}>
              {(candidate) => (
                <option value={candidate.value}>{candidate.label}</option>
              )}
            </For>
          </select>
        </label>
      </div>

      <Show when={needsValue(op())}>
        <label class="grid gap-1">
          <span class="text-[10px] font-medium tracking-wide text-ink-3 uppercase">
            Value
          </span>
          <input
            class="field w-full font-mono"
            value={value()}
            placeholder="value"
            spellcheck={false}
            onInput={(event) => setValue(event.currentTarget.value)}
          />
        </label>
      </Show>

      <div class="flex items-center justify-end gap-1.5 pt-0.5">
        <Show when={props.onRemove}>
          <button
            type="button"
            class="btn btn-danger mr-auto"
            onClick={() => props.onRemove?.()}
          >
            Remove
          </button>
        </Show>
        <button type="submit" class="btn btn-primary">
          Apply
        </button>
      </div>
    </form>
  );
}

import type { JSX } from "solid-js";
import { ChevronLeft, ChevronRight } from "lucide-solid";

export interface PaginationProps {
  total: number;
  offset: number;
  limit: number;
  onChange: (patch: { offset?: number; limit?: number }) => void;
}

const PAGE_SIZES = [25, 50, 100, 250];

/** The footer: honest row bounds, page size and a jump-to-page box. */
export function Pagination(props: PaginationProps): JSX.Element {
  const pages = () => Math.max(1, Math.ceil(props.total / props.limit));
  const page = () => Math.min(pages(), Math.floor(props.offset / props.limit) + 1);
  const from = () => (props.total === 0 ? 0 : props.offset + 1);
  const to = () => Math.min(props.offset + props.limit, props.total);
  const canPrevious = () => props.offset > 0;
  const canNext = () => props.offset + props.limit < props.total;

  const jump = (
    event: KeyboardEvent & { currentTarget: HTMLInputElement },
  ) => {
    if (event.key !== "Enter") return;
    event.preventDefault();
    const requested = Number.parseInt(event.currentTarget.value, 10);
    if (Number.isFinite(requested)) {
      const target = Math.min(Math.max(1, requested), pages());
      props.onChange({ offset: (target - 1) * props.limit });
      event.currentTarget.value = String(target);
    } else {
      event.currentTarget.value = String(page());
    }
  };

  return (
    <footer class="flex shrink-0 flex-wrap items-center gap-2 border-t border-line bg-panel px-3 py-1.5 text-[11.5px] text-ink-3">
      <span class="font-mono">
        rows {from().toLocaleString()}–{to().toLocaleString()} of{" "}
        {props.total.toLocaleString()}
      </span>

      <span class="mx-0.5 h-4 w-px bg-line" />

      <label class="flex items-center gap-1.5">
        <span class="text-[11px]">Rows</span>
        <select
          class="field h-7 w-[68px] py-0 text-[11.5px]"
          value={String(props.limit)}
          onChange={(event) =>
            props.onChange({
              limit: Number(event.currentTarget.value),
              offset: 0,
            })
          }
          aria-label="Rows per page"
        >
          {PAGE_SIZES.map((size) => (
            <option value={String(size)}>{size}</option>
          ))}
        </select>
      </label>

      <label class="flex items-center gap-1.5">
        <span class="text-[11px]">Go to</span>
        <input
          class="field h-7 w-14 px-1 text-center font-mono text-[11.5px]"
          type="number"
          min="1"
          max={pages()}
          value={page()}
          onKeyDown={jump}
          aria-label="Jump to page"
        />
      </label>

      <div class="ml-auto flex items-center gap-1">
        <button
          type="button"
          class="btn btn-icon h-7 w-7"
          onClick={() =>
            props.onChange({ offset: Math.max(0, props.offset - props.limit) })
          }
          disabled={!canPrevious()}
          aria-label="Previous page"
          title="Previous page"
        >
          <ChevronLeft size={13} />
        </button>
        <span class="px-1 font-mono text-ink-2">
          {page()} / {pages()}
        </span>
        <button
          type="button"
          class="btn btn-icon h-7 w-7"
          onClick={() => props.onChange({ offset: props.offset + props.limit })}
          disabled={!canNext()}
          aria-label="Next page"
          title="Next page"
        >
          <ChevronRight size={13} />
        </button>
      </div>
    </footer>
  );
}

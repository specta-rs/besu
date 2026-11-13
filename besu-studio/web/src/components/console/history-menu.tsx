import { For, Show, createSignal, onCleanup, onMount } from "solid-js";
import { ChevronDown, History } from "lucide-solid";

/**
 * The console's recent statements: a small dropdown beside the run controls,
 * and the same list the editor's `↑`/`↓` recall walks.
 */
export function HistoryMenu(props: {
  history: string[];
  onPick: (sql: string) => void;
  onClear: () => void;
}) {
  const [open, setOpen] = createSignal(false);
  let root: HTMLDivElement | undefined;

  onMount(() => {
    const onPointer = (event: MouseEvent) => {
      if (!open()) return;
      if (root && event.target instanceof Node && !root.contains(event.target)) {
        setOpen(false);
      }
    };
    const onKey = (event: KeyboardEvent) => {
      if (event.key === "Escape") setOpen(false);
    };
    document.addEventListener("mousedown", onPointer);
    document.addEventListener("keydown", onKey);
    onCleanup(() => {
      document.removeEventListener("mousedown", onPointer);
      document.removeEventListener("keydown", onKey);
    });
  });

  return (
    <div class="relative" ref={root}>
      <button
        class="btn btn-ghost"
        onClick={() => setOpen((current) => !current)}
        aria-label="Query history"
        aria-haspopup="menu"
        aria-expanded={open()}
        title="Query history"
      >
        <History size={13} />
        <span class="font-mono text-[11px] text-ink-3">{props.history.length}</span>
        <ChevronDown size={11} class="text-ink-3" />
      </button>

      <Show when={open()}>
        <div
          class="panel absolute top-full right-0 z-30 mt-1 w-[380px] overflow-hidden shadow-[var(--sp-shadow)]"
          role="menu"
          aria-label="Recent statements"
        >
          <div class="flex h-8 items-center justify-between border-b border-line px-2.5">
            <span class="text-[10px] font-medium tracking-wide text-ink-3 uppercase">
              Recent statements
            </span>
            <button
              class="btn btn-ghost h-6 px-1.5 text-[11px]"
              onClick={() => {
                props.onClear();
                setOpen(false);
              }}
              disabled={props.history.length === 0}
            >
              Clear
            </button>
          </div>

          <Show
            when={props.history.length > 0}
            fallback={
              <p class="px-2.5 py-3 text-[12px] text-ink-3">
                Nothing yet — run a statement and it lands here.
              </p>
            }
          >
            <ul class="max-h-[280px] overflow-auto py-1">
              <For each={props.history}>
                {(entry, position) => (
                  <li>
                    <button
                      class="flex w-full items-center gap-2 px-2.5 py-1.5 text-left hover:bg-panel-2"
                      role="menuitem"
                      onClick={() => {
                        props.onPick(entry);
                        setOpen(false);
                      }}
                    >
                      <span class="w-[2ch] shrink-0 text-right font-mono text-[10px] text-ink-3 tabular-nums">
                        {position() + 1}
                      </span>
                      <span class="min-w-0 flex-1 truncate font-mono text-[11.5px] text-ink-2">
                        {entry.replace(/\s+/g, " ")}
                      </span>
                    </button>
                  </li>
                )}
              </For>
            </ul>
          </Show>
        </div>
      </Show>
    </div>
  );
}

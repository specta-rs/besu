import {
  For,
  Match,
  type JSX,
  Show,
  Switch,
  createEffect,
  createMemo,
  createSignal,
  onCleanup,
  onMount,
} from "solid-js";
import { Portal } from "solid-js/web";
import {
  Eye,
  RefreshCw,
  Search,
  Sun,
  Table2,
  Terminal,
  X,
} from "lucide-solid";
import { navigate } from "../../lib/router";
import { useStudio } from "../../lib/store";

export const RECENT_KEY = "besu-studio-recent";
const RECENT_LIMIT = 5;

/** The most recently opened objects, newest first. */
export function readRecent(): string[] {
  try {
    const raw = localStorage.getItem(RECENT_KEY);
    if (!raw) return [];
    const parsed: unknown = JSON.parse(raw);
    if (!Array.isArray(parsed)) return [];
    return parsed.filter((name): name is string => typeof name === "string");
  } catch {
    return [];
  }
}

/** Push a table or view onto the recent list, capped at five. */
export function rememberRecent(name: string): void {
  try {
    const next = [name, ...readRecent().filter((entry) => entry !== name)].slice(
      0,
      RECENT_LIMIT,
    );
    localStorage.setItem(RECENT_KEY, JSON.stringify(next));
  } catch {
    // Private mode or a blocked origin: recents are a nicety, not state.
  }
}

type ItemKind = "table" | "view" | "sql" | "theme" | "refresh";
type Group = "Recent" | "Tables" | "Views" | "Actions";

interface PaletteItem {
  id: string;
  group: Group;
  label: string;
  hint?: string;
  kind: ItemKind;
  run: () => void;
}

function matches(text: string, query: string): boolean {
  return query === "" || text.toLowerCase().includes(query);
}

function ItemIcon(props: { kind: ItemKind; active: boolean }) {
  const frame = () =>
    `flex size-6 shrink-0 items-center justify-center rounded-md ${
      props.active ? "bg-accent-soft text-accent" : "bg-panel-2 text-ink-3"
    }`;

  return (
    <span class={frame()} aria-hidden="true">
      <Switch fallback={<Terminal size={13} />}>
        <Match when={props.kind === "table"}>
          <Table2 size={13} />
        </Match>
        <Match when={props.kind === "view"}>
          <Eye size={13} />
        </Match>
        <Match when={props.kind === "theme"}>
          <Sun size={13} />
        </Match>
        <Match when={props.kind === "refresh"}>
          <RefreshCw size={13} />
        </Match>
      </Switch>
    </span>
  );
}

/**
 * ⌘K palette: jump to a table or view, open the SQL console, toggle the theme
 * or refresh the schema. Fully keyboard driven; opens from anywhere.
 */
export function CommandPalette(): JSX.Element {
  const studio = useStudio();
  const [query, setQuery] = createSignal("");
  const [index, setIndex] = createSignal(0);
  const [recent, setRecent] = createSignal<string[]>([]);

  let inputEl: HTMLInputElement | undefined;
  let listEl: HTMLDivElement | undefined;

  const close = () => studio.setPaletteOpen(false);

  const runObject = (name: string, kind: "table" | "view") => {
    rememberRecent(name);
    navigate(
      kind === "view" ? { name: "schema", table: name } : { name: "data", table: name },
    );
    close();
  };

  const items = createMemo<PaletteItem[]>(() => {
    const q = query().trim().toLowerCase();
    const schema = studio.schema();
    const tables = schema?.tables ?? [];
    const views = schema?.views ?? [];

    const known = new Map<string, "table" | "view">();
    for (const table of tables) known.set(table.name, "table");
    for (const view of views) {
      if (!known.has(view.name)) known.set(view.name, "view");
    }

    const result: PaletteItem[] = [];
    const shown = new Set<string>();

    for (const name of recent()) {
      const kind = known.get(name);
      if (!kind || shown.has(name) || !matches(name, q)) continue;
      shown.add(name);
      result.push({
        id: `recent:${name}`,
        group: "Recent",
        label: name,
        hint: "recent",
        kind,
        run: () => runObject(name, kind),
      });
    }

    for (const table of tables) {
      if (shown.has(table.name) || !matches(table.name, q)) continue;
      shown.add(table.name);
      result.push({
        id: `table:${table.name}`,
        group: "Tables",
        label: table.name,
        hint: `${table.columns.length} cols`,
        kind: "table",
        run: () => runObject(table.name, "table"),
      });
    }

    for (const view of views) {
      if (shown.has(view.name) || !matches(view.name, q)) continue;
      shown.add(view.name);
      result.push({
        id: `view:${view.name}`,
        group: "Views",
        label: view.name,
        hint: view.materialised ? "mat. view" : "view",
        kind: "view",
        run: () => runObject(view.name, "view"),
      });
    }

    const actions: PaletteItem[] = [
      {
        id: "action:sql",
        group: "Actions",
        label: "Open SQL console",
        hint: "⌘↵",
        kind: "sql",
        run: () => {
          close();
          navigate("/sql");
        },
      },
      {
        id: "action:theme",
        group: "Actions",
        label: "Toggle theme",
        hint: studio.theme() === "dark" ? "to light" : "to dark",
        kind: "theme",
        run: () => {
          close();
          studio.toggleTheme();
        },
      },
      {
        id: "action:refresh",
        group: "Actions",
        label: "Refresh schema",
        hint: "reload",
        kind: "refresh",
        run: () => {
          close();
          studio.refresh();
          studio.toast("Refreshing schema…", "info");
        },
      },
    ];

    for (const action of actions) {
      if (matches(action.label, q)) result.push(action);
    }

    return result;
  });

  const runItem = (item: PaletteItem | undefined) => item?.run();

  const step = (delta: number) => {
    const count = items().length;
    if (count === 0) return;
    setIndex((current) => (current + delta + count) % count);
  };

  const onInputKey = (event: KeyboardEvent) => {
    if (event.key === "ArrowDown") {
      event.preventDefault();
      step(1);
    } else if (event.key === "ArrowUp") {
      event.preventDefault();
      step(-1);
    } else if (event.key === "Enter") {
      event.preventDefault();
      runItem(items()[index()]);
    } else if (event.key === "Tab") {
      // The palette owns focus while it is open; Tab stays in the search box.
      event.preventDefault();
    } else if (event.key === "Escape") {
      event.preventDefault();
      close();
    }
  };

  // Every open starts clean, with fresh recents and focus in the search box.
  createEffect(() => {
    if (!studio.paletteOpen()) return;
    setRecent(readRecent());
    setQuery("");
    setIndex(0);
    queueMicrotask(() => inputEl?.focus());
  });

  // Keep the arrow-key selection inside the scroll viewport.
  createEffect(() => {
    const current = index();
    if (!studio.paletteOpen() || !listEl) return;
    listEl
      .querySelector<HTMLElement>(`[data-index="${current}"]`)
      ?.scrollIntoView({ block: "nearest" });
  });

  onMount(() => {
    const onKey = (event: KeyboardEvent) => {
      if (studio.paletteOpen() && event.key === "Escape") {
        event.preventDefault();
        close();
      }
    };
    window.addEventListener("keydown", onKey);
    onCleanup(() => window.removeEventListener("keydown", onKey));
  });

  return (
    <Show when={studio.paletteOpen()}>
      <Portal>
        <div
          class="fixed inset-0 z-50 flex items-start justify-center bg-black/55 p-4 pt-[12vh] backdrop-blur-[2px]"
          onMouseDown={(event) => {
            if (event.target === event.currentTarget) close();
          }}
        >
          <div
            class="panel w-full max-w-[560px] overflow-hidden shadow-[var(--sp-shadow)]"
            role="dialog"
            aria-modal="true"
            aria-label="Command palette"
            onMouseDown={(event) => event.stopPropagation()}
          >
            <div class="flex items-center gap-2 border-b border-line px-2.5 focus-within:shadow-[inset_0_-2px_0_var(--sp-accent)]">
              <Search
                size={14}
                class="shrink-0 text-ink-3"
                aria-hidden="true"
              />
              <input
                ref={inputEl}
                class="h-11 w-full min-w-0 bg-transparent text-[13px] text-ink placeholder:text-ink-3"
                style={{ outline: "none" }}
                type="text"
                spellcheck={false}
                autocomplete="off"
                placeholder="Search tables, views and actions…"
                aria-label="Search tables, views and actions"
                role="combobox"
                aria-expanded="true"
                aria-controls="besu-palette-list"
                aria-autocomplete="list"
                aria-activedescendant={
                  items().length > 0 ? `besu-palette-option-${index()}` : undefined
                }
                value={query()}
                onInput={(event) => {
                  setQuery(event.currentTarget.value);
                  setIndex(0);
                }}
                onKeyDown={onInputKey}
              />
              <button
                class="btn btn-ghost btn-icon shrink-0"
                onClick={close}
                aria-label="Close command palette"
              >
                <X size={14} aria-hidden="true" />
              </button>
            </div>

            <div
              ref={listEl}
              id="besu-palette-list"
              class="max-h-[46vh] min-h-[200px] overflow-y-auto overscroll-contain px-1.5 py-1.5"
              role="listbox"
              aria-label="Command palette results"
            >
              <Show
                when={items().length > 0}
                fallback={
                  <p class="px-3 py-12 text-center text-[12px] leading-relaxed text-ink-3">
                    No matches for “{query()}”.
                  </p>
                }
              >
                <For each={items()}>
                  {(item, position) => (
                    <>
                      <Show
                        when={
                          position() === 0 ||
                          items()[position() - 1]?.group !== item.group
                        }
                      >
                        <div class="px-2 pt-2 pb-1 text-[10px] font-semibold uppercase tracking-[0.09em] text-ink-3">
                          {item.group}
                        </div>
                      </Show>
                      <div
                        id={`besu-palette-option-${position()}`}
                        data-index={position()}
                        role="option"
                        aria-selected={index() === position()}
                        class={`flex h-8 cursor-pointer items-center gap-2 rounded-md px-1.5 text-left ${
                          index() === position()
                            ? "bg-accent-soft text-ink"
                            : "text-ink-2 hover:bg-panel-2 hover:text-ink"
                        }`}
                        onMouseDown={(event) => {
                          event.preventDefault();
                          setIndex(position());
                          runItem(item);
                        }}
                        onMouseMove={() => setIndex(position())}
                      >
                        <ItemIcon
                          kind={item.kind}
                          active={index() === position()}
                        />
                        <span class="min-w-0 flex-1 truncate text-[12.5px]">
                          {item.label}
                        </span>
                        <Show when={item.hint}>
                          <span class="shrink-0 font-mono text-[10px] text-ink-3">
                            {item.hint}
                          </span>
                        </Show>
                      </div>
                    </>
                  )}
                </For>
              </Show>
            </div>

            <div class="flex items-center gap-3 border-t border-line bg-panel-2 px-3 py-2 text-[10.5px] text-ink-3">
              <span class="flex items-center gap-1">
                <span class="kbd">↑</span>
                <span class="kbd">↓</span>
                navigate
              </span>
              <span class="flex items-center gap-1">
                <span class="kbd">↵</span>
                open
              </span>
              <span class="ml-auto flex items-center gap-1">
                <span class="kbd">esc</span>
                close
              </span>
            </div>
          </div>
        </div>
      </Portal>
    </Show>
  );
}

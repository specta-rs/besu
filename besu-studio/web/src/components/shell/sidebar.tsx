import {
  For,
  type JSX,
  Show,
  createEffect,
  createMemo,
  createSignal,
  onCleanup,
  onMount,
} from "solid-js";
import {
  Database,
  Eye,
  Moon,
  Search,
  Sun,
  Table2,
  Terminal,
} from "lucide-solid";
import { href, linkClick, navigate, route } from "../../lib/router";
import type { Route } from "../../lib/router";
import { useStudio } from "../../lib/store";

export interface SidebarProps {
  /** Drawer state. Only meaningful below 900px; the sidebar is always open above it. */
  open: boolean;
  onClose: () => void;
}

interface Entry {
  name: string;
  view: boolean;
}

/** The persistent object list: connection card, live filter, tables, views, nav. */
export function Sidebar(props: SidebarProps): JSX.Element {
  const studio = useStudio();
  const [query, setQuery] = createSignal("");
  const [narrow, setNarrow] = createSignal(false);

  let listEl: HTMLDivElement | undefined;
  let asideEl: HTMLElement | undefined;

  const tables = () => studio.schema()?.tables ?? [];
  const views = () => studio.schema()?.views ?? [];
  const database = () => studio.info()?.database ?? "besu studio";
  const dialect = () => {
    if (studio.info.loading) return "connecting…";
    return studio.info()?.dialect ?? "not connected";
  };

  const needle = () => query().trim().toLowerCase();

  const filteredTables = createMemo(() => {
    const q = needle();
    return q
      ? tables().filter((table) => table.name.toLowerCase().includes(q))
      : tables();
  });

  const filteredViews = createMemo(() => {
    const q = needle();
    return q
      ? views().filter((view) => view.name.toLowerCase().includes(q))
      : views();
  });

  const entries = createMemo<Entry[]>(() => [
    ...filteredTables().map((table) => ({ name: table.name, view: false })),
    ...filteredViews().map((view) => ({ name: view.name, view: true })),
  ]);

  /** The object the current route is showing, if any. */
  const activeName = createMemo(() => {
    const current = route();
    return current.name === "data" || current.name === "schema"
      ? current.table
      : null;
  });

  /**
   * A schema route is a view when the schema says so; without a loaded schema
   * it is safest to treat it as a table (the route is shared).
   */
  const activeKind = createMemo<"table" | "view" | null>(() => {
    const current = route();
    if (current.name === "data") return "table";
    if (current.name !== "schema") return null;
    return views().some((view) => view.name === current.table)
      ? "view"
      : "table";
  });

  const isActive = (entry: Entry) =>
    activeName() === entry.name &&
    (entry.view ? activeKind() === "view" : activeKind() === "table");

  const move = (delta: number) => {
    const all = entries();
    if (all.length === 0) return;

    const current = activeName();
    const kind = activeKind();
    const index = all.findIndex(
      (entry) =>
        entry.name === current && entry.view === (kind === "view"),
    );
    const nextIndex =
      index === -1
        ? delta > 0
          ? 0
          : all.length - 1
        : (index + delta + all.length) % all.length;
    const next = all[nextIndex];
    if (!next) return;

    navigate(
      next.view
        ? { name: "schema", table: next.name }
        : { name: "data", table: next.name },
    );
  };

  const onListKey = (event: KeyboardEvent) => {
    if (event.metaKey || event.ctrlKey || event.altKey) return;
    if (event.key === "ArrowDown") {
      event.preventDefault();
      move(1);
    } else if (event.key === "ArrowUp") {
      event.preventDefault();
      move(-1);
    }
  };

  const onSearchKey = (
    event: KeyboardEvent & { currentTarget: HTMLInputElement },
  ) => {
    if (event.key === "ArrowDown" || event.key === "ArrowUp") {
      event.preventDefault();
      move(event.key === "ArrowDown" ? 1 : -1);
    } else if (event.key === "Escape") {
      if (query()) {
        event.preventDefault();
        setQuery("");
      } else {
        event.currentTarget.blur();
      }
    }
  };

  // Modifier-clicks keep working like browser links; either way, close the
  // drawer so the destination is visible on small screens.
  const follow = (event: MouseEvent, to: string | Route) => {
    linkClick(event, to);
    props.onClose();
  };

  // Keep the active object visible as the route changes or the filter narrows.
  createEffect(() => {
    const name = activeName();
    const kind = activeKind();
    if (!name || !kind || !listEl) return;
    const active = listEl.querySelector<HTMLElement>(
      `[data-name="${CSS.escape(name)}"][data-kind="${kind}"]`,
    );
    active?.scrollIntoView({ block: "nearest" });
  });

  // Off-screen drawers must not hold keyboard focus.
  createEffect(() => {
    asideEl?.toggleAttribute("inert", narrow() && !props.open);
  });

  onMount(() => {
    const media = window.matchMedia("(max-width: 899px)");
    setNarrow(media.matches);

    const onChange = () => setNarrow(media.matches);
    const onKey = (event: KeyboardEvent) => {
      if (event.key === "Escape" && props.open && media.matches) {
        props.onClose();
      }
    };

    media.addEventListener("change", onChange);
    window.addEventListener("keydown", onKey);
    onCleanup(() => {
      media.removeEventListener("change", onChange);
      window.removeEventListener("keydown", onKey);
    });
  });

  const rowClass = (active: boolean) =>
    `group relative flex h-[26px] w-full items-center gap-2 rounded-md pl-3 pr-2 text-left text-[12.5px] transition-colors ${
      active
        ? "bg-accent-soft text-ink"
        : "text-ink-2 hover:bg-panel-2 hover:text-ink"
    }`;

  return (
    <>
      <Show when={props.open}>
        <div
          class="fixed inset-0 z-30 bg-black/40 backdrop-blur-[1px] min-[900px]:hidden"
          onMouseDown={props.onClose}
          aria-hidden="true"
        />
      </Show>

      <aside
        ref={asideEl}
        class={`flex h-full w-[248px] shrink-0 flex-col border-r border-line bg-panel max-[900px]:fixed max-[900px]:inset-y-0 max-[900px]:left-0 max-[900px]:z-40 max-[900px]:w-[272px] max-[900px]:shadow-[var(--sp-shadow)] max-[900px]:transition-transform max-[900px]:duration-200 ${
          props.open
            ? "max-[900px]:translate-x-0"
            : "max-[900px]:-translate-x-full"
        }`}
        aria-label="Database objects"
      >
        <div class="flex shrink-0 items-center gap-2.5 border-b border-line px-3 py-2.5">
          <div class="flex size-7 shrink-0 items-center justify-center rounded-md bg-accent-soft text-accent">
            <Database size={14} aria-hidden="true" />
          </div>
          <div class="min-w-0 flex-1">
            <div class="flex min-w-0 items-center gap-1.5">
              <span
                class="truncate text-[12.5px] font-semibold tracking-[-0.01em]"
                title={database()}
              >
                {database()}
              </span>
              <Show when={studio.isDemo}>
                <span class="shrink-0 rounded-full bg-accent-soft px-1.5 py-px text-[9px] font-semibold uppercase tracking-[0.08em] text-accent">
                  demo
                </span>
              </Show>
            </div>
            <p class="truncate text-[11px] text-ink-3">{dialect()}</p>
          </div>
        </div>

        <div class="shrink-0 px-2 pt-2 pb-1">
          <div class="relative">
            <Search
              size={13}
              class="pointer-events-none absolute top-1/2 left-2.5 -translate-y-1/2 text-ink-3"
              aria-hidden="true"
            />
            <input
              id="besu-sidebar-search"
              class="field w-full pr-12 pl-7"
              type="text"
              spellcheck={false}
              autocomplete="off"
              placeholder="Filter tables"
              aria-label="Filter tables and views"
              value={query()}
              onInput={(event) => setQuery(event.currentTarget.value)}
              onKeyDown={onSearchKey}
            />
            <span
              class="kbd pointer-events-none absolute top-1/2 right-1.5 -translate-y-1/2"
              aria-hidden="true"
            >
              ⌘K
            </span>
          </div>
        </div>

        <div
          ref={listEl}
          class="min-h-0 flex-1 overflow-y-auto overscroll-contain px-1.5 pb-2"
          role="navigation"
          aria-label="Tables and views"
          tabIndex={0}
          onKeyDown={onListKey}
        >
          <Show when={entries().length === 0}>
            <p class="px-2 py-3 text-[11.5px] leading-relaxed text-ink-3">
              {studio.schema.loading
                ? "Loading schema…"
                : needle()
                  ? "No matches."
                  : "No tables or views. Is the Besu API running?"}
            </p>
          </Show>

          <Show when={filteredTables().length > 0}>
            <div class="flex h-7 items-center justify-between px-2">
              <span class="text-[10px] font-semibold uppercase tracking-[0.09em] text-ink-3">
                Tables
              </span>
              <span class="font-mono text-[10px] text-ink-3">
                {filteredTables().length}
              </span>
            </div>
            <For each={filteredTables()}>
              {(table) => {
                const active = () => isActive({ name: table.name, view: false });
                return (
                  <a
                    class={rowClass(active())}
                    href={href({ name: "data", table: table.name })}
                    onClick={(event) =>
                      follow(event, { name: "data", table: table.name })
                    }
                    aria-current={active() ? "page" : undefined}
                    data-name={table.name}
                    data-kind="table"
                  >
                    <Show when={active()}>
                      <span
                        class="absolute inset-y-[3px] left-0 w-[2px] rounded-r-full bg-accent"
                        aria-hidden="true"
                      />
                    </Show>
                    <Table2
                      size={13}
                      class={`shrink-0 ${active() ? "text-accent" : "text-ink-3"}`}
                      aria-hidden="true"
                    />
                    <span class="min-w-0 flex-1 truncate">{table.name}</span>
                    <span
                      class="shrink-0 font-mono text-[10px] tabular-nums text-ink-3"
                      title={`${table.columns.length} columns`}
                    >
                      {table.columns.length}
                    </span>
                  </a>
                );
              }}
            </For>
          </Show>

          <Show when={filteredViews().length > 0}>
            <div class="flex h-7 items-center justify-between px-2 pt-1">
              <span class="text-[10px] font-semibold uppercase tracking-[0.09em] text-ink-3">
                Views
              </span>
              <span class="font-mono text-[10px] text-ink-3">
                {filteredViews().length}
              </span>
            </div>
            <For each={filteredViews()}>
              {(view) => {
                const active = () => isActive({ name: view.name, view: true });
                return (
                  <a
                    class={rowClass(active())}
                    href={href({ name: "schema", table: view.name })}
                    onClick={(event) =>
                      follow(event, { name: "schema", table: view.name })
                    }
                    aria-current={active() ? "page" : undefined}
                    title={view.definition}
                    data-name={view.name}
                    data-kind="view"
                  >
                    <Show when={active()}>
                      <span
                        class="absolute inset-y-[3px] left-0 w-[2px] rounded-r-full bg-accent"
                        aria-hidden="true"
                      />
                    </Show>
                    <Eye
                      size={13}
                      class={`shrink-0 ${active() ? "text-accent" : "text-ink-3"}`}
                      aria-hidden="true"
                    />
                    <span class="min-w-0 flex-1 truncate">{view.name}</span>
                    <span class="shrink-0 font-mono text-[10px] text-ink-3">
                      view
                    </span>
                  </a>
                );
              }}
            </For>
          </Show>
        </div>

        <div class="shrink-0 border-t border-line p-1.5">
          <a
            class={`flex h-7 items-center gap-2 rounded-md px-2 text-[12px] transition-colors ${
              route().name === "sql"
                ? "bg-accent-soft text-ink"
                : "text-ink-2 hover:bg-panel-2 hover:text-ink"
            }`}
            href="/sql"
            onClick={(event) => follow(event, "/sql")}
            aria-current={route().name === "sql" ? "page" : undefined}
          >
            <Terminal size={13} aria-hidden="true" />
            <span class="flex-1 truncate">SQL console</span>
            <span class="kbd" aria-hidden="true">
              ⌘↵
            </span>
          </a>
          <button
            class="flex h-7 w-full items-center gap-2 rounded-md px-2 text-[12px] text-ink-2 transition-colors hover:bg-panel-2 hover:text-ink"
            onClick={studio.toggleTheme}
            aria-label={`Switch to ${
              studio.theme() === "dark" ? "light" : "dark"
            } theme`}
          >
            <Show
              when={studio.theme() === "dark"}
              fallback={<Moon size={13} aria-hidden="true" />}
            >
              <Sun size={13} aria-hidden="true" />
            </Show>
            <span class="flex-1 truncate text-left">Appearance</span>
            <span class="text-[11px] text-ink-3">
              {studio.theme() === "dark" ? "Dark" : "Light"}
            </span>
          </button>
        </div>
      </aside>
    </>
  );
}

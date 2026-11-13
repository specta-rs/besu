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
  ChevronRight,
  Database,
  Eye,
  Info,
  Play,
  RefreshCw,
  Table2,
} from "lucide-solid";
import { CommandPalette, rememberRecent } from "./components/shell/palette";
import { Sidebar } from "./components/shell/sidebar";
import { Toasts } from "./components/shell/toasts";
import { TopBar } from "./components/shell/topbar";
import { DataPage } from "./pages/data-page";
import { SchemaPage } from "./pages/schema-page";
import { SqlPage } from "./pages/sql-page";
import { href, linkClick, navigate, route } from "./lib/router";
import { createStudioState, StudioProvider, useStudio } from "./lib/store";
import type { Table } from "./lib/types";
import { Spinner } from "./lib/ui";

const primaryKey = (table: Table): string[] =>
  table.columns
    .filter((column) => column.primary_key)
    .map((column) => column.name);

function Stat(props: { label: string; value: string; title?: string }) {
  return (
    <div class="panel min-w-0 px-3 py-2">
      <p class="text-[10px] font-semibold uppercase tracking-[0.09em] text-ink-3">
        {props.label}
      </p>
      <p
        class="mt-0.5 truncate font-mono text-[12.5px] text-ink"
        title={props.title ?? props.value}
      >
        {props.value}
      </p>
    </div>
  );
}

function Welcome(): JSX.Element {
  const studio = useStudio();

  const tables = () => studio.schema()?.tables ?? [];
  const database = () => studio.info()?.database ?? "besu studio";
  const dialect = () => studio.info()?.dialect ?? "unknown";
  const version = () => studio.info()?.version ?? "—";
  const totalColumns = () =>
    tables().reduce((sum, table) => sum + table.columns.length, 0);

  const openSql = () => navigate("/sql");

  return (
    <div class="h-full overflow-y-auto">
      <div class="mx-auto w-full max-w-[1000px] px-6 py-8 sm:px-8">
        <Show when={studio.isDemo}>
          <section
            class="mb-6 flex items-start gap-3 rounded-[10px] border border-accent/35 bg-accent-soft px-4 py-3"
            aria-label="Demo notice"
          >
            <Info size={15} class="mt-px shrink-0 text-accent" aria-hidden="true" />
            <div class="min-w-0">
              <p class="text-[12.5px] font-semibold">
                Hosted demo — read-only fixture data
              </p>
              <p class="mt-0.5 text-[12px] leading-relaxed text-ink-2">
                Every table and row on this page is a fixture bundled with the
                demo; changes never leave your browser. Run{" "}
                <code class="rounded bg-panel-2 px-1 py-0.5 font-mono text-[11px]">
                  besu studio --database …
                </code>{" "}
                locally to browse your own database, write rows and run SQL.
              </p>
            </div>
          </section>
        </Show>

        <header class="flex flex-wrap items-start justify-between gap-4">
          <div class="min-w-0">
            <p class="flex items-center gap-1.5 text-[10.5px] font-semibold uppercase tracking-[0.09em] text-ink-3">
              <Database size={12} aria-hidden="true" />
              Connection
              <Show when={studio.info()?.read_only}>
                <span class="rounded-full bg-panel-2 px-1.5 py-px text-[9px] normal-case tracking-normal text-ink-3">
                  read-only
                </span>
              </Show>
            </p>
            <h1
              class="mt-1 truncate text-[22px] font-semibold tracking-[-0.02em]"
              title={database()}
            >
              {database()}
            </h1>
            <p class="mt-1 max-w-[580px] text-[12.5px] leading-relaxed text-ink-2">
              A browser for your Besu database. Pick a table to edit rows, open{" "}
              <span class="text-ink">Structure</span> to inspect its schema, or
              run a query in the SQL console.
            </p>
          </div>
          <div class="flex flex-wrap items-center gap-2">
            <button class="btn" onClick={() => studio.refresh()}>
              <RefreshCw
                size={13}
                class={studio.schema.loading ? "animate-spin" : undefined}
                aria-hidden="true"
              />
              Refresh
            </button>
            <button class="btn btn-primary" onClick={openSql}>
              <Play size={13} aria-hidden="true" />
              SQL console
            </button>
          </div>
        </header>

        <div class="mt-5 grid grid-cols-2 gap-2 sm:grid-cols-4">
          <Stat label="Dialect" value={dialect()} />
          <Stat label="Version" value={version()} />
          <Stat label="Tables" value={String(tables().length)} />
          <Stat label="Columns" value={String(totalColumns())} />
        </div>

        <Show when={studio.schema.loading && !studio.schema()}>
          <div class="mt-6 flex items-center gap-2.5 rounded-[10px] border border-line bg-panel px-4 py-6">
            <Spinner size={14} />
            <p class="text-[12px] text-ink-2">Reading the schema…</p>
          </div>
        </Show>

        <Show when={!studio.schema.loading && studio.schema() === null}>
          <div class="mt-6 rounded-[10px] border border-line bg-panel px-4 py-5">
            <p class="text-[12.5px] font-medium">No schema loaded</p>
            <p class="mt-1 max-w-[580px] text-[12px] leading-relaxed text-ink-2">
              The studio could not reach the Besu API. Start a server with{" "}
              <code class="rounded bg-panel-2 px-1 py-0.5 font-mono text-[11px]">
                besu studio --database …
              </code>{" "}
              and try again, or open this page from a running studio.
            </p>
            <button class="btn mt-3" onClick={() => studio.refresh()}>
              <RefreshCw size={13} aria-hidden="true" />
              Try again
            </button>
          </div>
        </Show>

        <Show when={studio.schema()}>
          {(schema) => (
            <>
              <section class="mt-8" aria-labelledby="welcome-tables">
                <div class="flex items-center justify-between">
                  <h2
                    id="welcome-tables"
                    class="text-[10.5px] font-semibold uppercase tracking-[0.09em] text-ink-3"
                  >
                    Tables
                  </h2>
                  <span class="font-mono text-[11px] text-ink-3">
                    {schema().tables.length}
                  </span>
                </div>

                <Show
                  when={schema().tables.length > 0}
                  fallback={
                    <p class="mt-2 text-[12px] text-ink-3">
                      No tables in this database.
                    </p>
                  }
                >
                  <div class="mt-2 grid gap-2 sm:grid-cols-2 lg:grid-cols-3">
                    <For each={schema().tables}>
                      {(table) => {
                        const pk = () => primaryKey(table);
                        return (
                          <a
                            class="panel group flex min-w-0 flex-col gap-1.5 px-3 py-2.5 transition-colors hover:border-line-strong hover:bg-panel-2"
                            href={href({ name: "data", table: table.name })}
                            onClick={(event) =>
                              linkClick(event, {
                                name: "data",
                                table: table.name,
                              })
                            }
                          >
                            <span class="flex min-w-0 items-center gap-2">
                              <Table2
                                size={13}
                                class="shrink-0 text-ink-3 group-hover:text-accent"
                                aria-hidden="true"
                              />
                              <span class="min-w-0 truncate text-[12.5px] font-medium">
                                {table.name}
                              </span>
                              <ChevronRight
                                size={12}
                                class="ml-auto shrink-0 text-ink-3 opacity-0 transition-opacity group-hover:opacity-100"
                                aria-hidden="true"
                              />
                            </span>
                            <span class="truncate font-mono text-[10.5px] text-ink-3">
                              {table.columns.length}{" "}
                              {table.columns.length === 1 ? "column" : "columns"}
                              <Show when={pk().length > 0}>
                                {" · "}pk {pk().join(", ")}
                              </Show>
                            </span>
                          </a>
                        );
                      }}
                    </For>
                  </div>
                </Show>
              </section>

              <Show when={(schema().views ?? []).length > 0}>
                <section class="mt-8" aria-labelledby="welcome-views">
                  <div class="flex items-center justify-between">
                    <h2
                      id="welcome-views"
                      class="text-[10.5px] font-semibold uppercase tracking-[0.09em] text-ink-3"
                    >
                      Views
                    </h2>
                    <span class="font-mono text-[11px] text-ink-3">
                      {(schema().views ?? []).length}
                    </span>
                  </div>
                  <div class="mt-2 grid gap-2 sm:grid-cols-2 lg:grid-cols-3">
                    <For each={schema().views ?? []}>
                      {(view) => (
                        <a
                          class="panel group flex min-w-0 flex-col gap-1.5 px-3 py-2.5 transition-colors hover:border-line-strong hover:bg-panel-2"
                          href={href({ name: "schema", table: view.name })}
                          onClick={(event) =>
                            linkClick(event, {
                              name: "schema",
                              table: view.name,
                            })
                          }
                          title={view.definition}
                        >
                          <span class="flex min-w-0 items-center gap-2">
                            <Eye
                              size={13}
                              class="shrink-0 text-ink-3 group-hover:text-accent"
                              aria-hidden="true"
                            />
                            <span class="min-w-0 truncate text-[12.5px] font-medium">
                              {view.name}
                            </span>
                            <ChevronRight
                              size={12}
                              class="ml-auto shrink-0 text-ink-3 opacity-0 transition-opacity group-hover:opacity-100"
                              aria-hidden="true"
                            />
                          </span>
                          <span class="truncate font-mono text-[10.5px] text-ink-3">
                            {view.materialised ? "materialised view" : "view"}
                          </span>
                        </a>
                      )}
                    </For>
                  </div>
                </section>
              </Show>
            </>
          )}
        </Show>

        <p class="mt-8 flex flex-wrap items-center gap-x-3 gap-y-1 text-[11px] text-ink-3">
          <span class="flex items-center gap-1">
            <span class="kbd">⌘K</span> jump anywhere
          </span>
          <span class="flex items-center gap-1">
            <span class="kbd">/</span> filter tables
          </span>
          <span class="flex items-center gap-1">
            <span class="kbd">↵</span> open
          </span>
        </p>
      </div>
    </div>
  );
}

function Studio(): JSX.Element {
  const studio = useStudio();
  const [sidebarOpen, setSidebarOpen] = createSignal(false);

  // ⌘K opens the palette from anywhere, including inside an editor; `/` jumps
  // to the object filter when nothing editable has focus.
  onMount(() => {
    const onKey = (event: KeyboardEvent) => {
      if ((event.metaKey || event.ctrlKey) && event.key.toLowerCase() === "k") {
        event.preventDefault();
        studio.setPaletteOpen(!studio.paletteOpen());
        return;
      }

      if (
        event.key !== "/" ||
        event.metaKey ||
        event.ctrlKey ||
        event.altKey ||
        event.defaultPrevented
      ) {
        return;
      }

      const target = event.target;
      if (
        target instanceof HTMLElement &&
        target.closest("input, textarea, select, [contenteditable='true']")
      ) {
        return;
      }

      event.preventDefault();
      setSidebarOpen(true);
      requestAnimationFrame(() => {
        document.getElementById("besu-sidebar-search")?.focus();
      });
    };

    window.addEventListener("keydown", onKey);
    onCleanup(() => window.removeEventListener("keydown", onKey));
  });

  createEffect(() => {
    const current = route();

    if (current.name === "data" || current.name === "schema") {
      studio.selectTable(current.table);
      rememberRecent(current.table);
    }

    // A navigation always reveals the destination, even from the drawer.
    setSidebarOpen(false);
  });

  const dataRoute = createMemo(() => {
    const current = route();
    return current.name === "data" ? current : undefined;
  });
  const schemaRoute = createMemo(() => {
    const current = route();
    return current.name === "schema" ? current : undefined;
  });

  return (
    <div class="flex h-screen overflow-hidden bg-bg text-ink">
      <Sidebar
        open={sidebarOpen()}
        onClose={() => setSidebarOpen(false)}
      />
      <div class="flex min-w-0 flex-1 flex-col">
        <TopBar onToggleSidebar={() => setSidebarOpen((open) => !open)} />
        <main class="min-h-0 flex-1 overflow-hidden">
          <Show when={route().name === "sql"}>
            <SqlPage />
          </Show>
          <Show when={dataRoute()}>
            {(current) => <DataPage table={current().table} />}
          </Show>
          <Show when={schemaRoute()}>
            {(current) => <SchemaPage table={current().table} />}
          </Show>
          <Show when={route().name === "welcome"}>
            <Welcome />
          </Show>
        </main>
      </div>
      <CommandPalette />
      <Toasts />
    </div>
  );
}

export function App() {
  const studio = createStudioState();

  return (
    <StudioProvider value={studio}>
      <Studio />
    </StudioProvider>
  );
}

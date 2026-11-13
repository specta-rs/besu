import { type JSX, Show, createMemo } from "solid-js";
import {
  ChevronRight,
  Menu,
  Moon,
  RefreshCw,
  Search,
  Sun,
} from "lucide-solid";
import { href, linkClick, route } from "../../lib/router";
import { useStudio } from "../../lib/store";
import { Spinner } from "../../lib/ui";

export interface TopBarProps {
  onToggleSidebar: () => void;
}

/**
 * The 44px bar above the page: breadcrumb, table/view switch, refresh, theme
 * and the command-palette affordance.
 */
export function TopBar(props: TopBarProps): JSX.Element {
  const studio = useStudio();

  const database = () => studio.info()?.database ?? "besu studio";

  const tableName = createMemo(() => {
    const current = route();
    return current.name === "data" || current.name === "schema"
      ? current.table
      : null;
  });

  const tableKind = createMemo<"table" | "view" | null>(() => {
    const current = route();
    if (current.name !== "data" && current.name !== "schema") return null;
    const isView = (studio.schema()?.views ?? []).some(
      (view) => view.name === current.table,
    );
    return isView ? "view" : "table";
  });

  const isRows = () => route().name === "data";
  const isStructure = () => route().name === "schema";
  const compactCrumb = () =>
    tableName() !== null || route().name === "sql";

  const segment = (active: boolean) =>
    `inline-flex h-6 items-center rounded-md border px-2 text-[11.5px] font-medium transition-colors ${
      active
        ? "border-line bg-panel text-ink"
        : "border-transparent text-ink-2 hover:text-ink"
    }`;

  return (
    <header class="flex h-11 shrink-0 items-center gap-2 border-b border-line bg-panel px-2.5">
      <button
        class="btn btn-ghost btn-icon min-[900px]:hidden"
        onClick={props.onToggleSidebar}
        aria-label="Toggle object list"
      >
        <Menu size={15} aria-hidden="true" />
      </button>

      <div class="flex min-w-0 flex-1 items-center gap-1 text-[12px]">
        <a
          class={`max-w-[45%] truncate rounded px-1 py-0.5 text-ink-2 hover:text-ink ${
            compactCrumb() ? "max-[640px]:hidden" : ""
          }`}
          href="/"
          onClick={(event) => linkClick(event, "/")}
          title={database()}
        >
          {database()}
        </a>
        <Show when={tableName() || route().name === "sql"}>
          <ChevronRight
            size={12}
            class={`shrink-0 text-ink-3 ${
              compactCrumb() ? "max-[640px]:hidden" : ""
            }`}
            aria-hidden="true"
          />
        </Show>
        <Show when={tableName()}>
          {(name) => (
            <>
              <span class="min-w-0 truncate font-medium text-ink">{name()}</span>
              <Show when={tableKind()}>
                {(kind) => (
                  <span class="shrink-0 rounded border border-line bg-panel-2 px-1.5 text-[9.5px] font-medium uppercase tracking-[0.06em] text-ink-3">
                    {kind()}
                  </span>
                )}
              </Show>
            </>
          )}
        </Show>
        <Show when={route().name === "sql"}>
          <span class="truncate font-medium text-ink">SQL console</span>
        </Show>
      </div>

      <div class="flex shrink-0 items-center gap-1.5">
        <span
          class="flex items-center justify-center"
          role="status"
          aria-live="polite"
        >
          <Show when={studio.schema.loading}>
            <Spinner size={12} />
            <span class="sr-only">Loading schema</span>
          </Show>
        </span>

        <Show when={tableName()}>
          {(name) => (
            <div
              class="flex items-center gap-0.5 rounded-lg border border-line bg-panel-2 p-0.5"
              role="group"
              aria-label="Table view"
            >
              <a
                class={segment(isRows())}
                href={href({ name: "data", table: name() })}
                onClick={(event) =>
                  linkClick(event, { name: "data", table: name() })
                }
                aria-current={isRows() ? "page" : undefined}
              >
                Rows
              </a>
              <a
                class={segment(isStructure())}
                href={href({ name: "schema", table: name() })}
                onClick={(event) =>
                  linkClick(event, { name: "schema", table: name() })
                }
                aria-current={isStructure() ? "page" : undefined}
              >
                Structure
              </a>
            </div>
          )}
        </Show>

        <button
          class="btn btn-ghost btn-icon"
          onClick={() => studio.refresh()}
          aria-label="Refresh schema"
          title="Refresh schema"
        >
          <RefreshCw
            size={14}
            class={studio.schema.loading ? "animate-spin" : undefined}
            aria-hidden="true"
          />
        </button>

        <button
          class="btn btn-ghost btn-icon max-[640px]:hidden"
          onClick={studio.toggleTheme}
          aria-label={`Switch to ${
            studio.theme() === "dark" ? "light" : "dark"
          } theme`}
          title={`Switch to ${
            studio.theme() === "dark" ? "light" : "dark"
          } theme`}
        >
          <Show
            when={studio.theme() === "dark"}
            fallback={<Moon size={14} aria-hidden="true" />}
          >
            <Sun size={14} aria-hidden="true" />
          </Show>
        </button>

        <button
          class="btn btn-ghost h-7 gap-1.5 px-1.5"
          onClick={() => studio.setPaletteOpen(true)}
          aria-label="Open command palette"
          aria-keyshortcuts="Meta+K Control+K"
          title="Command palette"
        >
          <Search size={13} aria-hidden="true" />
          <span class="kbd" aria-hidden="true">
            ⌘K
          </span>
        </button>
      </div>
    </header>
  );
}

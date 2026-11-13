import { For, Show, createMemo, createSignal, onCleanup } from "solid-js";
import type { JSX } from "solid-js";
import {
  CircleAlert,
  CircleCheck,
  Clock,
  Play,
  Square,
  Terminal,
  Trash2,
} from "lucide-solid";
import { HistoryMenu } from "../components/console/history-menu";
import { SqlEditor } from "../components/console/editor";
import { ResultGrid } from "../components/console/result-grid";
import { duration } from "../lib/format";
import { useStudio } from "../lib/store";
import type { QueryResult } from "../lib/types";
import { EmptyState, Spinner } from "../lib/ui";

/**
 * The statements the hosted demo answers. Keep in sync with the canned queries
 * in `src/demo/source.ts` — the fixtures are imported by the demo source, so
 * this list is duplicated rather than pulling the demo into the live bundle.
 */
const DEMO_EXAMPLES = [
  { label: "users", sql: "select * from users" },
  { label: "top posts", sql: "select title, views from posts order by views desc" },
  {
    label: "roles",
    sql: "select role, count(*) as people from users group by role order by people desc",
  },
];

/** Survives navigation between routes, so recall still works after a detour. */
const [history, setHistory] = createSignal<string[]>([]);

/** Pull the failing line out of a driver message when it mentions one. */
function failingLine(
  message: string,
  sql: string,
): { number: number; text: string } | null {
  const byLine = /\bline[ :]+(\d+)\b/i.exec(message);
  const byOffset = /\b(?:at )?(?:character|char|position|offset)[ :]+(\d+)\b/i.exec(message);

  let number: number | undefined;
  if (byLine?.[1]) number = Number(byLine[1]);
  else if (byOffset?.[1]) {
    const offset = Number(byOffset[1]);
    number = sql.slice(0, offset).split(/\r?\n/).length;
  }

  if (number === undefined || !Number.isFinite(number) || number < 1) return null;
  const text = sql.split(/\r?\n/)[number - 1];
  return text === undefined ? null : { number, text };
}

function ErrorPanel(props: { message: string; sql: string }) {
  const line = createMemo(() => failingLine(props.message, props.sql));

  return (
    <div class="h-full overflow-auto p-3">
      <div class="rounded-lg border border-err/50 bg-err/10 px-3 py-2.5">
        <div class="flex items-start gap-2">
          <CircleAlert size={14} class="mt-0.5 shrink-0 text-err" />
          <div class="min-w-0 flex-1">
            <p class="text-[12px] leading-relaxed break-words text-err">
              {props.message}
            </p>
            <Show when={line()}>
              {(found) => (
                <div class="mt-2 flex items-baseline gap-2 font-mono text-[11px]">
                  <span class="shrink-0 text-ink-3">line {found().number}</span>
                  <code class="min-w-0 overflow-x-auto whitespace-pre text-ink">
                    {found().text}
                  </code>
                </div>
              )}
            </Show>
          </div>
        </div>
      </div>
    </div>
  );
}

function Affected(props: { result: QueryResult }) {
  const count = () => props.result.rows_affected ?? 0;

  return (
    <div class="flex h-full flex-col items-center justify-center gap-2 p-10 text-center">
      <div class="flex size-8 items-center justify-center rounded-full bg-accent-soft text-accent">
        <CircleCheck size={16} />
      </div>
      <p class="text-[13px] font-medium">
        {count()} {count() === 1 ? "row" : "rows"} affected
      </p>
      <Show when={props.result.last_insert_id !== null && props.result.last_insert_id !== undefined}>
        <p class="font-mono text-[11px] text-ink-3">
          last insert id {props.result.last_insert_id}
        </p>
      </Show>
    </div>
  );
}

export function SqlPage(): JSX.Element {
  const studio = useStudio();

  const [sql, setSql] = createSignal(studio.isDemo() ? (DEMO_EXAMPLES[0]?.sql ?? "") : "");
  const [result, setResult] = createSignal<QueryResult | null>(null);
  const [error, setError] = createSignal<string | null>(null);
  const [running, setRunning] = createSignal(false);
  const [stopped, setStopped] = createSignal(false);
  const [lastDuration, setLastDuration] = createSignal<number | null>(null);
  const [split, setSplit] = createSignal(0.42);
  const [dragging, setDragging] = createSignal(false);

  let ticket = 0;

  const remember = (statement: string) =>
    setHistory((current) =>
      [statement, ...current.filter((entry) => entry !== statement)].slice(0, 20),
    );

  const run = async () => {
    const statement = sql().trim();
    if (statement === "" || running()) return;

    const id = ++ticket;
    setRunning(true);
    setStopped(false);
    setError(null);
    setResult(null);
    remember(statement);

    const started = performance.now();
    try {
      const output = await studio.client().query(statement);
      if (id !== ticket) return;
      setResult(output);
      setLastDuration(output.duration_ms ?? Math.round(performance.now() - started));
    } catch (cause) {
      if (id !== ticket) return;
      setError(cause instanceof Error ? cause.message : String(cause));
      setLastDuration(Math.round(performance.now() - started));
    } finally {
      if (id === ticket) setRunning(false);
    }
  };

  const stop = () => {
    if (!running()) return;
    ticket += 1;
    setRunning(false);
    setStopped(true);
  };

  const clear = () => {
    setSql("");
    setResult(null);
    setError(null);
    setStopped(false);
    setLastDuration(null);
  };

  let surface: HTMLDivElement | undefined;
  let release: (() => void) | undefined;

  const track = (event: PointerEvent) => {
    const rect = surface?.getBoundingClientRect();
    if (!rect) return;
    const next = (event.clientY - rect.top) / rect.height;
    setSplit(Math.min(0.78, Math.max(0.16, next)));
  };

  const beginDrag = (event: PointerEvent) => {
    event.preventDefault();
    release?.();

    const finish = () => {
      window.removeEventListener("pointermove", track);
      window.removeEventListener("pointerup", finish);
      document.body.style.removeProperty("cursor");
      document.body.style.removeProperty("user-select");
      release = undefined;
      setDragging(false);
    };

    release = finish;
    setDragging(true);
    document.body.style.cursor = "row-resize";
    document.body.style.userSelect = "none";
    window.addEventListener("pointermove", track);
    window.addEventListener("pointerup", finish);
  };

  onCleanup(() => release?.());

  const nudge = (delta: number) =>
    setSplit((current) => Math.min(0.78, Math.max(0.16, current + delta)));

  return (
    <div class="flex h-full min-h-0 flex-col">
      <header class="flex h-11 shrink-0 items-center gap-1.5 border-b border-line bg-panel px-3">
        <button
          class="btn btn-primary"
          onClick={() => void run()}
          disabled={running() || sql().trim() === ""}
          aria-keyshortcuts="Meta+Enter Control+Enter"
        >
          <Show when={running()} fallback={<Play size={12} />}>
            <Spinner size={12} />
          </Show>
          Run
        </button>
        <button
          class="btn btn-ghost btn-icon"
          onClick={stop}
          disabled={!running()}
          aria-label="Stop query"
          title="Stop"
        >
          <Square size={11} />
        </button>
        <button
          class="btn btn-ghost"
          onClick={clear}
          aria-label="Clear editor and results"
          title="Clear"
        >
          <Trash2 size={13} />
          Clear
        </button>

        <Show when={lastDuration() !== null}>
          <span
            class="ml-1 flex items-center gap-1 font-mono text-[11px] text-ink-3"
            title="Elapsed time of the last run"
          >
            <Clock size={11} />
            {duration(lastDuration() ?? 0)}
          </span>
        </Show>

        <Show when={studio.isDemo()}>
          <span class="ml-1 hidden text-[11px] text-ink-3 md:inline">
            demo · canned queries only
          </span>
        </Show>

        <div class="ml-auto flex items-center gap-1.5">
          <span class="hidden items-center gap-1 lg:flex">
            <span class="kbd">⌘</span>
            <span class="kbd">↵</span>
            <span class="text-[11px] text-ink-3">to run</span>
          </span>
          <HistoryMenu
            history={history()}
            onPick={(statement) => setSql(statement)}
            onClear={() => setHistory([])}
          />
        </div>
      </header>

      <Show when={studio.isDemo()}>
        <div class="flex shrink-0 flex-wrap items-center gap-x-2 gap-y-1 border-b border-line bg-accent-soft px-3 py-1.5 text-[11.5px] text-ink-2">
          <span class="flex items-center gap-1.5 font-medium text-ink">
            <Terminal size={12} class="text-accent" />
            No SQL server in the demo.
          </span>
          <span>
            Run{" "}
            <code class="rounded bg-panel-2 px-1 py-0.5 font-mono text-[11px]">
              besu studio --database …
            </code>{" "}
            locally for real queries.
          </span>
          <span class="ml-auto flex items-center gap-1.5">
            <span class="text-[10px] tracking-wide text-ink-3 uppercase">Try</span>
            <For each={DEMO_EXAMPLES}>
              {(example) => (
                <button
                  class="btn h-6 px-2 font-mono text-[10.5px]"
                  title={example.sql}
                  onClick={() => {
                    setSql(example.sql);
                    void run();
                  }}
                >
                  {example.label}
                </button>
              )}
            </For>
          </span>
        </div>
      </Show>

      <div ref={surface} class="flex min-h-0 flex-1 flex-col">
        <div
          class="min-h-0 shrink-0 overflow-hidden"
          style={{ height: `${split() * 100}%` }}
        >
          <SqlEditor
            value={sql()}
            onInput={setSql}
            onRun={() => void run()}
            history={history()}
          />
        </div>

        <div
          class="group flex h-2 shrink-0 cursor-row-resize items-center justify-center border-y border-line"
          classList={{ "bg-accent-soft": dragging(), "bg-panel-2": !dragging() }}
          role="separator"
          aria-orientation="horizontal"
          aria-label="Resize editor and results"
          aria-valuenow={Math.round(split() * 100)}
          aria-valuemin={16}
          aria-valuemax={78}
          tabindex="0"
          onPointerDown={beginDrag}
          onKeyDown={(event) => {
            if (event.key === "ArrowUp") {
              event.preventDefault();
              nudge(-0.04);
            } else if (event.key === "ArrowDown") {
              event.preventDefault();
              nudge(0.04);
            }
          }}
        >
          <span class="h-px w-8 rounded bg-line-strong transition-colors group-hover:bg-accent" />
        </div>

        <section class="flex min-h-0 flex-1 flex-col bg-panel">
          <header class="flex h-9 shrink-0 items-center gap-2 border-b border-line px-3">
            <h2 class="text-[12px] font-medium">Results</h2>
            <span class="font-mono text-[11px] text-ink-3" aria-live="polite">
              <Show when={result()}>
                {(output) => (
                  <>
                    <Show
                      when={output().columns.length > 0}
                      fallback={`${output().rows_affected ?? 0} affected`}
                    >
                      {output().rows.length} {output().rows.length === 1 ? "row" : "rows"} ·{" "}
                      {output().columns.length}{" "}
                      {output().columns.length === 1 ? "column" : "columns"}
                    </Show>
                  </>
                )}
              </Show>
            </span>
          </header>

          <div class="min-h-0 flex-1">
            <Show
              when={error()}
              fallback={
                <Show
                  when={running()}
                  fallback={
                    <Show
                      when={stopped() && result() === null}
                      fallback={
                        <Show
                          when={result()}
                          fallback={
                            <EmptyState
                              title="No results yet"
                              detail="Write a statement above and press ⌘↵. The server runs exactly one statement at a time."
                            />
                          }
                        >
                          {(output) => (
                            <Show
                              when={output().columns.length > 0}
                              fallback={<Affected result={output()} />}
                            >
                              <ResultGrid
                                columns={output().columns}
                                rows={output().rows}
                              />
                            </Show>
                          )}
                        </Show>
                      }
                    >
                      <EmptyState
                        title="Query stopped"
                        detail="The result was discarded. A statement already in flight may still finish on the server."
                      />
                    </Show>
                  }
                >
                  <div class="flex h-full items-center justify-center p-8">
                    <Spinner label="Running…" />
                  </div>
                </Show>
              }
            >
              {(message) => <ErrorPanel message={message()} sql={sql()} />}
            </Show>
          </div>
        </section>
      </div>
    </div>
  );
}

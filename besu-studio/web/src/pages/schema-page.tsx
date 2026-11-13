import { For, Show, createMemo, createSignal, onCleanup } from "solid-js";
import type { JSX } from "solid-js";
import { Check, Copy, Eye, Key, Table2 } from "lucide-solid";
import { createTableSql } from "../components/console/create-table";
import { copy, displayType } from "../lib/format";
import { href, linkClick } from "../lib/router";
import { useStudio } from "../lib/store";
import { indexColumnLabel } from "../lib/types";
import type { Column, Table, View } from "../lib/types";
import { EmptyState, Spinner } from "../lib/ui";

/** An inline pill for a column or constraint attribute. */
function Badge(props: {
  children: JSX.Element;
  tone?: "accent";
  title?: string;
}) {
  return (
    <span
      title={props.title}
      class="inline-flex h-[18px] items-center rounded border px-1.5 font-mono text-[10px] tracking-wide uppercase"
      classList={{
        "border-transparent bg-accent-soft text-accent": props.tone === "accent",
        "border-line bg-panel-2 text-ink-2": props.tone !== "accent",
      }}
    >
      {props.children}
    </span>
  );
}

function None(props: { children: JSX.Element }) {
  return <p class="text-[12px] leading-relaxed text-ink-3">{props.children}</p>;
}

function Section(props: {
  title: string;
  meta?: string;
  action?: JSX.Element;
  children: JSX.Element;
}) {
  return (
    <section class="panel overflow-hidden">
      <header class="flex h-10 items-center gap-2 border-b border-line bg-panel-2 px-3">
        <h2 class="text-[12px] font-semibold">{props.title}</h2>
        <Show when={props.meta}>
          <span class="font-mono text-[10px] text-ink-3">{props.meta}</span>
        </Show>
        <Show when={props.action}>
          <div class="ml-auto">{props.action}</div>
        </Show>
      </header>
      {props.children}
    </section>
  );
}

function Group(props: { label: string; count: number; children: JSX.Element }) {
  return (
    <div class="grid gap-1.5 px-3 py-3 sm:grid-cols-[132px_1fr] sm:gap-3">
      <div class="flex items-baseline gap-1.5">
        <h3 class="text-[10px] font-medium tracking-wide text-ink-3 uppercase">
          {props.label}
        </h3>
        <Show when={props.count > 0}>
          <span class="font-mono text-[10px] text-ink-3 tabular-nums">{props.count}</span>
        </Show>
      </div>
      <div class="min-w-0 text-[12px]">{props.children}</div>
    </div>
  );
}

function CopyButton(props: { text: string; label: string }) {
  const [copied, setCopied] = createSignal(false);
  let timer: number | undefined;

  onCleanup(() => {
    if (timer !== undefined) window.clearTimeout(timer);
  });

  return (
    <button
      class="btn btn-ghost h-7 px-2"
      onClick={() => {
        void copy(props.text);
        setCopied(true);
        timer = window.setTimeout(() => setCopied(false), 1600);
      }}
      aria-label={props.label}
    >
      <Show
        when={copied()}
        fallback={
          <>
            <Copy size={12} />
            Copy
          </>
        }
      >
        <Check size={12} class="text-ok" />
        Copied
      </Show>
    </button>
  );
}

const cellHead =
  "h-8 border-b border-line px-3 text-left text-[10px] font-medium tracking-wide text-ink-3 uppercase";
const cellBody = "border-b border-line/60 px-3 py-1.5 align-middle";

function ColumnRow(props: { column: Column }) {
  const column = () => props.column;

  return (
    <tr class="last:border-0">
      <td class={cellBody}>
        <div class="flex items-center gap-1.5">
          <Show when={column().primary_key}>
            <Key size={10} class="shrink-0 text-accent" aria-label="Primary key" />
          </Show>
          <span class="font-mono text-[12px] text-ink">{column().name}</span>
        </div>
        <Show when={column().comment}>
          <p class="mt-0.5 text-[11px] leading-relaxed text-ink-3">{column().comment}</p>
        </Show>
      </td>
      <td class={cellBody}>
        <span class="font-mono text-[11.5px] whitespace-nowrap text-ink-2">
          {displayType(column().ty)}
        </span>
      </td>
      <td class={cellBody}>
        <Show
          when={column().nullable}
          fallback={<span class="font-mono text-[11px] text-ink-3">not null</span>}
        >
          <span class="font-mono text-[11px] text-ink-3 italic">null</span>
        </Show>
      </td>
      <td class={cellBody}>
        <Show
          when={column().default !== undefined}
          fallback={<span class="text-ink-3">—</span>}
        >
          <code class="font-mono text-[11px] whitespace-nowrap text-ink-2">
            {column().default}
          </code>
        </Show>
      </td>
      <td class={cellBody}>
        <div class="flex flex-wrap items-center gap-1">
          <Show when={column().primary_key}>
            <Badge tone="accent">pk</Badge>
          </Show>
          <Show when={column().unique}>
            <Badge>unique</Badge>
          </Show>
          <Show when={column().auto_increment}>
            <Badge>auto</Badge>
          </Show>
          <Show when={column().generated}>
            {(generated) => (
              <Badge title={generated().expression}>
                {generated().stored === false ? "virtual" : "generated"}
              </Badge>
            )}
          </Show>
        </div>
      </td>
      <td class={cellBody} aria-hidden="true" />
    </tr>
  );
}

function ColumnsSection(props: { table: Table }) {
  return (
    <Section
      title="Columns"
      meta={`${props.table.columns.length} ${props.table.columns.length === 1 ? "column" : "columns"}`}
    >
      <div class="overflow-x-auto">
        <table class="w-full border-collapse text-[12px]">
          <thead>
            <tr>
              <th scope="col" class={cellHead}>Name</th>
              <th scope="col" class={cellHead}>Type</th>
              <th scope="col" class={cellHead}>Nullable</th>
              <th scope="col" class={cellHead}>Default</th>
              <th scope="col" class={cellHead}>Attributes</th>
              <th scope="col" class="h-8 w-full border-b border-line" aria-hidden="true" />
            </tr>
          </thead>
          <tbody>
            <Show
              when={props.table.columns.length > 0}
              fallback={
                <tr>
                  <td class="px-3 py-3 text-[12px] text-ink-3" colSpan={6}>
                    This table has no columns.
                  </td>
                </tr>
              }
            >
              <For each={props.table.columns}>
                {(column) => <ColumnRow column={column} />}
              </For>
            </Show>
          </tbody>
        </table>
      </div>
    </Section>
  );
}

function ConstraintsSection(props: { table: Table }) {
  const table = () => props.table;
  const primaryKey = () => table().columns.filter((column) => column.primary_key);
  const uniques = () => table().uniques ?? [];
  const indexes = () => table().indexes ?? [];
  const foreignKeys = () => table().foreign_keys ?? [];
  const checks = () => table().checks ?? [];

  const total = () =>
    (primaryKey().length > 0 ? 1 : 0) +
    uniques().length +
    indexes().length +
    foreignKeys().length +
    checks().length;

  return (
    <Section
      title="Constraints"
      meta={`${total()} ${total() === 1 ? "constraint" : "constraints"}`}
    >
      <div class="divide-y divide-line">
        <Group label="Primary key" count={primaryKey().length}>
          <Show
            when={primaryKey().length > 0}
            fallback={<None>No primary key. Row edits and deletes refuse without one.</None>}
          >
            <div class="flex flex-wrap gap-1.5">
              <For each={primaryKey()}>
                {(column) => (
                  <code class="rounded border border-line bg-panel-2 px-1.5 py-0.5 font-mono text-[11px] text-ink-2">
                    {column.name}
                  </code>
                )}
              </For>
            </div>
          </Show>
        </Group>

        <Group label="Unique" count={uniques().length}>
          <Show when={uniques().length > 0} fallback={<None>No unique constraints.</None>}>
            <div class="grid gap-1.5">
              <For each={uniques()}>
                {(unique) => (
                  <div class="flex flex-wrap items-baseline gap-2">
                    <span class="font-mono text-[11px] text-ink-3">{unique.name}</span>
                    <code class="font-mono text-[11px] text-ink-2">
                      ({unique.columns.join(", ")})
                    </code>
                  </div>
                )}
              </For>
            </div>
          </Show>
        </Group>

        <Group label="Indexes" count={indexes().length}>
          <Show when={indexes().length > 0} fallback={<None>No indexes.</None>}>
            <div class="divide-y divide-line/60">
              <For each={indexes()}>
                {(index) => (
                  <div class="grid gap-1 py-1.5 first:pt-0 last:pb-0">
                    <div class="flex flex-wrap items-baseline gap-2">
                      <span class="font-mono text-[11.5px] text-ink">{index.name}</span>
                      <code class="font-mono text-[11px] text-ink-3">
                        ({index.columns.map(indexColumnLabel).join(", ")})
                      </code>
                      <Show when={index.unique}>
                        <Badge tone="accent">unique</Badge>
                      </Show>
                      <Show when={index.kind}>
                        <Badge>{index.kind}</Badge>
                      </Show>
                    </div>
                    <Show when={index.predicate}>
                      {(predicate) => (
                        <code
                          class="block truncate font-mono text-[10.5px] text-ink-3"
                          title={predicate()}
                        >
                          where {predicate()}
                        </code>
                      )}
                    </Show>
                  </div>
                )}
              </For>
            </div>
          </Show>
        </Group>

        <Group label="Foreign keys" count={foreignKeys().length}>
          <Show
            when={foreignKeys().length > 0}
            fallback={<None>No foreign keys.</None>}
          >
            <div class="divide-y divide-line/60">
              <For each={foreignKeys()}>
                {(foreign) => (
                  <div class="grid gap-1 py-1.5 first:pt-0 last:pb-0">
                    <div class="flex flex-wrap items-baseline gap-2">
                      <span class="font-mono text-[11.5px] text-ink">{foreign.name}</span>
                      <code class="font-mono text-[11px] text-ink-2">
                        ({foreign.columns.join(", ")})
                      </code>
                      <span class="text-ink-3" aria-hidden="true">→</span>
                      <code class="font-mono text-[11px] text-ink-2">
                        {foreign.references_table}({foreign.references_columns.join(", ")})
                      </code>
                    </div>
                    <div class="flex flex-wrap gap-1.5">
                      <Show when={foreign.on_delete && foreign.on_delete !== "Default"}>
                        <Badge>on delete {foreign.on_delete}</Badge>
                      </Show>
                      <Show when={foreign.on_update && foreign.on_update !== "Default"}>
                        <Badge>on update {foreign.on_update}</Badge>
                      </Show>
                    </div>
                  </div>
                )}
              </For>
            </div>
          </Show>
        </Group>

        <Group label="Checks" count={checks().length}>
          <Show when={checks().length > 0} fallback={<None>No check constraints.</None>}>
            <div class="divide-y divide-line/60">
              <For each={checks()}>
                {(check) => (
                  <div class="grid gap-1 py-1.5 first:pt-0 last:pb-0">
                    <span class="font-mono text-[11px] text-ink-3">{check.name}</span>
                    <code class="font-mono text-[11px] break-words text-ink-2">
                      {check.expression}
                    </code>
                  </div>
                )}
              </For>
            </div>
          </Show>
        </Group>
      </div>
    </Section>
  );
}

function PreviewSection(props: {
  table?: Table;
  view?: View;
  dialect?: string;
}) {
  const ddl = createMemo(() =>
    props.table ? createTableSql(props.table, props.dialect) : "",
  );

  return (
    <Section
      title={props.view ? "Definition" : "Preview"}
      meta={props.view ? "view sql" : "create table"}
      action={
        <CopyButton
          text={props.view ? props.view.definition : ddl()}
          label={props.view ? "Copy the view definition" : "Copy the CREATE TABLE statement"}
        />
      }
    >
      <pre class="overflow-x-auto px-3 py-3 font-mono text-[12px] leading-[1.7] text-ink-2">
        <Show
          when={props.view ? props.view.definition : ddl()}
          fallback={<None>Nothing to show.</None>}
        >
          {(text) => <code class="whitespace-pre">{text()}</code>}
        </Show>
      </pre>
    </Section>
  );
}

export function SchemaPage(props: { table: string }): JSX.Element {
  const studio = useStudio();

  const table = createMemo(() => {
    const schema = studio.schema();
    if (!schema) return undefined;
    return schema.tables.find(
      (candidate) =>
        candidate.name === props.table ||
        `${candidate.schema ?? ""}.${candidate.name}` === props.table,
    );
  });

  const view = createMemo(() => {
    const schema = studio.schema();
    if (!schema) return undefined;
    return schema.views?.find(
      (candidate) => candidate.name === props.table,
    );
  });

  const dialect = () => studio.info()?.dialect;

  return (
    <div class="flex h-full min-h-0 flex-col">
      <header class="flex h-11 shrink-0 items-center gap-2 border-b border-line bg-panel px-3">
        <Show when={view()} fallback={<Table2 size={13} class="text-ink-3" />}>
          <Eye size={13} class="text-ink-3" />
        </Show>
        <h1 class="truncate font-mono text-[13px] font-medium">{props.table}</h1>
        <Show when={table()}>
          {(current) => (
            <span class="font-mono text-[11px] whitespace-nowrap text-ink-3">
              {current().columns.length}{" "}
              {current().columns.length === 1 ? "column" : "columns"} ·{" "}
              {dialect() ?? "sql"}
            </span>
          )}
        </Show>
        <Show when={view()}>
          {(current) => (
            <Badge>{current().materialised ? "materialised view" : "view"}</Badge>
          )}
        </Show>
        <div class="ml-auto">
          <a
            class="btn"
            href={href({ name: "data", table: props.table })}
            onClick={(event) => linkClick(event, { name: "data", table: props.table })}
          >
            <Table2 size={12} />
            Browse rows
          </a>
        </div>
      </header>

      <div class="min-h-0 flex-1 overflow-auto p-3">
        <Show
          when={studio.schema()}
          fallback={
            <Show
              when={studio.schema.loading}
              fallback={
                <EmptyState
                  title="Schema unavailable"
                  detail="The studio could not read the schema from the server. Check the connection and try again."
                />
              }
            >
              <div class="flex h-full items-center justify-center">
                <Spinner label="Loading schema…" />
              </div>
            </Show>
          }
        >
          <Show
            when={table() ?? view()}
            fallback={
              <EmptyState
                title={`No table named ${props.table}`}
                detail="It may have been renamed or dropped. Pick another object from the sidebar."
              />
            }
          >
            <div class="mx-auto flex w-full max-w-[1100px] flex-col gap-3">
              <Show when={table()?.comment}>
                {(comment) => (
                  <div class="panel bg-panel-2 px-3 py-2 text-[12px] leading-relaxed text-ink-2">
                    {comment()}
                  </div>
                )}
              </Show>

              <Show when={table()}>
                {(current) => (
                  <>
                    <ColumnsSection table={current()} />
                    <ConstraintsSection table={current()} />
                  </>
                )}
              </Show>

              <PreviewSection table={table()} view={view()} dialect={dialect()} />
            </div>
          </Show>
        </Show>
      </div>
    </div>
  );
}

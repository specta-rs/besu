import { For, Show, createMemo, createSignal, type JSX } from "solid-js";
import { KeyRound, Trash2, TriangleAlert } from "lucide-solid";
import { cellText } from "../../lib/format";
import { useStudio } from "../../lib/store";
import type { Column, Json } from "../../lib/types";
import { Drawer, Modal, Spinner } from "../../lib/ui";
import {
  DraftField,
  draftFrom,
  parseDraft,
  sameDraft,
  sameJson,
  type Draft,
} from "./editors";

export interface RowDrawerProps {
  table: string;
  columns: Column[];
  row: Json[];
  primaryKey: string[];
  onClose: () => void;
}

function errorText(error: unknown): string {
  return error instanceof Error ? error.message : String(error);
}

/**
 * A single record, with per-type editors and the two writes: update and
 * delete. Rows are addressed by primary key, so key-less tables are read-only.
 */
export function RowDrawer(props: RowDrawerProps): JSX.Element {
  const studio = useStudio();

  const indexOf = createMemo(() => {
    const map = new Map<string, number>();
    props.columns.forEach((column, index) => map.set(column.name, index));
    return map;
  });

  const valueAt = (name: string): Json => {
    const index = indexOf().get(name);
    return index === undefined ? null : (props.row[index] ?? null);
  };

  const initialDrafts = () => {
    const next: Record<string, Draft> = {};
    for (const column of props.columns) {
      next[column.name] = draftFrom(column, valueAt(column.name));
    }
    return next;
  };

  const [drafts, setDrafts] = createSignal<Record<string, Draft>>(
    initialDrafts(),
  );
  const [errors, setErrors] = createSignal<Record<string, string>>({});
  const [message, setMessage] = createSignal<string | null>(null);
  const [busy, setBusy] = createSignal(false);
  const [confirming, setConfirming] = createSignal(false);

  const hasKey = () => props.primaryKey.length > 0;
  const readOnly = () => Boolean(studio.info()?.read_only);
  const locked = (column: Column) =>
    props.primaryKey.includes(column.name) || Boolean(column.primary_key);

  const key = createMemo<Record<string, Json>>(() => {
    const entries: [string, Json][] = [];
    for (const name of props.primaryKey) {
      entries.push([name, valueAt(name)]);
    }
    return Object.fromEntries(entries);
  });

  const keyComplete = () =>
    hasKey() &&
    props.primaryKey.every((name) => {
      const value = valueAt(name);
      return value !== null && value !== undefined;
    });

  const dirty = createMemo(() =>
    props.columns.some((column) => {
      if (locked(column)) return false;
      const current = drafts()[column.name];
      if (!current) return false;
      return !sameDraft(draftFrom(column, valueAt(column.name)), current);
    }),
  );

  const canSave = () =>
    hasKey() &&
    keyComplete() &&
    dirty() &&
    !busy() &&
    !readOnly() &&
    Object.keys(errors()).length === 0;

  const canDelete = () =>
    hasKey() && keyComplete() && !busy() && !readOnly();

  const update = (column: Column, draft: Draft) => {
    setDrafts((current) => ({ ...current, [column.name]: draft }));
    setMessage(null);
    setErrors((current) => {
      const next = { ...current };
      const parsed = parseDraft(column, draft);
      if (parsed.ok) delete next[column.name];
      else next[column.name] = parsed.error;
      return next;
    });
  };

  const close = () => {
    if (busy() || confirming()) return;
    props.onClose();
  };

  const save = async () => {
    if (busy()) return;

    const values: Record<string, Json> = {};
    const problems: Record<string, string> = {};

    for (const column of props.columns) {
      if (locked(column)) continue;
      const draft = drafts()[column.name];
      if (!draft) continue;
      const parsed = parseDraft(column, draft);
      if (!parsed.ok) {
        problems[column.name] = parsed.error;
        continue;
      }
      if (!sameJson(parsed.value, valueAt(column.name))) {
        values[column.name] = parsed.value;
      }
    }

    setErrors(problems);
    if (Object.keys(problems).length > 0) {
      setMessage("Fix the highlighted fields before saving.");
      return;
    }
    if (!keyComplete()) {
      setMessage("This row has no complete primary key, so it can't be saved.");
      return;
    }
    if (Object.keys(values).length === 0) {
      props.onClose();
      return;
    }

    setBusy(true);
    setMessage(null);
    try {
      await studio.client().update(props.table, key(), values);
      studio.toast("Row updated", "ok");
      studio.refresh();
      props.onClose();
    } catch (error) {
      setMessage(errorText(error));
    } finally {
      setBusy(false);
    }
  };

  const remove = async () => {
    if (busy() || !keyComplete()) return;
    setBusy(true);
    setMessage(null);
    try {
      await studio.client().remove(props.table, key());
      studio.toast("Row deleted", "ok");
      studio.refresh();
      setConfirming(false);
      props.onClose();
    } catch (error) {
      setConfirming(false);
      setMessage(errorText(error));
    } finally {
      setBusy(false);
    }
  };

  const title = () => {
    if (!hasKey()) return "Row";
    const parts = props.primaryKey.map((name) => cellText(valueAt(name)));
    return `Row ${parts.join(", ")}`;
  };

  return (
    <>
      <Drawer
        open
        title={title()}
        onClose={close}
        width="480px"
        footer={
          <>
            <button
              type="button"
              class="btn btn-danger mr-auto"
              onClick={() => setConfirming(true)}
              disabled={!canDelete()}
              title={
                hasKey()
                  ? "Delete this row"
                  : "This table has no primary key"
              }
            >
              {busy() && confirming() ? (
                <Spinner size={12} />
              ) : (
                <Trash2 size={12} />
              )}
              Delete
            </button>
            <button
              type="button"
              class="btn"
              onClick={close}
              disabled={busy()}
            >
              Cancel
            </button>
            <button
              type="button"
              class="btn btn-primary"
              onClick={save}
              disabled={!canSave()}
            >
              <Show when={busy()} fallback="Save changes">
                <Spinner size={12} label="Saving…" />
              </Show>
            </button>
          </>
        }
      >
        <div class="grid gap-4 px-4 py-4">
          <Show
            when={hasKey()}
            fallback={
              <div class="flex gap-2 rounded-md border border-warn/40 bg-warn/10 px-3 py-2 text-[11.5px] leading-relaxed text-warn">
                <TriangleAlert size={14} class="mt-0.5 shrink-0" />
                <p>
                  This table has no primary key, so rows can't be edited or
                  deleted. Browse the data here and use the SQL console for
                  changes that target rows explicitly.
                </p>
              </div>
            }
          >
            <div class="rounded-md border border-line bg-panel-2 px-3 py-2">
              <div class="flex items-center gap-1.5 text-[10px] font-medium tracking-wide text-ink-3 uppercase">
                <KeyRound size={11} />
                Primary key
              </div>
              <div class="mt-1.5 flex flex-wrap gap-x-3 gap-y-1 font-mono text-[12px]">
                <For each={props.primaryKey}>
                  {(name) => (
                    <span class="min-w-0 break-all">
                      <span class="text-ink-3">{name}=</span>
                      <span class="text-ink">{cellText(valueAt(name))}</span>
                    </span>
                  )}
                </For>
              </div>
            </div>
          </Show>

          <div class="grid gap-3">
            <For each={props.columns}>
              {(column) => (
                <DraftField
                  column={column}
                  draft={
                    drafts()[column.name] ??
                    draftFrom(column, valueAt(column.name))
                  }
                  onChange={(draft) => update(column, draft)}
                  error={errors()[column.name]}
                  locked={locked(column)}
                  disabled={!hasKey() || readOnly()}
                  hint={
                    locked(column)
                      ? "Primary keys can't be edited here."
                      : undefined
                  }
                />
              )}
            </For>
          </div>

          <Show when={!readOnly()}>
            <p class="text-[11px] leading-relaxed text-ink-3">
              Changes save only the fields you edited, keyed by the primary key
              above.
            </p>
          </Show>

          <Show when={message()}>
            <p class="rounded-md border border-err/30 bg-err/10 px-2.5 py-1.5 text-[11.5px] leading-relaxed text-err">
              {message()}
            </p>
          </Show>
        </div>
      </Drawer>

      <Modal
        open={confirming()}
        title="Delete row"
        onClose={() => {
          if (!busy()) setConfirming(false);
        }}
        footer={
          <>
            <button
              type="button"
              class="btn"
              onClick={() => setConfirming(false)}
              disabled={busy()}
            >
              Cancel
            </button>
            <button
              type="button"
              class="btn border-transparent bg-err text-accent-ink hover:bg-err"
              onClick={remove}
              disabled={busy()}
            >
              {busy() ? <Spinner size={12} label="Deleting…" /> : "Delete row"}
            </button>
          </>
        }
      >
        <p class="text-[12.5px] leading-relaxed text-ink-2">
          This permanently deletes the row with:
        </p>
        <div class="mt-2 grid gap-0.5 rounded-md border border-line bg-panel-2 px-3 py-2 font-mono text-[12px]">
          <For each={props.primaryKey}>
            {(name) => (
              <div class="break-all">
                <span class="text-ink-3">{name} = </span>
                <span class="text-ink">{cellText(valueAt(name))}</span>
              </div>
            )}
          </For>
        </div>
        <p class="mt-2 text-[12px] text-ink-3">This can't be undone.</p>
      </Modal>
    </>
  );
}

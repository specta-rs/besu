import { For, Show, createSignal, type JSX } from "solid-js";
import { useStudio } from "../../lib/store";
import type { Column, Json } from "../../lib/types";
import { Modal, Spinner } from "../../lib/ui";
import { DraftField, parseDraft, type Draft } from "./editors";

export interface AddRowModalProps {
  table: string;
  columns: Column[];
  onClose: () => void;
}

const FORM_ID = "besu-add-row-form";

function errorText(error: unknown): string {
  return error instanceof Error ? error.message : String(error);
}

/** One field per writable column, with the same editors the drawer uses. */
export function AddRowModal(props: AddRowModalProps): JSX.Element {
  const studio = useStudio();

  const writable = () =>
    props.columns.filter(
      (column) => !column.auto_increment && !column.generated,
    );

  const skipped = () =>
    props.columns.filter(
      (column) => column.auto_increment || column.generated,
    );

  const initialDrafts = () => {
    const next: Record<string, Draft> = {};
    for (const column of writable()) {
      next[column.name] = { text: "", isNull: false };
    }
    return next;
  };

  const [drafts, setDrafts] = createSignal<Record<string, Draft>>(
    initialDrafts(),
  );
  const [errors, setErrors] = createSignal<Record<string, string>>({});
  const [message, setMessage] = createSignal<string | null>(null);
  const [busy, setBusy] = createSignal(false);

  const update = (column: Column, draft: Draft) => {
    setDrafts((current) => ({ ...current, [column.name]: draft }));
    setMessage(null);
    setErrors((current) => {
      const next = { ...current };
      const parsed = parseDraft(column, draft, {
        required: !column.nullable && column.default === undefined,
        emptyAsNull: true,
      });
      if (parsed.ok) delete next[column.name];
      else next[column.name] = parsed.error;
      return next;
    });
  };

  const submit = async (event: SubmitEvent) => {
    event.preventDefault();
    if (busy()) return;

    const values: Record<string, Json> = {};
    const problems: Record<string, string> = {};

    for (const column of writable()) {
      const draft = drafts()[column.name] ?? { text: "", isNull: false };
      const empty = draft.text.trim() === "";

      // Columns with a default can be left blank; the database fills them in.
      if (!draft.isNull && empty && column.default !== undefined) continue;

      const parsed = parseDraft(column, draft, {
        required: !column.nullable && column.default === undefined,
        emptyAsNull: true,
      });
      if (!parsed.ok) {
        problems[column.name] = parsed.error;
        continue;
      }
      values[column.name] = parsed.value;
    }

    setErrors(problems);
    if (Object.keys(problems).length > 0) {
      setMessage("Some fields need a value before this row can be added.");
      return;
    }

    setBusy(true);
    setMessage(null);
    try {
      await studio.client().insert(props.table, values);
      studio.toast("Row added", "ok");
      studio.refresh();
      props.onClose();
    } catch (error) {
      setMessage(errorText(error));
    } finally {
      setBusy(false);
    }
  };

  const close = () => {
    if (!busy()) props.onClose();
  };

  return (
    <Modal
      open
      title={`Add row to ${props.table}`}
      onClose={close}
      width="560px"
      footer={
        <>
          <button
            type="button"
            class="btn"
            onClick={close}
            disabled={busy()}
          >
            Cancel
          </button>
          <button
            type="submit"
            form={FORM_ID}
            class="btn btn-primary"
            disabled={busy() || writable().length === 0}
          >
            {busy() ? <Spinner size={12} label="Adding…" /> : "Add row"}
          </button>
        </>
      }
    >
      <form id={FORM_ID} class="grid gap-3" onSubmit={submit}>
        <Show
          when={writable().length > 0}
          fallback={
            <p class="text-[12.5px] text-ink-3">
              Every column in this table is generated, so there is nothing to
              fill in.
            </p>
          }
        >
          <For each={writable()}>
            {(column) => (
              <DraftField
                column={column}
                draft={drafts()[column.name] ?? { text: "", isNull: false }}
                onChange={(draft) => update(column, draft)}
                error={errors()[column.name]}
                hint={
                  column.default !== undefined
                    ? `default: ${column.default}`
                    : undefined
                }
              />
            )}
          </For>
        </Show>

        <Show when={skipped().length > 0}>
          <p class="font-mono text-[10px] leading-relaxed text-ink-3">
            generated, left to the database:{" "}
            {skipped()
              .map((column) => column.name)
              .join(", ")}
          </p>
        </Show>

        <Show when={message()}>
          <p class="rounded-md border border-err/30 bg-err/10 px-2.5 py-1.5 text-[11.5px] leading-relaxed text-err">
            {message()}
          </p>
        </Show>
      </form>
    </Modal>
  );
}

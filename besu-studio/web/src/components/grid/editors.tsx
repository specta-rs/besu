import { Show } from "solid-js";
import type { JSX } from "@solidjs/web";
import { KeyRound } from "../../lib/icons";
import {
  displayType,
  isBoolean,
  isJson,
  isNumeric,
  isTemporal,
} from "../../lib/format";
import type { Draft } from "./draft";
import type { Column } from "../../lib/types";

export { draftFrom, parseDraft, sameDraft, sameJson } from "./draft";
export type { Draft, ParseResult } from "./draft";

export interface DraftFieldProps {
  column: Column;
  draft: Draft;
  onChange: (draft: Draft) => void;
  error?: string;
  disabled?: boolean;
  /** A primary-key column: shown greyed out and never sent as an update. */
  locked?: boolean;
  hint?: string;
}

/** One labelled, type-aware editor used by the row drawer and the add-row form. */
export function DraftField(props: DraftFieldProps): JSX.Element {
  const patch = (changes: Partial<Draft>) =>
    props.onChange({ ...props.draft, ...changes });

  const canNull = () => Boolean(props.column.nullable) && !props.locked;

  return (
    <div class="grid gap-1.5">
      <div class="flex items-center justify-between gap-2">
        <span class="flex min-w-0 items-baseline gap-1.5">
          <span class="truncate text-[11px] font-medium tracking-wide text-ink-2 uppercase">
            {props.column.name}
          </span>
          <span class="shrink-0 font-mono text-[10px] text-ink-3">
            {displayType(props.column.ty)}
          </span>
          <Show when={props.locked}>
            <span class="inline-flex shrink-0 items-center gap-0.5 rounded bg-panel-2 px-1 font-mono text-[9px] text-ink-3">
              <KeyRound size={8} />
              pk
            </span>
          </Show>
        </span>
        <Show when={canNull()}>
          <label
            class={`flex cursor-pointer items-center gap-1 text-[10px] font-medium tracking-wide uppercase ${
              props.draft.isNull ? "text-warn" : "text-ink-3"
            }`}
          >
            <input
              type="checkbox"
              class="h-3 w-3 accent-accent"
              checked={props.draft.isNull}
              disabled={props.disabled}
              aria-label={`${props.column.name} is NULL`}
              onChange={(event) => patch({ isNull: event.currentTarget.checked })}
            />
            NULL
          </label>
        </Show>
      </div>

      <Show
        when={!props.draft.isNull}
        fallback={
          <div class="field flex w-full items-center font-mono text-ink-3 italic">
            NULL
          </div>
        }
      >
        <Show
          when={!props.locked}
          fallback={
            <div class="field flex w-full items-center gap-1.5 bg-panel-2 font-mono text-ink-2">
              <KeyRound size={10} class="shrink-0 text-ink-3" />
              <span class="truncate">{props.draft.text || "NULL"}</span>
            </div>
          }
        >
          <Show
            when={!isBoolean(props.column)}
            fallback={
              <label class="flex h-7 items-center gap-2 text-[12px]">
                <input
                  type="checkbox"
                  class="h-3.5 w-3.5 accent-accent"
                  checked={props.draft.text === "true"}
                  disabled={props.disabled}
                  aria-label={props.column.name}
                  onChange={(event) =>
                    patch({
                      text: event.currentTarget.checked ? "true" : "false",
                      isNull: false,
                    })
                  }
                />
                <span class="font-mono text-ink-2">
                  {props.draft.text === "true" ? "true" : "false"}
                </span>
              </label>
            }
          >
            <Show
              when={!isJson(props.column)}
              fallback={
                <textarea
                  class="field h-auto min-h-[76px] w-full resize-y py-1.5 font-mono text-[12px] leading-relaxed"
                  value={props.draft.text}
                  disabled={props.disabled}
                  spellcheck={false}
                  aria-label={props.column.name}
                  onInput={(event) => patch({ text: event.currentTarget.value })}
                />
              }
            >
              <input
                class="field w-full font-mono"
                value={props.draft.text}
                disabled={props.disabled}
                spellcheck={false}
                aria-label={props.column.name}
                inputmode={
                  isNumeric(props.column)
                    ? "decimal"
                    : isTemporal(props.column)
                      ? "text"
                      : undefined
                }
                placeholder={
                  isTemporal(props.column) ? "2026-02-14T09:12:00" : undefined
                }
                onInput={(event) => patch({ text: event.currentTarget.value })}
              />
            </Show>
          </Show>
        </Show>
      </Show>

      <Show when={props.error}>
        <p class="text-[11px] text-err">{props.error}</p>
      </Show>
      <Show when={props.hint}>
        <p class="truncate font-mono text-[10px] text-ink-3" title={props.hint}>
          {props.hint}
        </p>
      </Show>
    </div>
  );
}

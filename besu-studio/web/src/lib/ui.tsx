import { For, type JSX, Show, onCleanup, onMount } from "solid-js";
import { Portal } from "solid-js/web";
import { X } from "lucide-solid";

function useEscape(onClose: () => void) {
  onMount(() => {
    const onKey = (event: KeyboardEvent) => {
      if (event.key === "Escape") {
        event.stopPropagation();
        onClose();
      }
    };
    window.addEventListener("keydown", onKey);
    onCleanup(() => window.removeEventListener("keydown", onKey));
  });
}

export interface ModalProps {
  open: boolean;
  title: string;
  onClose: () => void;
  children: JSX.Element;
  footer?: JSX.Element;
  width?: string;
}

/** A centered dialog. Rendered in a portal so it escapes the pane's clipping. */
export function Modal(props: ModalProps) {
  useEscape(() => props.onClose());

  return (
    <Show when={props.open}>
      <Portal>
        <div
          class="fixed inset-0 z-50 flex items-start justify-center bg-black/55 p-6 pt-[12vh] backdrop-blur-[2px]"
          onMouseDown={(event) => {
            if (event.target === event.currentTarget) props.onClose();
          }}
        >
          <div
            class="panel w-full overflow-hidden shadow-[var(--sp-shadow)]"
            style={{ "max-width": props.width ?? "520px" }}
            role="dialog"
            aria-modal="true"
            aria-label={props.title}
          >
            <header class="flex h-11 items-center justify-between border-b border-line px-4">
              <h2 class="text-[13px] font-semibold">{props.title}</h2>
              <button
                class="btn btn-ghost btn-icon"
                onClick={props.onClose}
                aria-label="Close"
              >
                <X size={14} />
              </button>
            </header>
            <div class="max-h-[65vh] overflow-auto px-4 py-4">{props.children}</div>
            <Show when={props.footer}>
              <footer class="flex items-center justify-end gap-2 border-t border-line bg-panel-2 px-4 py-3">
                {props.footer}
              </footer>
            </Show>
          </div>
        </div>
      </Portal>
    </Show>
  );
}

export interface DrawerProps {
  open: boolean;
  title: string;
  onClose: () => void;
  children: JSX.Element;
  footer?: JSX.Element;
  width?: string;
}

/** A right-hand sheet, used for a record's detail and its editor. */
export function Drawer(props: DrawerProps) {
  useEscape(() => props.onClose());

  return (
    <Show when={props.open}>
      <Portal>
        <div
          class="fixed inset-0 z-40 bg-black/40"
          onMouseDown={(event) => {
            if (event.target === event.currentTarget) props.onClose();
          }}
        >
          <aside
            class="absolute top-0 right-0 flex h-full w-full flex-col border-l border-line bg-panel shadow-[var(--sp-shadow)]"
            style={{ "max-width": props.width ?? "440px" }}
            role="dialog"
            aria-modal="true"
            aria-label={props.title}
            onMouseDown={(event) => event.stopPropagation()}
          >
            <header class="flex h-11 shrink-0 items-center justify-between border-b border-line px-4">
              <h2 class="truncate text-[13px] font-semibold">{props.title}</h2>
              <button
                class="btn btn-ghost btn-icon"
                onClick={props.onClose}
                aria-label="Close"
              >
                <X size={14} />
              </button>
            </header>
            <div class="min-h-0 flex-1 overflow-auto">{props.children}</div>
            <Show when={props.footer}>
              <footer class="flex shrink-0 items-center justify-end gap-2 border-t border-line bg-panel-2 px-4 py-3">
                {props.footer}
              </footer>
            </Show>
          </aside>
        </div>
      </Portal>
    </Show>
  );
}

export function Spinner(props: { size?: number; label?: string }) {
  return (
    <span class="inline-flex items-center gap-2 text-ink-3">
      <svg
        width={props.size ?? 14}
        height={props.size ?? 14}
        viewBox="0 0 24 24"
        fill="none"
        class="animate-spin"
        aria-hidden="true"
      >
        <circle cx="12" cy="12" r="9" stroke="currentColor" stroke-width="3" opacity="0.25" />
        <path d="M21 12a9 9 0 0 0-9-9" stroke="currentColor" stroke-width="3" stroke-linecap="round" />
      </svg>
      <Show when={props.label}>
        <span class="text-[12px]">{props.label}</span>
      </Show>
    </span>
  );
}

export function EmptyState(props: {
  title: string;
  detail?: string;
  children?: JSX.Element;
}) {
  return (
    <div class="flex h-full flex-col items-center justify-center gap-2 p-10 text-center">
      <p class="text-[13px] font-medium text-ink">{props.title}</p>
      <Show when={props.detail}>
        <p class="max-w-[380px] text-[12px] leading-relaxed text-ink-3">{props.detail}</p>
      </Show>
      {props.children}
    </div>
  );
}

/** A labelled form row used by the editors. */
export function Field(props: {
  label: string;
  hint?: string;
  children: JSX.Element;
}) {
  return (
    <label class="grid gap-1.5">
      <span class="flex items-baseline justify-between gap-2">
        <span class="text-[11px] font-medium tracking-wide text-ink-2 uppercase">
          {props.label}
        </span>
        <Show when={props.hint}>
          <span class="font-mono text-[10px] text-ink-3">{props.hint}</span>
        </Show>
      </span>
      {props.children}
    </label>
  );
}

/** A vertical list of key/value rows, for metadata panels. */
export function KeyValues(props: { items: [string, JSX.Element | string][] }) {
  return (
    <dl class="grid grid-cols-[minmax(90px,auto)_1fr] gap-x-3 gap-y-1.5 text-[12px]">
      <For each={props.items}>
        {([key, value]) => (
          <>
            <dt class="text-ink-3">{key}</dt>
            <dd class="min-w-0 break-words">{value}</dd>
          </>
        )}
      </For>
    </dl>
  );
}

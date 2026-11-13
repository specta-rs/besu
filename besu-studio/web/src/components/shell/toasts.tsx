import { For, type JSX, Show } from "solid-js";
import { Portal } from "solid-js/web";
import { CircleAlert, CircleCheck, Info, X } from "lucide-solid";
import type { ToastKind } from "../../lib/store";
import { useStudio } from "../../lib/store";

function ToastIcon(props: { kind: ToastKind }) {
  const tone = () =>
    props.kind === "ok"
      ? "text-ok"
      : props.kind === "error"
        ? "text-err"
        : "text-info";

  return (
    <span class={`mt-px flex shrink-0 ${tone()}`} aria-hidden="true">
      <Show
        when={props.kind !== "info"}
        fallback={<Info size={14} />}
      >
        <Show
          when={props.kind === "ok"}
          fallback={<CircleAlert size={14} />}
        >
          <CircleCheck size={14} />
        </Show>
      </Show>
    </span>
  );
}

/**
 * Bottom-right confirmations. The store owns the 4.2s life; this only renders,
 * animates in and dismisses. Fixed container, so toasts never shift layout.
 */
export function Toasts(): JSX.Element {
  const studio = useStudio();

  return (
    <Portal>
      <style>
        {`@keyframes besu-toast-in {
          from { opacity: 0; transform: translateY(6px); }
          to { opacity: 1; transform: translateY(0); }
        }`}
      </style>
      <div
        class="pointer-events-none fixed right-4 bottom-4 z-[60] flex w-[min(340px,calc(100vw-2rem))] flex-col gap-2"
        role="status"
        aria-live="polite"
        aria-atomic="false"
      >
        <For each={studio.toasts()}>
          {(toast) => (
            <div class="pointer-events-auto flex items-start gap-2.5 rounded-lg border border-line bg-panel px-3 py-2 shadow-[var(--sp-shadow)] animate-[besu-toast-in_120ms_ease-out]">
              <ToastIcon kind={toast.kind} />
              <p class="min-w-0 flex-1 text-[12px] leading-snug break-words text-ink">
                {toast.message}
              </p>
              <button
                class="-mr-1 grid size-5 shrink-0 place-items-center rounded text-ink-3 hover:bg-panel-2 hover:text-ink"
                onClick={() => studio.dismiss(toast.id)}
                aria-label="Dismiss notification"
              >
                <X size={12} aria-hidden="true" />
              </button>
            </div>
          )}
        </For>
      </div>
    </Portal>
  );
}

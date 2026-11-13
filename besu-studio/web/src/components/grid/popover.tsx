import {
  Show,
  createEffect,
  createSignal,
  onCleanup,
  onMount,
  type JSX,
} from "solid-js";
import { Portal } from "solid-js/web";

export interface PopoverProps {
  open: boolean;
  /** The control the panel hangs off. Ignored while `open` is false. */
  anchor: HTMLElement | undefined;
  onClose: () => void;
  children: JSX.Element;
  width?: number;
  align?: "start" | "end";
}

/**
 * A small fixed-position panel anchored to a trigger. Used by the filter chips
 * and the toolbar menus so they escape the grid's clipping and the page's
 * scrolling. Closes on outside press and Escape.
 */
export function Popover(props: PopoverProps) {
  let panel: HTMLDivElement | undefined;
  const [position, setPosition] = createSignal({ top: 0, left: 0 });

  const place = () => {
    const anchor = props.anchor;
    if (!anchor) return;

    const rect = anchor.getBoundingClientRect();
    const width = props.width ?? 280;
    const spaceBelow = window.innerHeight - rect.bottom;
    const height = panel?.offsetHeight ?? 0;
    const below = spaceBelow < height + 16 && rect.top > height + 16;
    const left = Math.min(
      Math.max(8, props.align === "end" ? rect.right - width : rect.left),
      Math.max(8, window.innerWidth - width - 8),
    );
    const top = below
      ? Math.max(8, rect.top - height - 6)
      : Math.min(rect.bottom + 6, Math.max(8, window.innerHeight - height - 8));

    setPosition({ top, left });
  };

  createEffect(() => {
    if (!props.open) return;
    // Reading the anchor keeps this in sync when a ref is (re)assigned.
    const anchor = props.anchor;
    if (!anchor) return;
    place();
    const frame = requestAnimationFrame(place);
    onCleanup(() => cancelAnimationFrame(frame));
  });

  onMount(() => {
    const onPointerDown = (event: MouseEvent) => {
      if (!props.open) return;
      const target = event.target as Node | null;
      if (!target) return;
      if (props.anchor?.contains(target) || panel?.contains(target)) return;
      props.onClose();
    };
    const onKeyDown = (event: KeyboardEvent) => {
      if (!props.open || event.key !== "Escape") return;
      event.stopPropagation();
      props.onClose();
    };
    const reposition = () => {
      if (props.open) place();
    };

    window.addEventListener("mousedown", onPointerDown, true);
    window.addEventListener("keydown", onKeyDown);
    window.addEventListener("resize", reposition);
    window.addEventListener("scroll", reposition, true);
    onCleanup(() => {
      window.removeEventListener("mousedown", onPointerDown, true);
      window.removeEventListener("keydown", onKeyDown);
      window.removeEventListener("resize", reposition);
      window.removeEventListener("scroll", reposition, true);
    });
  });

  return (
    <Show when={props.open}>
      <Portal>
        <div
          ref={panel}
          class="panel fixed z-[60] p-2 shadow-[var(--sp-shadow)]"
          style={{
            top: `${position().top}px`,
            left: `${position().left}px`,
            width: `${props.width ?? 280}px`,
          }}
          onMouseDown={(event) => event.stopPropagation()}
        >
          {props.children}
        </div>
      </Portal>
    </Show>
  );
}

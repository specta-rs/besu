import { createEffect, createSignal, onMount } from "solid-js";

/**
 * A plain textarea dressed as a code surface. Monospace, soft-wrapped, grows
 * with its content up to 40vh, `Tab` inserts two spaces, `⌘↵` runs, and
 * `↑`/`↓` walk the recent statements when the caret is on the first or last
 * line.
 */
export function SqlEditor(props: {
  value: string;
  onInput: (value: string) => void;
  onRun: () => void;
  /** Newest first, at most 20 entries. */
  history: string[];
}) {
  let area: HTMLTextAreaElement | undefined;

  const [index, setIndex] = createSignal(-1);
  let draft = "";
  let recalled = props.value;

  const autosize = () => {
    if (!area) return;
    area.style.height = "auto";
    const ceiling = window.innerHeight * 0.4;
    area.style.height = `${Math.min(area.scrollHeight, ceiling)}px`;
  };

  // Any value that did not come from history recall resets the walk.
  createEffect(() => {
    if (props.value !== recalled) {
      recalled = props.value;
      setIndex(-1);
      draft = "";
    }
    autosize();
  });

  onMount(() => {
    autosize();
    area?.focus();
  });

  const placeCaret = (at: number) => {
    requestAnimationFrame(() => {
      if (!area) return;
      area.focus();
      area.setSelectionRange(at, at);
    });
  };

  const insert = (text: string) => {
    if (!area) return;
    const start = area.selectionStart;
    const end = area.selectionEnd;
    props.onInput(`${props.value.slice(0, start)}${text}${props.value.slice(end)}`);
    placeCaret(start + text.length);
  };

  const recall = (next: number) => {
    const statement = next === -1 ? draft : (props.history[next] ?? draft);
    recalled = statement;
    setIndex(next);
    props.onInput(statement);
    placeCaret(statement.length);
  };

  const up = () => {
    const current = index();
    if (current === -1) {
      if (props.history.length === 0) return;
      draft = props.value;
      recall(0);
      return;
    }
    if (current + 1 < props.history.length) recall(current + 1);
  };

  const down = () => {
    const current = index();
    if (current === -1) return;
    recall(current === 0 ? -1 : current - 1);
  };

  const caret = () => area?.selectionStart ?? 0;
  const hasSelection = () =>
    area !== undefined && area.selectionStart !== area.selectionEnd;

  return (
    <div
      class="h-full min-h-0 overflow-auto bg-panel"
      onMouseDown={(event) => {
        if (event.target === event.currentTarget) {
          event.preventDefault();
          placeCaret(props.value.length);
        }
      }}
    >
      <textarea
        ref={area}
        value={props.value}
        onInput={(event) => props.onInput(event.currentTarget.value)}
        onKeyDown={(event) => {
          if (event.key === "Tab") {
            event.preventDefault();
            insert("  ");
            return;
          }
          if ((event.metaKey || event.ctrlKey) && event.key === "Enter") {
            event.preventDefault();
            props.onRun();
            return;
          }
          if (
            event.key === "ArrowUp" &&
            !hasSelection() &&
            !props.value.slice(0, caret()).includes("\n") &&
            props.history.length > 0
          ) {
            event.preventDefault();
            up();
            return;
          }
          if (
            event.key === "ArrowDown" &&
            !hasSelection() &&
            index() !== -1 &&
            !props.value.slice(caret()).includes("\n")
          ) {
            event.preventDefault();
            down();
          }
        }}
        aria-label="SQL editor"
        aria-keyshortcuts="Meta+Enter Control+Enter"
        placeholder="select * from users;"
        spellcheck={false}
        autocomplete="off"
        autocapitalize="off"
        class="block w-full resize-none bg-transparent px-4 py-3 font-mono text-[13px] leading-[1.7] text-ink outline-none placeholder:text-ink-3"
        style={{
          "min-height": "132px",
          "max-height": "40vh",
          "tab-size": "2",
        }}
      />
    </div>
  );
}

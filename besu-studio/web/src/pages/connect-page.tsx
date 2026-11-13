import { type JSX, Show, createMemo, createSignal, onMount } from "solid-js";
import {
  ArrowRight,
  Check,
  Copy,
  Database,
  FlaskConical,
  ShieldCheck,
  Terminal,
} from "lucide-solid";
import { targetFrom } from "../lib/connection";
import { copy } from "../lib/format";
import { useStudio } from "../lib/store";
import { Spinner } from "../lib/ui";

const COMMANDS = [
  { label: "SQLite", command: "besu studio --database sqlite://app.db" },
  { label: "PostgreSQL", command: "besu studio --database postgres://user:pass@localhost/app" },
  { label: "MySQL", command: "besu studio --database mysql://root:pass@localhost/app" },
];

/**
 * What the hosted page shows until it has a database to talk to.
 *
 * The order is deliberate: run the CLI, connect, and only then — as a quiet
 * second choice — look at fixture data. The demo exists so the page is not a
 * dead end, not as the product.
 */
export function ConnectPage(): JSX.Element {
  const studio = useStudio();
  const [port, setPort] = createSignal(String(studio.defaultPort));
  const [command, setCommand] = createSignal(0);
  const [copied, setCopied] = createSignal(false);
  const [attempted, setAttempted] = createSignal(false);

  const state = () => studio.connection();
  const probing = () => state().kind === "probing";
  const offline = () => {
    const current = state();
    return current.kind === "offline" ? current : null;
  };

  const target = createMemo(() => targetFrom(port()));

  onMount(() => {
    // Focus the port so a keyboard user can type and hit Enter immediately.
    document.getElementById("besu-connect-port")?.focus();
  });

  const submit = async (event: Event) => {
    event.preventDefault();
    if (probing()) return;
    setAttempted(true);
    await studio.connect(port());
  };

  const copyCommand = async () => {
    await copy(COMMANDS[command()]?.command ?? "");
    setCopied(true);
    window.setTimeout(() => setCopied(false), 1600);
  };

  return (
    <div class="h-screen overflow-y-auto bg-bg text-ink">
      <div class="mx-auto flex min-h-full w-full max-w-[860px] flex-col justify-center px-6 py-10 sm:px-8">
        <header class="flex items-start gap-3.5">
          <div class="flex size-10 shrink-0 items-center justify-center rounded-xl bg-accent-soft text-accent">
            <Database size={19} aria-hidden="true" />
          </div>
          <div class="min-w-0">
            <div class="flex items-baseline gap-2">
              <h1 class="text-[21px] font-semibold tracking-[-0.02em]">
                besu studio
              </h1>
              <span class="font-mono text-[10.5px] text-ink-3">0.1.0</span>
            </div>
            <p class="mt-1.5 max-w-[560px] text-[13px] leading-relaxed text-ink-2">
              A browser for your Besu database: edit rows, read the schema, run
              SQL. It runs on your machine, so start the CLI against a
              development database and connect to it from here.
            </p>
          </div>
        </header>

        <ol class="mt-8 grid gap-3">
          <li class="panel overflow-hidden">
            <div class="flex items-center gap-2 border-b border-line bg-panel-2 px-4 py-2">
              <span class="flex size-4 items-center justify-center rounded-full bg-accent-soft font-mono text-[10px] font-semibold text-accent">
                1
              </span>
              <h2 class="text-[12px] font-semibold">Start the studio</h2>
              <div class="ml-auto flex items-center gap-1" role="tablist" aria-label="Database">
                {COMMANDS.map((entry, index) => (
                  <button
                    class="rounded px-1.5 py-0.5 font-mono text-[10px] transition-colors"
                    classList={{
                      "bg-panel text-ink": command() === index,
                      "text-ink-3 hover:text-ink": command() !== index,
                    }}
                    role="tab"
                    aria-selected={command() === index}
                    onClick={() => setCommand(index)}
                  >
                    {entry.label}
                  </button>
                ))}
              </div>
            </div>
            <div class="flex items-center gap-3 px-4 py-3">
              <Terminal size={14} class="shrink-0 text-ink-3" aria-hidden="true" />
              <code class="min-w-0 flex-1 overflow-x-auto font-mono text-[12px] text-ink max-sm:whitespace-pre-wrap sm:whitespace-nowrap">
                {COMMANDS[command()]?.command}
              </code>
              <button
                class="btn btn-ghost btn-icon shrink-0"
                onClick={copyCommand}
                aria-label="Copy command"
                title="Copy command"
              >
                <Show when={copied()} fallback={<Copy size={12} />}>
                  <Check size={12} class="text-ok" />
                </Show>
              </button>
            </div>
            <p class="px-4 pb-3 text-[11.5px] leading-relaxed text-ink-3">
              It listens on{" "}
              <span class="font-mono text-ink-2">127.0.0.1:{studio.defaultPort}</span>{" "}
              by default. Add{" "}
              <span class="font-mono text-ink-2">--port 5000</span> if that one
              is taken, and read the line it prints on startup.
            </p>
          </li>

          <li class="panel overflow-hidden">
            <div class="flex items-center gap-2 border-b border-line bg-panel-2 px-4 py-2">
              <span class="flex size-4 items-center justify-center rounded-full bg-accent-soft font-mono text-[10px] font-semibold text-accent">
                2
              </span>
              <h2 class="text-[12px] font-semibold">Connect</h2>
              <Show when={probing()}>
                <span class="ml-auto flex items-center gap-1.5 text-[11px] text-ink-3">
                  <Spinner size={11} />
                  checking {target()}
                </span>
              </Show>
            </div>

            <form class="flex flex-wrap items-center gap-2 px-4 py-3" onSubmit={submit}>
              <div class="flex h-8 min-w-0 flex-1 items-center rounded-[7px] border border-line bg-panel focus-within:border-accent focus-within:ring-3 focus-within:ring-accent-soft">
                <span class="pl-2.5 font-mono text-[12px] text-ink-3 select-none">
                  http://127.0.0.1:
                </span>
                <input
                  id="besu-connect-port"
                  class="h-full min-w-0 flex-1 bg-transparent pr-2.5 pl-0 font-mono text-[12px] outline-none"
                  value={port()}
                  spellcheck={false}
                  autocomplete="off"
                  inputmode="numeric"
                  aria-label="Studio port or address"
                  placeholder={String(studio.defaultPort)}
                  onInput={(event) => setPort(event.currentTarget.value)}
                />
              </div>
              <button class="btn btn-primary" type="submit" disabled={probing()}>
                <Show when={probing()} fallback={<>Connect <ArrowRight size={13} /></>}>
                  <Spinner size={12} label="Connecting" />
                </Show>
              </button>
            </form>

            <Show when={offline()}>
              {(failure) => (
                <div class="mx-4 mb-3 rounded-[7px] border border-err/35 bg-err/8 px-3 py-2">
                  <p class="text-[12px] font-medium text-err">{failure().message}</p>
                  <p class="mt-1 text-[11.5px] leading-relaxed text-ink-2">
                    {failure().hint}
                  </p>
                  <Show when={attempted()}>
                    <p class="mt-1.5 text-[11.5px] leading-relaxed text-ink-3">
                      Running{" "}
                      <span class="font-mono text-ink-2">
                        besu studio --database …
                      </span>{" "}
                      and clicking Connect again is usually all it takes.
                    </p>
                  </Show>
                </div>
              )}
            </Show>

            <Show when={!offline()}>
              <p class="px-4 pb-3 text-[11.5px] leading-relaxed text-ink-3">
                Any port works. The page talks to your machine directly; no data
                leaves it.
              </p>
            </Show>
          </li>
        </ol>

        <div class="mt-5 flex flex-wrap items-center gap-x-4 gap-y-2 text-[11px] text-ink-3">
          <span class="flex items-center gap-1.5">
            <ShieldCheck size={12} aria-hidden="true" />
            Loopback only unless you pass{" "}
            <span class="font-mono">--allow-remote</span>.
          </span>
          <span class="flex items-center gap-1.5">
            <Database size={12} aria-hidden="true" />
            SQLite, PostgreSQL and MySQL.
          </span>
        </div>

        <div class="mt-10 border-t border-line pt-5">
          <p class="text-[11.5px] leading-relaxed text-ink-3">
            No database yet?{" "}
            <button
              class="inline-flex items-center gap-1 text-ink-2 underline decoration-line-strong decoration-1 underline-offset-2 transition-colors hover:text-ink"
              onClick={() => studio.useDemo()}
            >
              <FlaskConical size={11} aria-hidden="true" />
              Explore with demo data
            </button>{" "}
            — three sample tables, read-only, no SQL.
          </p>
        </div>
      </div>
    </div>
  );
}

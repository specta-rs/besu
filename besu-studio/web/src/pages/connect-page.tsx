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
import {
  DEFAULT_HOST,
  composeTarget,
  mixedContentBlocked,
} from "../lib/connection";
import { copy } from "../lib/format";
import { navigate } from "../lib/router";
import { useStudio } from "../lib/store";
import { Spinner } from "../lib/ui";

const COMMANDS = [
  { label: "SQLite", command: "besu studio --database sqlite://app.db" },
  { label: "PostgreSQL", command: "besu studio --database postgres://user:pass@localhost/app" },
  { label: "MySQL", command: "besu studio --database mysql://root:pass@localhost/app" },
];

/**
 * What `/` shows until it has a database to talk to.
 *
 * The order is deliberate: run the CLI, connect, and only then — as a quiet
 * second choice — look at fixture data. The demo exists so the page is not a
 * dead end, not as the product.
 */
export function ConnectPage(): JSX.Element {
  const studio = useStudio();
  const [host, setHost] = createSignal(DEFAULT_HOST);
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

  const target = createMemo(() => composeTarget(host(), port()));
  const pastedUrl = createMemo(() => /^https?:\/\//i.test(host().trim()));
  const blocked = createMemo(() => mixedContentBlocked(target()));

  onMount(() => {
    // Focus the address so a keyboard user can type and hit Enter immediately.
    const field = document.getElementById("besu-connect-host");
    if (field instanceof HTMLInputElement) {
      field.focus();
      field.select();
    }
  });

  const submit = async (event: Event) => {
    event.preventDefault();
    if (probing()) return;
    setAttempted(true);
    await studio.connect(target());
    if (studio.connected()) navigate("/overview");
  };

  const explore = () => {
    studio.useDemo();
    navigate("/overview");
  };

  const copyCommand = async () => {
    await copy(COMMANDS[command()]?.command ?? "");
    setCopied(true);
    window.setTimeout(() => setCopied(false), 1600);
  };

  return (
    <div class="h-screen overflow-y-auto bg-bg text-ink">
      <div class="mx-auto flex min-h-full w-full max-w-[860px] flex-col justify-center px-6 py-10 sm:px-8">
        <Show when={studio.connected()}>
          <button
            class="mb-5 flex w-full items-center gap-2 rounded-[9px] border border-line bg-panel px-3.5 py-2.5 text-left text-[12px] transition-colors hover:border-line-strong hover:bg-panel-2"
            onClick={() => navigate("/overview")}
          >
            <span class="size-1.5 shrink-0 rounded-full bg-ok" aria-hidden="true" />
            <span class="min-w-0 truncate">
              Connected to{" "}
              <span class="font-medium">
                {studio.info()?.database ?? "a database"}
              </span>
            </span>
            <span class="ml-auto flex shrink-0 items-center gap-1 text-ink-3">
              Open overview
              <ArrowRight size={12} aria-hidden="true" />
            </span>
          </button>
        </Show>

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
              <span class="font-mono text-ink-2">--port 5000</span> for another
              port, and <span class="font-mono text-ink-2">--allow-remote</span>{" "}
              to reach it from another device.
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

            <form class="px-4 py-3" onSubmit={submit}>
              <div class="flex flex-wrap items-end gap-2">
                <label class="grid min-w-0 flex-1 gap-1">
                  <span class="text-[10px] font-medium tracking-[0.08em] text-ink-3 uppercase">
                    Host
                  </span>
                  <input
                    id="besu-connect-host"
                    class="field w-full font-mono text-[12px]"
                    value={host()}
                    spellcheck={false}
                    autocomplete="off"
                    aria-label="Studio host or address"
                    placeholder={DEFAULT_HOST}
                    onInput={(event) => setHost(event.currentTarget.value)}
                  />
                </label>
                <label class="grid w-[92px] shrink-0 gap-1">
                  <span class="text-[10px] font-medium tracking-[0.08em] text-ink-3 uppercase">
                    Port
                  </span>
                  <input
                    class="field w-full font-mono text-[12px]"
                    value={port()}
                    spellcheck={false}
                    autocomplete="off"
                    inputmode="numeric"
                    aria-label="Studio port"
                    placeholder={String(studio.defaultPort)}
                    disabled={pastedUrl()}
                    onInput={(event) => setPort(event.currentTarget.value)}
                  />
                </label>
                <Show
                  when={!blocked()}
                  fallback={
                    <a
                      class="btn btn-primary"
                      href={target()}
                      target="_blank"
                      rel="noreferrer"
                    >
                      Open <ArrowRight size={13} />
                    </a>
                  }
                >
                  <button class="btn btn-primary" type="submit" disabled={probing()}>
                    <Show when={probing()} fallback={<>Connect <ArrowRight size={13} /></>}>
                      <Spinner size={12} label="Connecting" />
                    </Show>
                  </button>
                </Show>
              </div>

              <p class="mt-2 flex flex-wrap items-center gap-x-1.5 font-mono text-[11px] text-ink-3">
                <span class="text-ink-2">{target()}</span>
                <Show when={pastedUrl()}>
                  <span class="font-sans">— the port comes from the URL</span>
                </Show>
              </p>
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
                      and clicking Connect again is usually all it takes. If your
                      browser will not allow it,{" "}
                      <a
                        class="underline decoration-line-strong decoration-1 underline-offset-2 hover:text-ink-2"
                        href={target()}
                        target="_blank"
                        rel="noreferrer"
                      >
                        open {target().replace(/^https?:\/\//, "")} directly
                      </a>
                      .
                    </p>
                  </Show>
                </div>
              )}
            </Show>

            <Show when={blocked()}>
              <div class="mx-4 mb-3 rounded-[7px] border border-warn/35 bg-warn/8 px-3 py-2">
                <p class="text-[12px] font-medium text-warn">
                  This page is HTTPS, so it cannot call a plain-HTTP studio on
                  another machine.
                </p>
                <p class="mt-1 text-[11.5px] leading-relaxed text-ink-2">
                  Your browser stops the request before it leaves. The CLI serves
                  this same UI at{" "}
                  <span class="font-mono text-ink">{target().replace(/^https?:\/\//, "")}</span>
                  , so open it there. A studio behind HTTPS — a tunnel or a
                  reverse proxy — connects from here as normal.
                </p>
              </div>
            </Show>

            <Show when={!offline() && !blocked()}>
              <p class="px-4 pb-3 text-[11.5px] leading-relaxed text-ink-3">
                A hostname or IP works too, for a studio running on another
                machine or over a tailnet. The page talks to it directly; no data
                passes through this site.
              </p>
            </Show>
          </li>
        </ol>

        <div class="mt-5 flex flex-wrap items-center gap-x-4 gap-y-2 text-[11px] text-ink-3">
          <span class="flex items-center gap-1.5">
            <ShieldCheck size={12} aria-hidden="true" />
            <span>
              Loopback only unless you pass{" "}
              <span class="font-mono">--allow-remote</span>.
            </span>
          </span>
          <span class="flex items-center gap-1.5">
            <Database size={12} aria-hidden="true" />
            <span>SQLite, PostgreSQL and MySQL.</span>
          </span>
        </div>

        <div class="mt-10 border-t border-line pt-5">
          <p class="flex flex-wrap items-center gap-x-1.5 gap-y-1 text-[11.5px] leading-none text-ink-3">
            <span>No database yet?</span>
            <button
              class="inline-flex items-center gap-1.5 text-ink-2 underline decoration-line-strong decoration-1 underline-offset-[3px] transition-colors hover:text-ink"
              onClick={explore}
            >
              <FlaskConical size={11} class="shrink-0" aria-hidden="true" />
              <span>Explore with demo data</span>
            </button>
            <span>— three sample tables, read-only, no SQL.</span>
          </p>
        </div>
      </div>
    </div>
  );
}

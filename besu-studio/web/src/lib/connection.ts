import { createMemo, createSignal } from "solid-js";
import { ApiError, httpSource } from "./api";
import { demoSource, DEMO_INFO } from "../demo/source";
import type { ConnectionInfo, DataSource } from "./types";

/** The port `besu studio` listens on unless `--port` says otherwise. */
export const DEFAULT_PORT = 4983;

const STORAGE_KEY = "besu-studio-connection";

export type Connection =
  | { kind: "idle" }
  | { kind: "probing"; target: string }
  | { kind: "live"; target: string; info: ConnectionInfo }
  | { kind: "demo" }
  | { kind: "offline"; target: string; message: string; hint: string };

/** What the connect screen remembers between visits. */
type Stored = { mode: "live"; target: string } | { mode: "demo" };

function stored(): Stored | null {
  try {
    const raw = localStorage.getItem(STORAGE_KEY);
    if (!raw) return null;
    const value = JSON.parse(raw) as Stored;
    if (value.mode === "live" && typeof value.target === "string") return value;
    if (value.mode === "demo") return value;
  } catch {
    // A corrupt entry is the same as none.
  }
  return null;
}

function store(value: Stored | null): void {
  if (value) localStorage.setItem(STORAGE_KEY, JSON.stringify(value));
  else localStorage.removeItem(STORAGE_KEY);
}

/** The host the connect form starts on. */
export const DEFAULT_HOST = "localhost";

/**
 * Build an origin from the two fields the connect form offers.
 *
 * A host may be a name, an IPv4 address, a bracketed IPv6 address, `host:port`,
 * or a whole URL — the last one wins over the port field, because someone who
 * pasted `https://db.internal:8443` has already answered the question. An empty
 * host falls back to the loopback name.
 */
export function composeTarget(host: string, port: string): string {
  const name = host.trim().replace(/\/+$/, "");
  if (name === "") return `http://${DEFAULT_HOST}:${port || DEFAULT_PORT}`;
  if (/^https?:\/\//i.test(name)) return name;
  if (/^\[[^\]]+\](:\d+)?$/.test(name)) {
    return name.includes("]") && /\]:\d+$/.test(name)
      ? `http://${name}`
      : `http://${name}:${port || DEFAULT_PORT}`;
  }
  // `host:port` with no scheme: the colon already carries the port.
  if (/^[^:]+:\d+$/.test(name)) return `http://${name}`;
  return `http://${name}:${port || DEFAULT_PORT}`;
}

/**
 * Canonicalise an origin before it is stored or probed: a scheme is required
 * and a trailing slash is noise.
 */
export function normalizeTarget(input: string): string {
  const value = input.trim().replace(/\/+$/, "");
  if (value === "") return `http://${DEFAULT_HOST}:${DEFAULT_PORT}`;
  return /^https?:\/\//i.test(value) ? value : `http://${value}`;
}

/** A short, readable form of a target for the connection card. */
export function targetLabel(target: string): string {
  return target.replace(/^https?:\/\//, "");
}

interface ProbeFailure {
  message: string;
  hint: string;
}

/**
 * Ask a studio server who it is.
 *
 * A page served from HTTPS can only reach `127.0.0.1` when the browser allows
 * it and the server returns CORS headers for this origin; both failures look
 * like a thrown `fetch`, so the hint names the likely causes instead of
 * pretending they can be told apart.
 */
export async function probe(
  target: string,
  timeoutMs = 4000,
): Promise<ConnectionInfo> {
  const controller = new AbortController();
  const timer = window.setTimeout(() => controller.abort(), timeoutMs);
  try {
    const response = await fetch(`${target}/api/info`, {
      signal: controller.signal,
      headers: { accept: "application/json" },
    });
    const type = response.headers.get("content-type") ?? "";
    if (!type.includes("application/json")) {
      throw new ApiError(
        `${target} answered with ${type.split(";")[0] || "no content type"}, not a studio server.`,
        response.status,
      );
    }
    if (!response.ok) {
      throw new ApiError(
        `${target} answered ${response.status} ${response.statusText}.`,
        response.status,
      );
    }
    return (await response.json()) as ConnectionInfo;
  } finally {
    window.clearTimeout(timer);
  }
}

function describe(target: string, error: unknown): ProbeFailure {
  const label = targetLabel(target);
  if (error instanceof ApiError) {
    return {
      message: error.message,
      hint: `Nothing answered as a studio server on ${label}. Check the port the CLI printed when it started.`,
    };
  }
  if (error instanceof DOMException && error.name === "AbortError") {
    return {
      message: `${label} did not answer within four seconds.`,
      hint: "The server may be starting, or something is blocking the request.",
    };
  }
  return {
    message: `Could not reach ${label}.`,
    hint: "Start `besu studio` and check the port it printed. If it is already running, your browser is asking for permission to reach a device on the local network — allow it, then press Connect again. Chrome asks once per site; Safari and Brave block loopback from an HTTPS page.",
  };
}

/** Everything the app needs to decide what it is talking to. */
export function createConnection() {
  const [state, setState] = createSignal<Connection>({ kind: "idle" });

  const source = createMemo<DataSource | null>(() => {
    const current = state();
    if (current.kind === "live") return httpSource(current.target);
    if (current.kind === "demo") return demoSource;
    return null;
  });

  const live = (target: string, info: ConnectionInfo) => {
    store({ mode: "live", target });
    setState({ kind: "live", target, info });
  };

  const connect = async (input: string) => {
    const target = normalizeTarget(input);
    setState({ kind: "probing", target });
    try {
      live(target, await probe(target));
    } catch (error) {
      store(null);
      setState({ kind: "offline", target, ...describe(target, error) });
    }
  };

  const useDemo = () => {
    store({ mode: "demo" });
    setState({ kind: "demo" });
  };

  const disconnect = () => {
    store(null);
    setState({ kind: "idle" });
  };

  /** Restore the previous choice, or look for a server on this origin first. */
  const start = async () => {
    const saved = stored();
    if (saved?.mode === "demo") {
      setState({ kind: "demo" });
      return;
    }
    if (saved?.mode === "live") {
      await connect(saved.target);
      return;
    }

    // Served by `besu studio` itself: same-origin always works, and a hosted
    // page falls through to the loopback address the CLI defaults to.
    setState({ kind: "probing", target: "" });
    try {
      live(window.location.origin, await probe(""));
      return;
    } catch {
      // Not a studio server. Try the default port before asking the user.
    }

    const target = `http://127.0.0.1:${DEFAULT_PORT}`;
    setState({ kind: "probing", target });
    try {
      live(target, await probe(target));
    } catch (error) {
      setState({ kind: "idle" });
      void error;
    }
  };

  return {
    state,
    source,
    connect,
    useDemo,
    disconnect,
    start,
    demoInfo: DEMO_INFO,
  };
}

export type ConnectionHandle = ReturnType<typeof createConnection>;

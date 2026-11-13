import {
  type Accessor,
  createContext,
  createResource,
  createSignal,
  useContext,
} from "solid-js";
import type { Resource } from "solid-js";
import type { ConnectionHandle, Connection } from "./connection";
import { DEFAULT_PORT } from "./connection";
import type { ConnectionInfo, DataSource, Schema } from "./types";

export type ToastKind = "info" | "ok" | "error";

export interface Toast {
  id: number;
  kind: ToastKind;
  message: string;
}

export interface StudioState {
  schema: Resource<Schema | null>;
  info: Accessor<ConnectionInfo | null>;
  connecting: Accessor<boolean>;
  connection: Accessor<Connection>;
  /** The server origin, empty for a same-origin studio and `null` when local. */
  target: Accessor<string | null>;
  /** True when the app is showing bundled fixtures instead of a database. */
  isDemo: Accessor<boolean>;
  connected: Accessor<boolean>;
  /** The client for the active source. Only call it while `connected()`. */
  client: () => DataSource;
  connect: (input: string) => Promise<void>;
  useDemo: () => void;
  disconnect: () => void;
  defaultPort: number;
  selectedTable: Accessor<string | null>;
  selectTable: (table: string | null) => void;
  /** Bumped after a write so every table view reloads. */
  revision: Accessor<number>;
  refresh: () => void;
  toasts: Accessor<Toast[]>;
  toast: (message: string, kind?: ToastKind) => void;
  dismiss: (id: number) => void;
  theme: Accessor<"dark" | "light">;
  toggleTheme: () => void;
  paletteOpen: Accessor<boolean>;
  setPaletteOpen: (open: boolean) => void;
}

const StudioContext = createContext<StudioState>();

const initialTheme = (): "dark" | "light" => {
  const stored = localStorage.getItem("besu-studio-theme");
  if (stored === "dark" || stored === "light") return stored;
  return "dark";
};

export function createStudioState(connection: ConnectionHandle): StudioState {
  const [schema, { refetch: refetchSchema }] = createResource(
    connection.source,
    async (source) => {
      if (!source) return null;
      try {
        return await source.schema();
      } catch {
        return null;
      }
    },
  );

  const [selectedTable, setSelectedTable] = createSignal<string | null>(null);
  const [revision, setRevision] = createSignal(0);
  const [toasts, setToasts] = createSignal<Toast[]>([]);
  const [theme, setTheme] = createSignal<"dark" | "light">(initialTheme());
  const [paletteOpen, setPaletteOpen] = createSignal(false);

  let nextToast = 1;

  document.documentElement.classList.toggle("dark", theme() === "dark");

  const current = connection.state;

  const info = (): ConnectionInfo | null => {
    const state = current();
    if (state.kind === "live") return state.info;
    if (state.kind === "demo") return connection.demoInfo;
    return null;
  };

  const state: StudioState = {
    schema,
    info,
    connecting: () =>
      current().kind === "probing" || current().kind === "idle",
    connection: current,
    target: () => {
      const value = current();
      return value.kind === "live" || value.kind === "offline"
        ? value.target
        : null;
    },
    isDemo: () => current().kind === "demo",
    connected: () => current().kind === "live" || current().kind === "demo",
    client: () => {
      const source = connection.source();
      if (!source) throw new Error("no database is connected");
      return source;
    },
    connect: async (input) => {
      await connection.connect(input);
      const result = current();
      if (result.kind === "offline") return;
      if (result.kind === "live") {
        setSelectedTable(null);
      }
    },
    useDemo: () => {
      connection.useDemo();
      setSelectedTable(null);
    },
    disconnect: () => {
      connection.disconnect();
      setSelectedTable(null);
    },
    defaultPort: DEFAULT_PORT,
    selectedTable,
    selectTable: (table) => setSelectedTable(table),
    revision,
    refresh: () => {
      setRevision((n) => n + 1);
      void refetchSchema();
    },
    toasts,
    toast: (message, kind = "info") => {
      const id = nextToast++;
      setToasts((all) => [...all, { id, kind, message }]);
      window.setTimeout(() => {
        setToasts((all) => all.filter((toast) => toast.id !== id));
      }, 4200);
    },
    dismiss: (id) => setToasts((all) => all.filter((toast) => toast.id !== id)),
    theme,
    toggleTheme: () => {
      const next = theme() === "dark" ? "light" : "dark";
      setTheme(next);
      localStorage.setItem("besu-studio-theme", next);
      document.documentElement.classList.toggle("dark", next === "dark");
    },
    paletteOpen,
    setPaletteOpen,
  };

  return state;
}

export function StudioProvider(props: {
  value: StudioState;
  children: unknown;
}) {
  return (
    <StudioContext.Provider value={props.value}>
      {props.children as never}
    </StudioContext.Provider>
  );
}

/** The studio's shared state. Only valid inside `<App>`. */
export function useStudio(): StudioState {
  const state = useContext(StudioContext);
  if (!state) throw new Error("useStudio must be used inside the studio app");
  return state;
}


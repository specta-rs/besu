import {
  type Accessor,
  createContext,
  createResource,
  createSignal,
  useContext,
} from "solid-js";
import type { Resource } from "solid-js";
import { source, isDemo } from "./source";
import type { ConnectionInfo, Schema } from "./types";

export type ToastKind = "info" | "ok" | "error";

export interface Toast {
  id: number;
  kind: ToastKind;
  message: string;
}

export interface StudioState {
  schema: Resource<Schema | null>;
  info: Resource<ConnectionInfo | null>;
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
  isDemo: boolean;
}

const StudioContext = createContext<StudioState>();

const initialTheme = (): "dark" | "light" => {
  const stored = localStorage.getItem("besu-studio-theme");
  if (stored === "dark" || stored === "light") return stored;
  return "dark";
};

export function createStudioState(): StudioState {
  const [schema, { refetch: refetchSchema }] = createResource(async () => {
    try {
      return await source.schema();
    } catch {
      return null;
    }
  });
  const [info] = createResource(async () => {
    try {
      return await source.info();
    } catch {
      return null;
    }
  });

  const [selectedTable, setSelectedTable] = createSignal<string | null>(null);
  const [revision, setRevision] = createSignal(0);
  const [toasts, setToasts] = createSignal<Toast[]>([]);
  const [theme, setTheme] = createSignal<"dark" | "light">(initialTheme());
  const [paletteOpen, setPaletteOpen] = createSignal(false);

  let nextToast = 1;

  document.documentElement.classList.toggle("dark", theme() === "dark");

  const state: StudioState = {
    schema,
    info,
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
      setToasts((current) => [...current, { id, kind, message }]);
      window.setTimeout(() => {
        setToasts((current) => current.filter((toast) => toast.id !== id));
      }, 4200);
    },
    dismiss: (id) =>
      setToasts((current) => current.filter((toast) => toast.id !== id)),
    theme,
    toggleTheme: () => {
      const next = theme() === "dark" ? "light" : "dark";
      setTheme(next);
      localStorage.setItem("besu-studio-theme", next);
      document.documentElement.classList.toggle("dark", next === "dark");
    },
    paletteOpen,
    setPaletteOpen,
    isDemo,
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

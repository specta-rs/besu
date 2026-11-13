import type { Column, Json, TypeDef } from "./types";

/** The bare name of a type: `"VarChar"`, not `{ "VarChar": 255 }`. */
export function typeName(ty: TypeDef): string {
  return typeof ty === "string" ? ty : (Object.keys(ty)[0] ?? "Custom");
}

/** The argument of a parameterised type, when it has one. */
export function typeArg(ty: TypeDef): unknown {
  return typeof ty === "string" ? undefined : Object.values(ty)[0];
}

/** A short, human spelling of a column type, as the sidebar and headers show it. */
export function displayType(ty: TypeDef): string {
  const name = typeName(ty);
  const arg = typeArg(ty);

  switch (name) {
    case "UInt":
      return `uint${String(arg)}`;
    case "Char":
      return `char(${String(arg)})`;
    case "VarChar":
      return `varchar(${String(arg)})`;
    case "Time":
      return "time";
    case "TimeTz":
      return "timetz";
    case "Timestamp":
      return "timestamp";
    case "TimestampTz":
      return "timestamptz";
    case "Custom": {
      const custom = typeof arg === "string" ? arg : "custom";
      return custom.length > 18 ? `${custom.slice(0, 16)}…` : custom;
    }
    default: {
      // `Int2` -> `int2`, `Jsonb` -> `jsonb`, `Float8` -> `float8`.
      return name.replace(/([a-z])([A-Z])/g, "$1$2").toLowerCase();
    }
  }
}

export function isNumeric(column: Column): boolean {
  return /^(Int|UInt|Float)/.test(typeName(column.ty));
}

export function isBoolean(column: Column): boolean {
  return typeName(column.ty) === "Bool";
}

export function isJson(column: Column): boolean {
  const name = typeName(column.ty);
  return name === "Json" || name === "Jsonb";
}

export function isTemporal(column: Column): boolean {
  return /^(Date|Time|Timestamp)/.test(typeName(column.ty));
}

/** The one-line text a cell shows when it is not being edited. */
export function cellText(value: Json): string {
  if (value === null || value === undefined) return "NULL";
  if (typeof value === "boolean") return value ? "true" : "false";
  if (typeof value === "object") return JSON.stringify(value);
  return String(value);
}

/** `n` abbreviated for a badge: `1.2k`, `14k`, `3.4m`. */
export function compactNumber(value: number): string {
  if (value < 1000) return String(value);
  if (value < 1_000_000) return `${(value / 1000).toFixed(value < 10_000 ? 1 : 0)}k`;
  return `${(value / 1_000_000).toFixed(1)}m`;
}

/** `12 ms`, `1.4 s`. */
export function duration(ms: number | undefined): string {
  if (ms === undefined) return "";
  if (ms < 1000) return `${Math.round(ms)} ms`;
  return `${(ms / 1000).toFixed(2)} s`;
}

/** Copy text to the clipboard, falling back to a transient textarea. */
export async function copy(text: string): Promise<void> {
  try {
    await navigator.clipboard.writeText(text);
  } catch {
    const area = document.createElement("textarea");
    area.value = text;
    area.style.position = "fixed";
    area.style.opacity = "0";
    document.body.append(area);
    area.select();
    document.execCommand("copy");
    area.remove();
  }
}

/** Build a CSV file from a header and rows, quoting everything fragile. */
export function toCsv(columns: string[], rows: Json[][]): string {
  const escape = (value: Json) => {
    if (value === null || value === undefined) return "";
    const text = typeof value === "object" ? JSON.stringify(value) : String(value);
    return /[",\n]/.test(text) ? `"${text.replaceAll('"', '""')}"` : text;
  };
  return [
    columns.map(escape).join(","),
    ...rows.map((row) => row.map(escape).join(",")),
  ].join("\n");
}

/** Trigger a browser download for generated text. */
export function download(name: string, text: string, type: string): void {
  const blob = new Blob([text], { type });
  const url = URL.createObjectURL(blob);
  const link = document.createElement("a");
  link.href = url;
  link.download = name;
  link.click();
  URL.revokeObjectURL(url);
}

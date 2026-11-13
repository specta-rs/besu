import { createSignal } from "solid-js";

export type Route =
  | { name: "setup" }
  | { name: "overview" }
  | { name: "data"; table: string }
  | { name: "sql" }
  | { name: "schema"; table: string };

/** Read the current route out of a pathname. */
export function parse(path: string): Route {
  const parts = path.replace(/^\/+|\/+$/g, "").split("/").filter(Boolean);

  if (parts[0] === "overview") return { name: "overview" };
  if (parts[0] === "sql") return { name: "sql" };
  if (parts[0] === "schema" && parts[1]) {
    return { name: "schema", table: decodeURIComponent(parts[1]) };
  }
  if (parts[0] === "t" && parts[1]) {
    return { name: "data", table: decodeURIComponent(parts[1]) };
  }
  return { name: "setup" };
}

export function href(route: Route): string {
  switch (route.name) {
    case "data":
      return `/t/${encodeURIComponent(route.table)}`;
    case "schema":
      return `/schema/${encodeURIComponent(route.table)}`;
    case "sql":
      return "/sql";
    case "overview":
      return "/overview";
    case "setup":
      return "/";
  }
}

const [path, setPath] = createSignal(
  typeof window === "undefined" ? "/" : window.location.pathname,
);

if (typeof window !== "undefined") {
  window.addEventListener("popstate", () => setPath(window.location.pathname));
}

export const currentPath = path;
export const route = () => parse(path());

/** Push a route without a full page load. */
export function navigate(to: string | Route, replace = false): void {
  const target = typeof to === "string" ? to : href(to);
  if (target === window.location.pathname) return;
  if (replace) window.history.replaceState(null, "", target);
  else window.history.pushState(null, "", target);
  setPath(window.location.pathname);
}

/** An `<a>` handler that keeps modifier-clicks working like a browser tab. */
export function linkClick(event: MouseEvent, to: string | Route): void {
  if (event.metaKey || event.ctrlKey || event.shiftKey || event.button !== 0) return;
  event.preventDefault();
  navigate(to);
}

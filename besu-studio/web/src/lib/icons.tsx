import { Dynamic, type ValidComponent, type JSX } from "@solidjs/web";
import {
  AlertTriangle as AlertTriangleNode,
  ArrowDown as ArrowDownNode,
  ArrowRight as ArrowRightNode,
  ArrowUp as ArrowUpNode,
  Check as CheckNode,
  ChevronDown as ChevronDownNode,
  ChevronLeft as ChevronLeftNode,
  ChevronRight as ChevronRightNode,
  CircleAlert as CircleAlertNode,
  CircleCheck as CircleCheckNode,
  Clock as ClockNode,
  Columns3 as Columns3Node,
  Copy as CopyNode,
  Database as DatabaseNode,
  Download as DownloadNode,
  Eye as EyeNode,
  Filter as FilterNode,
  FlaskConical as FlaskConicalNode,
  History as HistoryNode,
  Info as InfoNode,
  Key as KeyNode,
  KeyRound as KeyRoundNode,
  Menu as MenuNode,
  Moon as MoonNode,
  Play as PlayNode,
  Plus as PlusNode,
  RefreshCw as RefreshCwNode,
  Search as SearchNode,
  ShieldCheck as ShieldCheckNode,
  Square as SquareNode,
  Sun as SunNode,
  Table2 as Table2Node,
  Terminal as TerminalNode,
  Trash2 as Trash2Node,
  TriangleAlert as TriangleAlertNode,
  Unplug as UnplugNode,
  X as XNode,
  type IconNode,
} from "lucide";

/**
 * The app's icons.
 *
 * `lucide-solid` is a Solid 1 renderer: it imports `mergeProps`, `splitProps`
 * and `solid-js/web`, none of which exist in Solid 2. The framework-agnostic
 * `lucide` package ships the same artwork as tag/attribute data, so this thin
 * wrapper renders it the way the adapter did — same defaults, same classes.
 */
export interface IconProps {
  size?: number;
  class?: string;
  strokeWidth?: number;
  "aria-hidden"?: "true" | "false";
  "aria-label"?: string;
}

const defaults = {
  xmlns: "http://www.w3.org/2000/svg",
  viewBox: "0 0 24 24",
  fill: "none",
  stroke: "currentColor",
  "stroke-linecap": "round",
  "stroke-linejoin": "round",
} as const;

function icon(name: string, node: IconNode) {
  const shapes = node.map(([tag, attrs]) => ({ tag, attrs }));

  return function Icon(props: IconProps): JSX.Element {
    return (
      <svg
        {...defaults}
        width={props.size ?? 24}
        height={props.size ?? 24}
        stroke-width={props.strokeWidth ?? 2}
        class={
          props.class === undefined
            ? `lucide lucide-${name}`
            : `lucide lucide-${name} ${props.class}`
        }
        aria-hidden={
          props["aria-label"] === undefined
            ? (props["aria-hidden"] ?? "true")
            : undefined
        }
        aria-label={props["aria-label"]}
      >
        {shapes.map((shape) => (
          <Dynamic component={shape.tag as ValidComponent} {...shape.attrs} />
        ))}
      </svg>
    );
  };
}

export const AlertTriangle = icon("alert-triangle", AlertTriangleNode);
export const ArrowDown = icon("arrow-down", ArrowDownNode);
export const ArrowRight = icon("arrow-right", ArrowRightNode);
export const ArrowUp = icon("arrow-up", ArrowUpNode);
export const Check = icon("check", CheckNode);
export const ChevronDown = icon("chevron-down", ChevronDownNode);
export const ChevronLeft = icon("chevron-left", ChevronLeftNode);
export const ChevronRight = icon("chevron-right", ChevronRightNode);
export const CircleAlert = icon("circle-alert", CircleAlertNode);
export const CircleCheck = icon("circle-check", CircleCheckNode);
export const Clock = icon("clock", ClockNode);
export const Columns3 = icon("columns-3", Columns3Node);
export const Copy = icon("copy", CopyNode);
export const Database = icon("database", DatabaseNode);
export const Download = icon("download", DownloadNode);
export const Eye = icon("eye", EyeNode);
export const Filter = icon("filter", FilterNode);
export const FlaskConical = icon("flask-conical", FlaskConicalNode);
export const History = icon("history", HistoryNode);
export const Info = icon("info", InfoNode);
export const Key = icon("key", KeyNode);
export const KeyRound = icon("key-round", KeyRoundNode);
export const Menu = icon("menu", MenuNode);
export const Moon = icon("moon", MoonNode);
export const Play = icon("play", PlayNode);
export const Plus = icon("plus", PlusNode);
export const RefreshCw = icon("refresh-cw", RefreshCwNode);
export const Search = icon("search", SearchNode);
export const ShieldCheck = icon("shield-check", ShieldCheckNode);
export const Square = icon("square", SquareNode);
export const Sun = icon("sun", SunNode);
export const Table2 = icon("table-2", Table2Node);
export const Terminal = icon("terminal", TerminalNode);
export const Trash2 = icon("trash-2", Trash2Node);
export const TriangleAlert = icon("triangle-alert", TriangleAlertNode);
export const Unplug = icon("unplug", UnplugNode);
export const X = icon("x", XNode);

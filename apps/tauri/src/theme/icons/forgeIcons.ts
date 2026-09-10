/**
 * The UI icon set, backed by lucide.
 *
 * A name maps to a concrete lucide component imported by name, so the bundler
 * keeps only the glyphs the app actually names — the registry is a lookup, not
 * a barrel re-export. Call sites still say `name="check"`; what changed is what
 * sits behind the name.
 *
 * Two kinds of mark are deliberately *not* here. A provider's or an editor's
 * logo is a brand, drawn from its own SVG by `BrandIcon`. A session's state —
 * running, starting, exited, failed, needs-you — is the app's own vocabulary,
 * not an icon, and is a geometric `StateMarker`. Everything else is lucide.
 */
import {
  ArrowLeft,
  Bot,
  ChartColumn,
  Check,
  ChevronDown,
  ChevronLeft,
  ChevronRight,
  Columns2,
  Copy,
  ExternalLink,
  File,
  FileCode,
  Filter,
  Folder,
  FolderOpen,
  GitBranch,
  GitPullRequest,
  History,
  LayoutGrid,
  ListChecks,
  Loader2,
  MessageSquarePlus,
  MoreHorizontal,
  Palette,
  PanelLeftClose,
  PanelLeftOpen,
  PanelRight,
  Pencil,
  Plus,
  RefreshCw,
  Search,
  Settings,
  SquareTerminal,
  Trash2,
  X,
  type LucideProps,
} from "lucide-solid";
import type { Component } from "solid-js";

export type ForgeIconName =
  | "git-branch"
  | "git-pull-request"
  | "arrow-left"
  | "settings"
  | "appearance"
  | "stats"
  | "check"
  | "loader"
  | "filter"
  | "refresh"
  | "copy"
  | "external-link"
  | "edit"
  | "trash"
  | "agent"
  | "square-terminal"
  | "panel-left-open"
  | "panel-left-close"
  | "panel-right"
  | "chevron-left"
  | "chevron-right"
  | "chevron-down"
  | "close"
  | "plus"
  | "folder-open"
  | "folder"
  | "file"
  | "file-code"
  | "search"
  | "message-square-plus"
  | "columns-2"
  | "list-checks"
  | "history"
  | "layout-grid"
  | "more-horizontal";

export const ICONS: Record<ForgeIconName, Component<LucideProps>> = {
  "git-branch": GitBranch,
  "git-pull-request": GitPullRequest,
  "arrow-left": ArrowLeft,
  settings: Settings,
  appearance: Palette,
  stats: ChartColumn,
  check: Check,
  loader: Loader2,
  filter: Filter,
  refresh: RefreshCw,
  copy: Copy,
  "external-link": ExternalLink,
  edit: Pencil,
  trash: Trash2,
  agent: Bot,
  "square-terminal": SquareTerminal,
  "panel-left-open": PanelLeftOpen,
  "panel-left-close": PanelLeftClose,
  "panel-right": PanelRight,
  "chevron-left": ChevronLeft,
  "chevron-right": ChevronRight,
  "chevron-down": ChevronDown,
  close: X,
  plus: Plus,
  "folder-open": FolderOpen,
  folder: Folder,
  file: File,
  "file-code": FileCode,
  search: Search,
  "message-square-plus": MessageSquarePlus,
  "columns-2": Columns2,
  "list-checks": ListChecks,
  history: History,
  "layout-grid": LayoutGrid,
  "more-horizontal": MoreHorizontal,
};

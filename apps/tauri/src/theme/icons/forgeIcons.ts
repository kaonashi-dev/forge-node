/**
 * The UI icon set: Forge's own glyphs (`glyphs.tsx`, one 16px grid) wherever
 * the set draws one, lucide for the rest.
 *
 * A name maps to a concrete component imported by name, so the bundler keeps
 * only the glyphs the app actually names — the registry is a lookup, not a
 * barrel re-export.
 *
 * Two kinds of mark are deliberately *not* here. A provider's or an editor's
 * logo is a brand, drawn from its own SVG by `BrandIcon`. A session's state —
 * running, starting, exited, failed, needs-you — is the app's own vocabulary,
 * not an icon, and is a geometric `StateMarker`.
 *
 * `bell` is the one glyph that also answers for a state: it takes the identity
 * slot of a session whose agent stopped to ask, the way `loader` takes it for
 * one that is working. The marker beside it still says which state.
 */
import {
  ArrowLeft,
  Bot,
  ChartColumn,
  ChevronDown,
  ChevronLeft,
  ChevronRight,
  ChevronUp,
  Copy,
  ExternalLink,
  File,
  Filter,
  FolderOpen,
  ImageIcon,
  ImageOffIcon,
  Maximize2,
  Loader2,
  MoreHorizontal,
  Palette,
  PanelLeftClose,
  PanelLeftOpen,
  Pencil,
  Plus,
  RefreshCw,
  Search,
  Keyboard,
  Sparkles,
  Settings,
  Trash2,
  X,
  type LucideProps,
} from "lucide-solid";
import type { Component } from "solid-js";
import { GLYPHS } from "./glyphs";

export type ForgeIconName =
  | "git-branch"
  | "git-pull-request"
  | "arrow-left"
  | "settings"
  | "appearance"
  | "stats"
  | "check"
  | "loader"
  | "bell"
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
  | "chevron-left"
  | "chevron-right"
  | "chevron-down"
  | "chevron-up"
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
  | "layers"
  | "maximize"
  | "more-horizontal"
  | "image"
  | "image-off"
  | "sparkles"
  | "keyboard"
  | "folder-symlink"
  | "worktree"
  | "commit"
  | "fetch"
  | "push"
  | "patch"
  | "merged"
  | "conflict"
  | "spawn-child"
  | "idle"
  | "exited"
  | "failed"
  | "command-palette";

export const ICONS: Record<ForgeIconName, Component<LucideProps>> = {
  "git-branch": GLYPHS.git,
  "git-pull-request": GLYPHS.pullRequest,
  "arrow-left": ArrowLeft,
  settings: Settings,
  appearance: Palette,
  stats: ChartColumn,
  check: GLYPHS.done,
  loader: Loader2,
  bell: GLYPHS.needsYou,
  filter: Filter,
  refresh: RefreshCw,
  copy: Copy,
  "external-link": ExternalLink,
  edit: Pencil,
  trash: Trash2,
  agent: Bot,
  "square-terminal": GLYPHS.terminal,
  "panel-left-open": PanelLeftOpen,
  "panel-left-close": PanelLeftClose,
  "chevron-left": ChevronLeft,
  "chevron-right": ChevronRight,
  "chevron-down": ChevronDown,
  "chevron-up": ChevronUp,
  close: X,
  plus: Plus,
  "folder-open": FolderOpen,
  folder: GLYPHS.files,
  file: File,
  "file-code": GLYPHS.code,
  search: Search,
  "message-square-plus": GLYPHS.handoff,
  "columns-2": GLYPHS.split,
  "list-checks": GLYPHS.review,
  history: GLYPHS.history,
  layers: GLYPHS.projects,
  maximize: Maximize2,
  "more-horizontal": MoreHorizontal,
  image: ImageIcon,
  "image-off": ImageOffIcon,
  sparkles: Sparkles,
  keyboard: Keyboard,
  "folder-symlink": GLYPHS.sharedFiles,
  worktree: GLYPHS.worktree,
  commit: GLYPHS.commit,
  fetch: GLYPHS.fetch,
  push: GLYPHS.push,
  patch: GLYPHS.patch,
  merged: GLYPHS.merged,
  conflict: GLYPHS.conflict,
  "spawn-child": GLYPHS.spawnChild,
  idle: GLYPHS.idle,
  exited: GLYPHS.exited,
  failed: GLYPHS.failed,
  "command-palette": GLYPHS.palette,
};

/**
 * The UI icon set: every name maps to a glyph Forge draws itself — the app's
 * own vocabulary in `glyphs.tsx`, the chrome's marks in `uiGlyphs.tsx`, all on
 * one 16px grid. There is no icon library behind this registry.
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
import type { Component } from "solid-js";
import { GLYPHS, type GlyphProps } from "./glyphs";
import { UI_GLYPHS } from "./uiGlyphs";

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
  | "play"
  | "stop"
  | "activity"
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
  | "minus"
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

export const ICONS: Record<ForgeIconName, Component<GlyphProps>> = {
  "git-branch": GLYPHS.git,
  "git-pull-request": GLYPHS.pullRequest,
  "arrow-left": UI_GLYPHS.arrowLeft,
  settings: UI_GLYPHS.settings,
  appearance: UI_GLYPHS.appearance,
  stats: UI_GLYPHS.stats,
  check: GLYPHS.done,
  loader: UI_GLYPHS.loader,
  bell: GLYPHS.needsYou,
  filter: UI_GLYPHS.filter,
  refresh: UI_GLYPHS.refresh,
  copy: UI_GLYPHS.copy,
  "external-link": UI_GLYPHS.externalLink,
  play: UI_GLYPHS.play,
  stop: UI_GLYPHS.stop,
  activity: UI_GLYPHS.activity,
  edit: UI_GLYPHS.edit,
  trash: UI_GLYPHS.trash,
  agent: UI_GLYPHS.agent,
  "square-terminal": GLYPHS.terminal,
  "panel-left-open": UI_GLYPHS.panelLeftOpen,
  "panel-left-close": UI_GLYPHS.panelLeftClose,
  "chevron-left": UI_GLYPHS.chevronLeft,
  "chevron-right": UI_GLYPHS.chevronRight,
  "chevron-down": UI_GLYPHS.chevronDown,
  "chevron-up": UI_GLYPHS.chevronUp,
  close: UI_GLYPHS.close,
  plus: UI_GLYPHS.plus,
  minus: UI_GLYPHS.minus,
  "folder-open": UI_GLYPHS.folderOpen,
  folder: GLYPHS.files,
  file: UI_GLYPHS.file,
  "file-code": GLYPHS.code,
  search: UI_GLYPHS.search,
  "message-square-plus": GLYPHS.handoff,
  "columns-2": GLYPHS.split,
  "list-checks": GLYPHS.review,
  history: GLYPHS.history,
  layers: GLYPHS.projects,
  maximize: UI_GLYPHS.maximize,
  "more-horizontal": UI_GLYPHS.moreHorizontal,
  image: UI_GLYPHS.image,
  "image-off": UI_GLYPHS.imageOff,
  sparkles: UI_GLYPHS.sparkles,
  keyboard: UI_GLYPHS.keyboard,
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

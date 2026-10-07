import type { Branches, DiffFile } from "../../contracts/workbench";

/** Bounds the base picker; a repository with hundreds of stale remote branches is not a menu. */
export const MAX_BASE_OPTIONS = 40;

/**
 * Refs a pull request from `head` could target, spelled as git resolves them.
 *
 * The default branch comes first because it is the usual answer, then remote
 * refs before local ones: a pull request is opened against what the forge
 * has, not against a local branch that may be behind it.
 */
export function baseRefOptions(
  branches: Branches | null,
  head: string | null,
  current: string | null,
): string[] {
  const refs: string[] = [];
  const add = (ref: string | null | undefined) => {
    if (ref && ref !== head && !refs.includes(ref)) refs.push(ref);
  };
  add(current);
  if (branches?.default_branch) add(`origin/${branches.default_branch}`);
  for (const branch of branches?.branches ?? []) {
    if (branch.scope !== "Local") add(`${branch.scope.Remote.remote}/${branch.name}`);
  }
  for (const branch of branches?.branches ?? []) {
    if (branch.scope === "Local") add(branch.name);
  }
  return refs.slice(0, MAX_BASE_OPTIONS);
}

/** The files the diff pane shows: every one, or the one picked in the rail. */
export function visibleFiles(files: DiffFile[], selected: string | null): DiffFile[] {
  if (selected === null) return files;
  const picked = files.filter((file) => file.path === selected);
  return picked.length > 0 ? picked : files;
}

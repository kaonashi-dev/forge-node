// Branch picker rows

import type { BranchRef, Branches } from "../workbench/types";
import type { ShellSnapshot } from "../runtime/types";
import { score } from "../palette/fuzzy";

export type BranchPickerChoice = {
  branch: string;
  base: string | null;
};

export type BranchPickerEntry = {
  label: string;
  group: string;
  note: string | null;
  enabled: boolean;
  choice: BranchPickerChoice | null;
};

export function branchEntries(
  query: string,
  branches: Branches | null,
  store: ShellSnapshot,
): BranchPickerEntry[] {
  const trimmed = query.trim();
  if (!branches) return [];

  const matches = (name: string) => !trimmed || score(trimmed, name) !== null;
  const out: BranchPickerEntry[] = [];

  if (trimmed && !branches.branches.some((branch) => branch.name === trimmed)) {
    const base = branches.default_branch;
    out.push({
      label: trimmed,
      group: "Create",
      note: base ? `new branch from ${base}` : "new branch from HEAD",
      enabled: plausibleBranchName(trimmed),
      choice: { branch: trimmed, base: base ?? null },
    });
  }

  for (const branch of branches.branches.filter((b) => !isRemote(b))) {
    if (!matches(branch.name)) continue;
    const occupied = branch.checked_out_in ? workspaceLabel(store, branch.checked_out_in) : null;
    const checkedOut = branch.checked_out_in !== null;
    out.push({
      label: branch.name,
      group: "Local branches",
      note: checkedOut ? `already open${occupied ? ` in ${occupied}` : ""}` : branch.subject,
      enabled: !checkedOut,
      choice: checkedOut ? null : { branch: branch.name, base: null },
    });
  }

  for (const branch of branches.branches.filter((b) => isRemote(b))) {
    if (!matches(branch.name)) continue;
    if (branches.branches.some((other) => !isRemote(other) && other.name === branch.name)) {
      continue;
    }
    const start = startPoint(branch);
    out.push({
      label: branch.name,
      group: "Remote branches",
      note: branch.subject ? `${start} · ${branch.subject}` : start,
      enabled: true,
      choice: { branch: branch.name, base: start },
    });
  }

  return out;
}

function isRemote(branch: BranchRef): boolean {
  return branch.scope !== "Local";
}

function startPoint(branch: BranchRef): string {
  if (branch.scope !== "Local" && typeof branch.scope === "object" && "Remote" in branch.scope) {
    const remote = branch.scope.Remote.remote;
    return `${remote}/${branch.name}`;
  }
  return branch.name;
}

function workspaceLabel(store: ShellSnapshot, workspaceId: string): string | null {
  const workspace = store.workspaces.find((item) => item.id === workspaceId);
  return workspace?.display_name ?? workspace?.branch ?? workspace?.path ?? null;
}

function plausibleBranchName(name: string): boolean {
  return (
    name.length > 0 &&
    !name.startsWith("-") &&
    !name.startsWith(".") &&
    !name.endsWith("/") &&
    !name.endsWith(".lock") &&
    !name.includes("..") &&
    !name.includes("//") &&
    !/[\s~^:?*[\]\\]/.test(name)
  );
}

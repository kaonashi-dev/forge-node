import { describe, expect, it } from "vitest";
import { emptySnapshot } from "../store/forgeStore";
import type { Branches } from "../workbench/types";
import { branchEntries } from "./branchPickerEntries";

describe("branchEntries", () => {
  it("keeps an occupied branch disabled when its workspace is missing locally", () => {
    const branches: Branches = {
      default_branch: "main",
      remotes: [],
      branches: [
        {
          name: "feature",
          scope: "Local",
          upstream: null,
          committed_at: null,
          subject: "work",
          checked_out_in: "missing-workspace",
        },
      ],
    };

    expect(branchEntries("", branches, emptySnapshot())).toEqual([
      {
        label: "feature",
        group: "Local branches",
        note: "already open",
        enabled: false,
        choice: null,
      },
    ]);
  });
});

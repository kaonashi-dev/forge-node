import { listen, type UnlistenFn } from "@tauri-apps/api/event";
import { toast } from "../../../ui/index";
import { setProjectDialogsStore } from "../../../features/projects/dialogs";
import {
  applyCandidates,
  applyShareActions,
  applySharesApplied,
  applyShareStatus,
  setSharesError,
} from "../../../features/projects/sharesStore";
import type {
  ShareAction,
  ShareCandidate,
  ShareStatusEntry,
  ShareTrigger,
} from "../../../contracts/runtime";
import type { Failure } from "../workbenchAnswer";

export function bindProjectsEvents(): Promise<UnlistenFn[]> {
  return Promise.all([
    // Candidates and rules belong to a project, so
    // they carry no workspace tag to check; status and plans do.
    listen<{ project: string; candidates: ShareCandidate[]; truncated: boolean }>(
      "workbench:share_candidates",
      ({ payload }) => applyCandidates(payload.project, payload.candidates, payload.truncated),
    ),
    listen<Failure>("workbench:share_candidates_failed", ({ payload }) =>
      setSharesError(payload.error),
    ),
    listen<{ workspace: string | null; actions: ShareAction[] }>(
      "workbench:share_plan",
      ({ payload }) => {
        if (payload.workspace) applyShareActions(payload.workspace, payload.actions);
      },
    ),
    listen<Failure>("workbench:share_plan_failed", ({ payload }) => setSharesError(payload.error)),
    listen<{ workspace: string; entries: ShareStatusEntry[] }>(
      "workbench:share_status",
      ({ payload }) => applyShareStatus(payload.workspace, payload.entries),
    ),
    listen<Failure>("workbench:share_status_failed", ({ payload }) =>
      setSharesError(payload.error),
    ),
    listen<Failure>("workbench:share_store_failed", ({ payload }) => setSharesError(payload.error)),
    // Provisioning acks when it *starts*; this is where a worktree stops
    // saying "setting up" and says what it got.
    listen<{
      workspace: string;
      trigger: ShareTrigger;
      actions: ShareAction[];
      error: string | null;
    }>("runtime:shares_applied", (event) => {
      applySharesApplied(
        event.payload.workspace,
        event.payload.trigger,
        event.payload.actions,
        event.payload.error,
      );
      const fell = event.payload.actions.filter((action) => action.fallback).length;
      if (fell > 0) {
        toast({
          title: fell === 1 ? "One file was copied, not cloned" : `${fell} files were copied`,
          detail: "This volume has no copy-on-write, so the clone fell back to a full copy.",
        });
      }
    }),
    listen<{ workspace: string; reason: string }>("runtime:worktree_blocked", (event) => {
      setProjectDialogsStore("worktreeRemoval", {
        workspace: event.payload.workspace,
        reason: event.payload.reason,
      });
    }),
    listen<{ project: string; branch: string; reason: string }>(
      "runtime:worktree_create_failed",
      (event) => setProjectDialogsStore("worktreeCreationFailure", event.payload),
    ),
  ]);
}

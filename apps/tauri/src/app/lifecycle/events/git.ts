import { listen, type UnlistenFn } from "@tauri-apps/api/event";
import { answer, failure, forCurrent, type Tagged } from "../workbenchAnswer";
import { applySessionChanges, failSessionChanges } from "../../../features/git/sessionChangesStore";
import { applyTranscript, failTranscript } from "../../../features/sessions/dialogs";
import { asSessionTranscript } from "../../../features/sessions/externalTranscript";
import {
  applyEditorConflict,
  failEditorConflict,
} from "../../../features/editor/conflict/editorConflictStore";
import type {
  Branches,
  JuvaDraft,
  RebaseState,
  SessionChanges,
  SessionTranscript,
  UsageAnalytics,
  WorkspaceDiff,
  WorkspaceReview,
} from "../../../contracts/workbench";
import type { ExternalTranscript } from "../../../contracts/runtime";
import { setGitStore } from "../../../features/git/state";
import { setSettingsStore } from "../../../features/settings/state";
import { setLoading } from "../../../state/loading";

type SessionFailure = { session: string; error: string };

export function bindGitEvents(): Promise<UnlistenFn[]> {
  /* Keyed on the session, not the checkout: the split lives beside one
     terminal, and a session's answer is still its own after the workbench has
     been pointed somewhere else. */
  const sessionAnswer = <T>(event: string, apply: (session: string, value: T) => void) =>
    listen<Tagged<T>>(event, ({ payload: [session, value] }) => apply(session, value));

  return Promise.all([
    answer<WorkspaceDiff>("workbench:diff", "diff", (diff) =>
      setGitStore({ diff, diffError: null }),
    ),
    failure("workbench:diff_failed", "diff", (error) => setGitStore("diffError", error)),
    answer<WorkspaceReview>("workbench:review", "review", (review) =>
      setGitStore({ review, reviewError: null, reviewAt: Date.now() }),
    ),
    failure("workbench:review_failed", "review", (error) => setGitStore("reviewError", error)),
    sessionAnswer<SessionChanges>("workbench:session_changes", applySessionChanges),
    listen<SessionFailure>("workbench:session_changes_failed", ({ payload }) =>
      failSessionChanges(payload.session, payload.error),
    ),
    /* Four-tuple rather than the usual pair: the answer names the path it is
       about, because a reload can land between the ask and the answer. */
    listen<[string, string, string, string]>(
      "workbench:editor_conflict",
      ({ payload: [session, path, disk, mine] }) =>
        applyEditorConflict(session, { path, disk, mine }),
    ),
    listen<SessionFailure>("workbench:editor_conflict_failed", ({ payload }) =>
      failEditorConflict(payload.session, payload.error),
    ),
    sessionAnswer<SessionTranscript>("workbench:session_transcript", applyTranscript),
    listen<SessionFailure>("workbench:session_transcript_failed", ({ payload }) =>
      failTranscript(payload.session, payload.error),
    ),
    /* A discovered run resolves into the same handoff slot as a live one: both
       are keyed by the correlation id the request carried, and only the shape
       of the capture differs. */
    sessionAnswer<ExternalTranscript>("workbench:external_transcript", (session, transcript) =>
      applyTranscript(session, asSessionTranscript(transcript)),
    ),
    listen<SessionFailure>("workbench:external_transcript_failed", ({ payload }) =>
      failTranscript(payload.session, payload.error),
    ),
    answer<RebaseState>("workbench:rebase", "rebase", (rebase) =>
      setGitStore({ rebase, rebaseError: null }),
    ),
    failure("workbench:rebase_failed", "rebase", (error) => setGitStore("rebaseError", error)),
    // Branches are a project's, not a workspace's, so they carry no tag to
    // check and land whoever asked.
    listen<{
      project: string;
      branches: Branches["branches"];
      remotes: Branches["remotes"];
      default_branch: string | null;
    }>("workbench:branches", ({ payload }) => {
      setLoading("branches", false);
      setGitStore("branches", {
        branches: payload.branches,
        remotes: payload.remotes,
        default_branch: payload.default_branch,
      });
    }),
    listen<UsageAnalytics>("workbench:usage", ({ payload }) => {
      setLoading("usage", false);
      setSettingsStore({ usage: payload, usageError: null });
    }),
    listen<{ error: string }>("workbench:usage_failed", ({ payload }) => {
      setLoading("usage", false);
      setSettingsStore("usageError", payload.error);
    }),
    // Juva acks when the draft starts, so the text arrives on the runtime
    // thread's own event, not as a workbench answer.
    listen<{ workspace: string; draft: JuvaDraft; fell_back: boolean }>(
      "runtime:juva_draft",
      ({ payload }) => {
        setLoading("juva", false);
        if (!forCurrent(payload.workspace)) return;
        setGitStore({ juvaDraft: payload.draft, juvaError: null });
      },
    ),
    listen<{ workspace: string | null; error: string }>("workbench:juva_failed", ({ payload }) => {
      setLoading("juva", false);
      setGitStore("juvaError", payload.error);
    }),
    listen<string>("workbench:juva_applied", () => {
      setGitStore({ juvaDraft: null, juvaError: null });
    }),
  ]);
}

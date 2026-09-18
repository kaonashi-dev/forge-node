import { listen, type UnlistenFn } from "@tauri-apps/api/event";
import {
  fileWatchReady,
  rearmFileWatches,
  failFileWatch,
} from "../../../features/files/watches/fileWatch";
import { fileChangesChannel } from "../../../runtime/bus";
import { refreshDiff } from "../../../features/git/decorations";
import { dataUrl } from "../../../features/files/preview/previewImages";
import {
  directories,
  type DirectoryAnswer,
  type DirectoryFailure,
} from "../../../features/files/directories/directoryState";
import {
  pathOperations,
  type PathOperationResult,
} from "../../../features/files/operations/operations";
import type {
  FileContents,
  FileTree,
  ImageContents,
  SearchResults,
} from "../../../contracts/workbench";
import { answer, failure, forCurrent, type Tagged } from "../workbenchAnswer";
import {
  acceptFileIndex,
  failFileIndex,
  previewImageReader,
} from "../../../features/files/commands";
import { filesStore, setFilesStore, invalidateFileIndex } from "../../../features/files/state";

export function bindFilesEvents(): Promise<UnlistenFn[]> {
  return Promise.all([
    listen<[string, string]>("workbench:file_changed", ({ payload }) => {
      invalidateFileIndex(payload[0]);
      directories.changed(payload[0], payload[1]);
      rearmFileWatches(payload[0], payload[1]);
      fileChangesChannel.publish(payload);
    }),
    listen<{ workspace: string; generation: number }>("workbench:watch_ready", ({ payload }) =>
      fileWatchReady(payload.workspace, payload.generation),
    ),
    listen<{ workspace: string; generation: number; error: string }>(
      "workbench:watch_failed",
      ({ payload }) => failFileWatch(payload.workspace, payload.error, payload.generation),
    ),
    listen<PathOperationResult>("workbench:path_result", ({ payload }) =>
      pathOperations.settle(payload),
    ),
    listen<{ workspace: string; request_id: string; tree: FileTree }>(
      "workbench:file_tree",
      ({ payload }) => acceptFileIndex(payload),
    ),
    listen<{ workspace: string; request_id: string; error: string }>(
      "workbench:file_tree_failed",
      ({ payload }) => failFileIndex(payload),
    ),
    listen<DirectoryAnswer>("workbench:directory", ({ payload }) => directories.accept(payload)),
    listen<DirectoryFailure>("workbench:directory_failed", ({ payload }) =>
      directories.fail(payload),
    ),
    answer<FileContents>("workbench:file", "file", (file) =>
      setFilesStore({ file, fileError: null }),
    ),
    answer<FileContents>("workbench:file_saved", "file", (file) => {
      setFilesStore({ file, fileError: null });
      // The write may have changed marks the gutter already shows.
      refreshDiff();
    }),
    failure("workbench:file_failed", "file", (error) => setFilesStore("fileError", error)),
    failure("workbench:save_failed", "file", (error) => setFilesStore("fileError", error)),
    /* Not checked against the current workspace: the reader matches on the
       workspace it was asked for, and a preview that moved on has stopped
       listening for the answer. */
    listen<Tagged<ImageContents>>("workbench:image", ({ payload: [workspace, image] }) =>
      previewImageReader.settle(workspace, image.path, { url: dataUrl(image) }),
    ),
    listen<{ workspace: string; path: string; error: string }>(
      "workbench:image_failed",
      ({ payload }) =>
        previewImageReader.settle(payload.workspace, payload.path, { error: payload.error }),
    ),
    answer<SearchResults>("workbench:search", "search", (search) =>
      setFilesStore({
        search,
        searchError: null,
        searchStale: filesStore.searchReadVersion !== filesStore.treeVersion,
      }),
    ),
    failure("workbench:search_failed", "search", (error) => setFilesStore("searchError", error)),
    answer<SearchResults>("workbench:name_search", "nameSearch", (search) =>
      setFilesStore({ nameSearch: search, nameSearchError: null }),
    ),
    failure("workbench:name_search_failed", "nameSearch", (error) =>
      setFilesStore("nameSearchError", error),
    ),
  ]);
}

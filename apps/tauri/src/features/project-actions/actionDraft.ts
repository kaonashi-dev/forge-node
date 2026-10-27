import type {
  ActionKind,
  ActionSuggestion,
  ProjectAction,
  RepoActionsFile,
} from "../../contracts/runtime";

export type DraftOrigin = "saved" | "suggested" | "repo";

/** One row of the actions dialog. Nothing reaches the daemon until Save. */
export type DraftRow = {
  action: ProjectAction;
  origin: DraftOrigin;
  include: boolean;
  /** The file a suggestion came from, such as `package.json`. */
  detectedFrom: string | null;
  /** Labels a suggestion depends on, resolved to ids when the set is built. */
  dependsOnLabels: readonly string[];
  /** A saved repository action whose file has changed since it was imported. */
  drift: boolean;
};

const PRECHECKED: readonly ActionKind[] = ["dev", "test", "lint"];

type Mint = () => string;

function fromSuggestion(
  suggestion: ActionSuggestion,
  projectId: string,
  id: string,
  now: string,
): ProjectAction {
  return {
    id,
    project_id: projectId,
    label: suggestion.label,
    command: suggestion.command,
    kind: suggestion.kind,
    cwd: suggestion.cwd ?? null,
    single_instance: suggestion.single_instance ?? true,
    pinned: suggestion.pinned,
    ready: suggestion.ready ?? { kind: "none" },
    port: suggestion.port ?? null,
    depends_on: [],
    source: suggestion.source,
    position: 0,
    created_at: now,
  };
}

/**
 * Saved actions first, then detected suggestions that are not saved yet
 * (servers, tests and linters prechecked), then what the repository file
 * offers (never prechecked: importing is the consent). A repository action
 * already saved from an older version of the file is offered again as an
 * update, unchecked.
 */
export function buildDraft(
  projectId: string,
  saved: readonly ProjectAction[],
  suggestions: readonly ActionSuggestion[],
  repoFile: RepoActionsFile | null,
  mint: Mint = () => crypto.randomUUID(),
  now: string = new Date().toISOString(),
): DraftRow[] {
  const drifted = new Set(repoFile?.drift ?? []);
  const rows: DraftRow[] = [...saved]
    .sort((a, b) => a.position - b.position)
    .map((action) => ({
      action,
      origin: "saved",
      include: true,
      detectedFrom: null,
      dependsOnLabels: [],
      drift: drifted.has(action.id),
    }));
  for (const suggestion of suggestions) {
    if (suggestion.saved_as !== null) continue;
    rows.push({
      action: fromSuggestion(suggestion, projectId, mint(), now),
      origin: "suggested",
      include: PRECHECKED.includes(suggestion.kind),
      detectedFrom: suggestion.detected_from,
      dependsOnLabels: suggestion.depends_on ?? [],
      drift: false,
    });
  }
  for (const suggestion of repoFile?.actions ?? []) {
    const replaces = suggestion.saved_as;
    if (replaces !== null && !drifted.has(replaces)) continue;
    rows.push({
      action: fromSuggestion(suggestion, projectId, replaces ?? mint(), now),
      origin: "repo",
      include: false,
      detectedFrom: suggestion.detected_from,
      dependsOnLabels: suggestion.depends_on ?? [],
      drift: false,
    });
  }
  return rows;
}

/**
 * The set Save sends: included rows in order, a repository update replacing
 * the saved row it shares an id with, and dependencies kept only among what is
 * being saved — a suggestion's by label, an edited row's by id. A missing one
 * is dropped, not invented, since the daemon refuses a set that names it.
 */
export function toActionSet(rows: readonly DraftRow[]): ProjectAction[] {
  const included = rows.filter((row) => row.include);
  const updates = new Set(
    included.filter((row) => row.origin === "repo").map((row) => row.action.id),
  );
  const kept = included.filter((row) => !(row.origin === "saved" && updates.has(row.action.id)));
  const byLabel = new Map(kept.map((row) => [row.action.label.toLowerCase(), row.action.id]));
  const keptIds = new Set(kept.map((row) => row.action.id));
  return kept.map((row, position) => {
    const resolved = row.dependsOnLabels
      .map((label) => byLabel.get(label.trim().toLowerCase()))
      .filter((id): id is string => id !== undefined && id !== row.action.id);
    const depends_on =
      row.dependsOnLabels.length > 0
        ? resolved
        : (row.action.depends_on ?? []).filter((id) => keptIds.has(id) && id !== row.action.id);
    return { ...row.action, depends_on, position };
  });
}

/** Labels that collide ignoring case, which the daemon would refuse. */
export function duplicateLabels(rows: readonly DraftRow[]): string[] {
  const seen = new Map<string, number>();
  for (const row of rows) {
    if (!row.include) continue;
    const label = row.action.label.trim().toLowerCase();
    seen.set(label, (seen.get(label) ?? 0) + 1);
  }
  return [...seen].filter(([, count]) => count > 1).map(([label]) => label);
}

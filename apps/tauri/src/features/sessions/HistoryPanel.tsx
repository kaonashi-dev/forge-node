import { For, Show, createMemo, createSignal } from "solid-js";
import { newAgent } from "./commands";
import { refreshSnapshot } from "../../runtime/host";
import type { ExternalAgentSession } from "../../contracts/runtime";
import { historyMenuItems } from "./historyMenuItems";
import { forgeStore } from "../../state/forgeStore";
import { requestConfirm } from "../../state/dialogs";
import { SessionGlyph } from "./SessionGlyph";
import { Icon } from "../../theme/icons/index";
import { Button, FilterHeader, ListCard, Menu, Tooltip, buttonClass, toast } from "../../ui/index";
import { deleteExternalSession } from "./commands";
import { activeWorkspace } from "../../state/workspace";
import { hasAllTerms, termsOf } from "../../shared/search/terms";

type Scope = "checkout" | "project" | "everywhere";

/* Hoisted: the three scopes never change, and an array built inside the JSX is
   a new array on every render — which is a new set of chips for the strip to
   diff each time one of them is clicked. */
const SCOPES: ReadonlyArray<{ value: Scope }> = [
  { value: "checkout" },
  { value: "project" },
  { value: "everywhere" },
];

/** ISO-8601 stamps of one format order as strings; a locale collation is not needed. */
function newestFirst(a: string, b: string): number {
  return a < b ? 1 : a > b ? -1 : 0;
}

export function HistoryPanel() {
  const [scope, setScope] = createSignal<Scope>("checkout");
  const [query, setQuery] = createSignal("");

  const workspace = () =>
    forgeStore.workspaces.find((item) => item.id === activeWorkspace()) ?? null;

  /* Sorted and lowered when the store changes, so a filter keystroke only
     filters. Most recent first: history is read from the top. */
  const indexed = createMemo(() =>
    forgeStore.external_agents
      .map((session) => ({
        session,
        haystack: `${session.title} ${session.branch ?? ""} ${session.provider}`.toLowerCase(),
      }))
      .sort((a, b) => newestFirst(a.session.last_activity, b.session.last_activity)),
  );

  const sessions = createMemo(() => {
    const terms = termsOf(query());
    const current = workspace();
    const within = scope();
    return indexed()
      .filter(({ session, haystack }) => {
        if (within === "checkout" && session.workspace_id !== current?.id) return false;
        if (within === "project" && session.project_id !== current?.project_id) return false;
        return hasAllTerms(haystack, terms);
      })
      .map(({ session }) => session);
  });

  /** The profile that recorded a run, when it still exists. */
  function account(session: ExternalAgentSession) {
    if (!session.profile_id) return null;
    return forgeStore.agent_profiles.find((item) => item.id === session.profile_id) ?? null;
  }

  /**
   * Whether the account that recorded this run can be launched.
   *
   * The account, not the provider: a run recorded under a profile's config
   * directory only exists for a CLI started with that same directory, so the
   * row that matters is that profile's — which is also the one that stays
   * launchable when the provider is only reachable through a profile's own
   * executable.
   */
  function canResume(session: ExternalAgentSession): boolean {
    return forgeStore.launchables.some(
      (item) =>
        item.provider === session.provider && item.profile === session.profile_id && item.enabled,
    );
  }

  function blockedReason(session: ExternalAgentSession): string | null {
    if (canResume(session)) return null;
    if (session.profile_id && !account(session)) {
      return "The profile that recorded this run no longer exists";
    }
    return `${session.provider} is not installed`;
  }

  function resume(session: ExternalAgentSession): void {
    // The ordinary agent launch, told which conversation to re-enter — there
    // is no second kind of agent, and the provider's own CLI is what resumes.
    // The profile travels with it: the transcript lives in that account's
    // config directory, and the default account has never heard of its id.
    void newAgent(
      session.provider,
      session.profile_id,
      session.workspace_id,
      session.session_id,
    ).catch(() => undefined);
  }

  /**
   * The description names what is *not* lost: someone reading "delete" in a
   * tool that manages git worktrees has every reason to fear the checkout.
   */
  function confirmDelete(session: ExternalAgentSession): void {
    requestConfirm({
      title: `Delete "${session.title}"?`,
      description:
        `Removes ${session.provider}'s transcript for this run from disk. ` +
        "The conversation cannot be resumed or read afterwards. " +
        "Your code and branches are untouched.",
      confirmLabel: "Delete",
      destructive: true,
      onConfirm: () => {
        void deleteExternalSession(session.session_id, session.provider, session.profile_id)
          // History reaches the GUI only on the snapshot: there is no
          // `ExternalAgentsChanged` event to wait for.
          .then(() => refreshSnapshot())
          .catch((error: unknown) =>
            toast({
              title: "Could not delete the transcript.",
              detail: error instanceof Error ? error.message : undefined,
              tone: "danger",
            }),
          );
      },
    });
  }

  return (
    <div class="panel-body">
      <FilterHeader
        label="Filter history"
        placeholder="Filter history…"
        query={query()}
        onQuery={setQuery}
        rows={[
          {
            label: "Scope",
            value: scope(),
            onChange: (value) => setScope(value as Scope),
            options: SCOPES,
          },
        ]}
      />
      <For each={sessions()} fallback={<p class="empty-copy">No agent runs recorded here.</p>}>
        {(session) => (
          <ListCard
            class="history-card"
            variant="flat"
            glyph={<SessionGlyph providerId={session.provider} size={14} />}
            title={session.title}
            aside={<span class="forge-caption history-provider">{session.provider}</span>}
            meta={
              <>
                <Show when={session.branch}>
                  {(branch) => <span class="history-branch">{branch()}</span>}
                </Show>
                <Show when={account(session)}>
                  {(profile) => <span class="history-account">{profile().name}</span>}
                </Show>
                <span>{session.message_count} turns</span>
                <Show when={session.subagent_count > 0}>
                  <span>{session.subagent_count} subagents</span>
                </Show>
              </>
            }
            actions={
              <>
                <Show
                  when={blockedReason(session)}
                  fallback={
                    <Button size="xs" onClick={() => resume(session)}>
                      Resume
                    </Button>
                  }
                >
                  {(reason) => (
                    <Tooltip label={reason()}>
                      <Button size="xs" disabled onClick={() => resume(session)}>
                        Resume
                      </Button>
                    </Tooltip>
                  )}
                </Show>
                <Menu
                  triggerClass={buttonClass("xs", "ghost", true)}
                  triggerLabel={`More actions for ${session.title}`}
                  trigger={<Icon name="more-horizontal" size={14} />}
                  items={historyMenuItems(session, () => confirmDelete(session))}
                />
              </>
            }
          >
            <Show when={session.preview}>
              {(preview) => <p class="history-preview">{preview()}</p>}
            </Show>
          </ListCard>
        )}
      </For>
    </div>
  );
}

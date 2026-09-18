import { For, Show } from "solid-js";
import * as harness from "../harness/api";
import { featureLabel, statusLabel } from "../../contracts/harness";
import { forgeStore } from "../../state/forgeStore";
import { harnessStore } from "../harness/harnessStore";
import { invokeAction } from "../../actions/dispatch";
import { Button } from "../../ui/index";
import { Group, Page } from "./SettingsLayout";
import { activeWorkspace } from "../../state/workspace";

export function HarnessSection() {
  const workspace = () =>
    forgeStore.workspaces.find((item) => item.id === activeWorkspace()) ?? null;
  const project = () => workspace()?.project_id ?? null;

  return (
    <Page title="Harness" summary="Whether this repository has a harness, and what is in it.">
      <Show
        when={project()}
        fallback={<p class="empty-copy">Open a checkout to reach its harness.</p>}
      >
        {(id) => (
          <>
            <Group title="State">
              <dl class="settings-facts">
                <dt>State file</dt>
                <dd>{harnessStore.initialized ? "harness/features.json" : "not initialized"}</dd>
                <dt>Features</dt>
                <dd>{harnessStore.features.length} registered</dd>
                <dt>At the gate</dt>
                <dd>
                  {harnessStore.features.filter((f) => f.status === "spec_ready").length} waiting
                </dd>
              </dl>

              <div class="settings-actions">
                <Button
                  variant="secondary"
                  onClick={() => void harness.loadFeatures(id()).catch(() => undefined)}
                >
                  Re-read features
                </Button>
                <Button
                  variant="secondary"
                  onClick={() => void harness.validateHarness(id()).catch(() => undefined)}
                >
                  Validate
                </Button>
              </div>

              {/* `ok: false` is an answer, not a failure: the output is the
                report, and it is the interesting case. */}
              <Show when={harnessStore.validate}>
                {(result) => (
                  <section
                    class="feature-validate"
                    classList={{ ok: result().ok, bad: !result().ok }}
                  >
                    <h3>{result().ok ? "Harness validates" : "Harness does not validate"}</h3>
                    <pre class="feature-validate-output">{result().output}</pre>
                  </section>
                )}
              </Show>
            </Group>

            <Group
              title="Registering work"
              description="A feature starts as a draft tab: a title, a spec, and Register. Reachable from the + menu, the New button in the Features panel, or the command palette."
            >
              <div class="settings-actions">
                {/* Through the action, not the store: the draft is a centre
                  tab and this screen is covering it. */}
                <Button variant="primary" onClick={() => invokeAction("new_feature")}>
                  New feature…
                </Button>
              </div>

              <Show when={harnessStore.listError}>
                {(error) => <p class="panel-error">{error()}</p>}
              </Show>
            </Group>

            <Group title="Registered">
              <For
                each={harnessStore.features}
                fallback={<p class="empty-copy">Nothing registered yet.</p>}
              >
                {(feature) => (
                  <div class="settings-row">
                    <span class="feature-id">#{feature.id}</span>
                    <span class="settings-row-label">{featureLabel(feature)}</span>
                    <span class="settings-row-note">{statusLabel(feature.status)}</span>
                  </div>
                )}
              </For>
            </Group>
          </>
        )}
      </Show>
    </Page>
  );
}

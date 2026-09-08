import { Show, createEffect, createMemo, createSignal } from "solid-js";
import * as harness from "../harness/api";
import { featureIsOpen, featureLabel } from "../harness/types";
import { forgeStore } from "../store/forgeStore";
import { harnessStore, openFeature, setHarnessStore } from "../store/harnessStore";
import { workbenchStore } from "../store/workbenchStore";
import { close, openFeatureView } from "../store/viewsStore";
import { Button, TextArea, TextField } from "../ui";
import { FEATURE_COMPOSE_VIEW } from "./views";

/**
 * The draft a feature starts as.
 *
 * Sibling of `PrComposeView`: a centre tab because the input is a spec, and a
 * spec is paragraphs — a dialog over the terminal would offer three lines for
 * the one field the whole cycle is derived from.
 *
 * Registration is the only thing here. The daemon mints the number, writes the
 * spec under `harness/specs/` and starts the spec step; the moment it answers,
 * this tab is replaced by the feature's own, which is where the cycle is
 * driven.
 */
export function FeatureComposeView() {
  const [title, setTitle] = createSignal("");
  const [spec, setSpec] = createSignal("");
  const [issue, setIssue] = createSignal("");
  const [sent, setSent] = createSignal(false);

  const workspace = () =>
    forgeStore.workspaces.find((item) => item.id === workbenchStore.workspace) ?? null;
  const project = () => workspace()?.project_id ?? null;

  // A draft that is not for the project on screen is not this draft: the
  // number the daemon is about to mint would open a feature in the other one.
  createEffect(() => {
    project();
    setSent(false);
    setHarnessStore("registered", null);
  });

  // The register command acks without the number, so the answer is what hands
  // over: swap the draft for the feature it became.
  createEffect(() => {
    const id = harnessStore.registered;
    if (!sent() || id === null) return;
    setHarnessStore("registered", null);
    close(FEATURE_COMPOSE_VIEW);
    openFeatureView(id);
  });

  // The daemon refuses asynchronously — the command acks and the reason
  // arrives as `harness:failed` — so the button has to be released by the
  // error, not by the promise that already resolved.
  createEffect(() => {
    if (harnessStore.listError !== null) setSent(false);
  });

  /**
   * The feature already open in the checkout this draft is for.
   *
   * One open feature per checkout is a rule of the harness, so a refused draft
   * has exactly one thing in its way; naming it is what turns "finish or block
   * it" into something the reader can click.
   */
  const blocking = createMemo(() => {
    if (harnessStore.listError === null) return null;
    const anchor = workspace()?.id ?? null;
    if (anchor === null) return null;
    return (
      harnessStore.features.find((item) => item.workspace_id === anchor && featureIsOpen(item)) ??
      null
    );
  });

  function openBlocking(): void {
    const feature = blocking();
    const id = project();
    if (!feature) return;
    openFeature(feature.id);
    close(FEATURE_COMPOSE_VIEW);
    openFeatureView(feature.id);
    if (id) void harness.loadFeatureDetail(id, feature.id).catch(() => undefined);
  }

  function register(): void {
    const id = project();
    const text = spec().trim();
    if (!id || text === "" || sent()) return;
    setSent(true);
    setHarnessStore("registered", null);
    void harness
      .registerFeature(id, text, title().trim() || null, workspace()?.id ?? null)
      .catch(() => setSent(false));
  }

  function fromIssue(): void {
    const id = project();
    const number = Number.parseInt(issue().trim(), 10);
    if (!id || !Number.isFinite(number) || number <= 0 || sent()) return;
    setSent(true);
    setHarnessStore("registered", null);
    void harness.registerFromIssue(id, number, workspace()?.id ?? null).catch(() => setSent(false));
  }

  return (
    <section class="feature-view" aria-label="New feature">
      <header class="feature-view-head">
        <h2 class="feature-title">New feature</h2>
        <span class="feature-status">draft</span>
        <span class="history-spacer" />
        <Button variant="secondary" onClick={() => close(FEATURE_COMPOSE_VIEW)}>
          Close
        </Button>
      </header>

      <Show
        when={project()}
        fallback={
          <div class="feature-view-body">
            <p class="empty-copy">Open a checkout to register a feature in its harness.</p>
          </div>
        }
      >
        <div class="feature-view-body">
          <Show when={!harnessStore.initialized}>
            <p class="feature-isolation">
              This project has no <code>harness/</code> directory, so registering will fail until it
              is initialized.
            </p>
          </Show>

          <section class="feature-compose-form">
            <p class="panel-note">
              The spec is the whole input: the daemon writes it to <code>harness/specs/</code>, runs
              the spec step from it, and stops at the human gate.
            </p>
            <TextField
              label="Title (optional)"
              value={title()}
              onChange={setTitle}
              placeholder="Derived from the spec when empty"
            />
            <TextArea
              label="Spec"
              rows={12}
              value={spec()}
              onChange={setSpec}
              placeholder="What should change, and how you will know it worked."
            />
            <div class="feature-actions">
              <Button
                variant="primary"
                disabled={spec().trim() === "" || sent()}
                onClick={register}
              >
                {sent() ? "Registering…" : "Register"}
              </Button>
            </div>
          </section>

          <section class="feature-compose-form">
            <h3>From a GitHub issue</h3>
            <p class="panel-note">
              The daemon reads the issue through gh and uses its body as the spec.
            </p>
            <div class="feature-actions">
              <TextField
                class="panel-search"
                aria-label="Issue number"
                value={issue()}
                onChange={setIssue}
                placeholder="Issue number"
              />
              <Button
                variant="secondary"
                disabled={issue().trim() === "" || sent()}
                onClick={fromIssue}
              >
                Register from issue
              </Button>
            </div>
          </section>

          {/* The refusal keeps what was typed: "that checkout already has an
              open feature" is answered by editing or by going elsewhere, and
              throwing away the draft to show the reason helps with neither. */}
          <Show when={harnessStore.listError}>
            {(error) => (
              <section class="feature-draft-error">
                <p class="panel-error">{error()}</p>
                <Show when={blocking()}>
                  {(feature) => (
                    <Button variant="secondary" size="xs" onClick={openBlocking}>
                      Open #{feature().id} · {featureLabel(feature())}
                    </Button>
                  )}
                </Show>
              </section>
            )}
          </Show>
        </div>
      </Show>
    </section>
  );
}

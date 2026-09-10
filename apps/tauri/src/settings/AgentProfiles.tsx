import { For, Show, createEffect, createMemo, createSignal } from "solid-js";
import { removeAgentProfile, saveAgentProfile, type AgentProfile } from "../runtime/api";
import type { ConfigDirSpec } from "../runtime/types";
import { forgeStore } from "../store/forgeStore";
import { runtimeStore, setRuntimeStore } from "../store/runtimeStore";
import { formatArgs, newAgentProfile, normalizeArgs, parseArgs } from "./agentProfile";
import { Button, TextArea, TextField } from "../ui";
import { Group } from "./SettingsLayout";

type EditingProfile = { profile: AgentProfile; isNew: boolean };

export function AgentProfiles() {
  const [editing, setEditing] = createSignal<EditingProfile | null>(null);

  const providers = createMemo(() =>
    forgeStore.providers
      .map((provider) => provider.descriptor)
      .filter((descriptor): descriptor is { id: string; display_name?: string } =>
        Boolean(descriptor),
      ),
  );

  return (
    <Group
      title="Launch profiles"
      description="Save a custom executable, account, or launch arguments. Profiles appear in the + menu."
      class="settings-profiles"
    >
      <For each={forgeStore.agent_profiles} fallback={<p class="empty-copy">No profiles yet.</p>}>
        {(profile) => (
          <div class="settings-row profile-row">
            <div class="profile-row-main">
              <span class="settings-row-label">{profile.name}</span>
              <span class="settings-row-note">{profile.provider_id}</span>
              <Show when={profile.executable}>
                {(executable) => <span class="settings-row-note">{executable()}</span>}
              </Show>
              <Show when={profile.config_dir}>
                {(dir) => <span class="settings-row-note">{dir()}</span>}
              </Show>
            </div>
            <div class="profile-row-actions">
              <Button
                variant="secondary"
                size="xs"
                onClick={() => setEditing({ profile: { ...profile }, isNew: false })}
              >
                Edit
              </Button>
              <Button
                variant="danger"
                size="xs"
                onClick={() => void removeAgentProfile(profile.id).catch(() => undefined)}
              >
                Delete
              </Button>
            </div>
          </div>
        )}
      </For>

      <div class="settings-actions">
        <For each={providers()}>
          {(descriptor) => (
            <Button
              variant="secondary"
              size="xs"
              onClick={() => setEditing({ profile: newAgentProfile(descriptor.id), isNew: true })}
            >
              New {descriptor.display_name ?? descriptor.id} profile
            </Button>
          )}
        </For>
      </div>

      <Show when={editing()}>
        {(state) => (
          <ProfileForm
            profile={state().profile}
            isNew={state().isNew}
            onCancel={() => setEditing(null)}
            onSaved={() => setEditing(null)}
          />
        )}
      </Show>
    </Group>
  );
}

function ProfileForm(props: {
  profile: AgentProfile;
  isNew: boolean;
  onCancel: () => void;
  onSaved: () => void;
}) {
  // Absent for a provider that documents no directory of its own (Cursor CLI):
  // the daemon refuses one, so the form does not offer it.
  const configDir = createMemo<ConfigDirSpec | null>(
    () =>
      forgeStore.providers.find((provider) => provider.descriptor?.id === props.profile.provider_id)
        ?.descriptor?.config_dir ?? null,
  );
  const [name, setName] = createSignal(props.profile.name);
  const [executable, setExecutable] = createSignal(props.profile.executable ?? "");
  const [dir, setDir] = createSignal(props.profile.config_dir ?? "");
  const [args, setArgs] = createSignal(formatArgs(normalizeArgs(props.profile.args)));
  const [saving, setSaving] = createSignal(false);
  const [saveError, setSaveError] = createSignal<string | null>(null);

  const parsedArgs = createMemo(() => parseArgs(args()));
  const validationError = createMemo(() => {
    if (name().trim() === "") return "A profile needs a name.";
    return parsedArgs().error;
  });
  const problem = () => validationError() ?? saveError();

  function submit(): void {
    if (validationError()) return;
    const profile: AgentProfile = {
      ...props.profile,
      name: name().trim(),
      executable: executable().trim() || null,
      config_dir: configDir() ? dir().trim() || null : null,
      args: parsedArgs().args,
    };
    setSaveError(null);
    setSaving(true);
    setRuntimeStore("profileSave", null);
    void saveAgentProfile(profile).catch((reason: unknown) => {
      setSaving(false);
      setSaveError(String(reason));
    });
  }

  createEffect(() => {
    const result = runtimeStore.profileSave;
    if (!result || result.profile !== props.profile.id) return;
    setRuntimeStore("profileSave", null);
    setSaving(false);
    if (result.error) setSaveError(result.error);
    else props.onSaved();
  });

  return (
    <form
      class="settings-form"
      onSubmit={(event) => {
        event.preventDefault();
        submit();
      }}
    >
      <h4>{props.isNew ? "New profile" : `Edit ${props.profile.name}`}</h4>
      <TextField
        class="settings-field"
        label="Name"
        value={name()}
        onChange={setName}
        placeholder="Personal"
      />
      <TextField
        class="settings-field"
        label="Executable"
        description="A path, or the name of a wrapper on your PATH. Empty inherits the detected binary."
        value={executable()}
        onChange={setExecutable}
        placeholder="Inherit the detected binary"
      />
      <Show when={configDir()}>
        {(spec) => (
          <TextField
            class="settings-field"
            label="Config directory"
            description={`${spec().help} Relative to your home directory unless it starts with /.`}
            value={dir()}
            onChange={setDir}
            placeholder=".claude-personal"
          />
        )}
      </Show>
      <TextArea
        class="settings-field"
        label="Additional arguments"
        description="Written like a command line: spaces separate arguments, and a new line is just another space. Quote one that contains a space; no shell runs them."
        rows={3}
        value={args()}
        onChange={setArgs}
        placeholder="--model opus"
      />
      <Show when={problem()}>{(message) => <p class="panel-error">{message()}</p>}</Show>
      <div class="settings-actions">
        <Button
          variant="primary"
          disabled={Boolean(validationError()) || saving()}
          onClick={() => submit()}
        >
          {saving() ? "Saving…" : "Save"}
        </Button>
        <Button variant="secondary" disabled={saving()} onClick={props.onCancel}>
          Cancel
        </Button>
      </div>
    </form>
  );
}

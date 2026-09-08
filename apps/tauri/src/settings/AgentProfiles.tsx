import { For, Show, createEffect, createMemo, createSignal } from "solid-js";
import { removeAgentProfile, saveAgentProfile, type AgentProfile } from "../runtime/api";
import type { ProfileField } from "../runtime/types";
import { forgeStore } from "../store/forgeStore";
import { runtimeStore, setRuntimeStore } from "../store/runtimeStore";
import { parseEnv } from "./profileEnv";
import {
  applyProfileFields,
  newAgentProfile,
  profileFieldKey,
  splitProfileFields,
  type ProfileFieldValues,
} from "./profileFields";
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
      description="A profile is a provider plus the way you run it — a different binary, extra arguments, a different environment. They appear in the + menu beside the providers themselves."
      class="settings-profiles"
    >
      <For each={forgeStore.agent_profiles} fallback={<p class="empty-copy">No profiles yet.</p>}>
        {(profile) => (
          <div class="settings-row profile-row">
            <span class="settings-row-label">{profile.name}</span>
            <span class="settings-row-note">{profile.provider_id}</span>
            <Show when={profile.executable}>
              {(executable) => <span class="settings-row-note">{executable()}</span>}
            </Show>
            <span class="history-spacer" />
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
  const fields = createMemo<ProfileField[]>(
    () =>
      forgeStore.providers.find((provider) => provider.descriptor?.id === props.profile.provider_id)
        ?.descriptor?.profile_fields ?? [],
  );
  const initial = splitProfileFields(props.profile, fields());
  const [name, setName] = createSignal(props.profile.name);
  const [executable, setExecutable] = createSignal(props.profile.executable ?? "");
  const [args, setArgs] = createSignal(initial.args.join("\n"));
  const [env, setEnv] = createSignal(
    initial.env.map(([key, value]) => `${key}=${value}`).join("\n"),
  );
  const [fieldValues, setFieldValues] = createSignal<ProfileFieldValues>(initial.values);
  const [saving, setSaving] = createSignal(false);
  const [saveError, setSaveError] = createSignal<string | null>(null);

  const parsedEnv = createMemo(() => parseEnv(env()));
  const validationError = createMemo(() => {
    if (name().trim() === "") return "A profile needs a name.";
    return parsedEnv().error;
  });
  const problem = () => validationError() ?? saveError();

  function submit(): void {
    if (validationError()) return;
    const profile = applyProfileFields(
      {
        ...props.profile,
        name: name().trim(),
        executable: executable().trim() || null,
        args: args()
          .split("\n")
          .map((line) => line.trim())
          .filter((line) => line !== ""),
        env: parsedEnv().pairs,
      },
      fields(),
      fieldValues(),
    );
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
        value={executable()}
        onChange={setExecutable}
        placeholder="Inherit the detected binary"
      />
      <For each={fields()}>
        {(field) => (
          <TextField
            class="settings-field"
            label={field.label}
            description={field.help}
            value={fieldValues()[profileFieldKey(field)] ?? ""}
            onChange={(value) =>
              setFieldValues((current) => ({
                ...current,
                [profileFieldKey(field)]: value,
              }))
            }
          />
        )}
      </For>
      <TextArea
        class="settings-field"
        label="Additional arguments, one per line"
        description="Each line is passed as one argument. Quotes and spaces are literal; no shell is used."
        rows={3}
        value={args()}
        onChange={setArgs}
        placeholder={"--model\nopus"}
      />
      <TextArea
        class="settings-field"
        label="Additional environment, one NAME=value per line"
        description="Stored in clear text. Prefer a config directory over an API key."
        rows={3}
        value={env()}
        onChange={setEnv}
        placeholder="HTTP_PROXY=http://localhost:8080"
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

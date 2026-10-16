import { For, Show, createEffect, createMemo, createSignal } from "solid-js";
import { removeAgentProfile, saveAgentProfile } from "./commands";
import type { AgentProfile, ConfigDirSpec } from "../../contracts/runtime";
import { forgeStore } from "../../state/forgeStore";
import { profilesStore, setProfilesStore } from "./profiles";
import { formatArgs, newAgentProfile, normalizeArgs, parseArgs } from "./agentProfile";
import { Button, Menu, TextArea, TextField, buttonClass } from "../../ui/index";
import { Icon } from "../../theme/icons/index";
import { Group } from "./SettingsLayout";
import { AgentVisibilitySwitch } from "./AgentVisibilitySwitch";

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
      description="A custom executable, account or arguments. Profiles appear in the + menu."
      class="settings-profiles"
      aside={
        <Menu
          triggerLabel="New profile"
          triggerClass={buttonClass("sm", "secondary")}
          trigger={
            <>
              New profile
              <Icon name="chevron-down" size={13} class="forge-icon-muted" />
            </>
          }
          items={providers().map((descriptor) => ({
            kind: "item" as const,
            label: descriptor.display_name ?? descriptor.id,
            run: () => setEditing({ profile: newAgentProfile(descriptor.id), isNew: true }),
          }))}
        />
      }
    >
      <For each={forgeStore.agent_profiles} fallback={<p class="empty-copy">No profiles yet.</p>}>
        {(profile) => (
          <div class="profile-row">
            <span class="profile-row-name">{profile.name}</span>
            <span class="profile-row-meta forge-truncate">
              {[profile.provider_id, profile.executable, profile.config_dir]
                .filter(Boolean)
                .join(" · ")}
            </span>
            <AgentVisibilitySwitch agentKey={`profile:${profile.id}`} label={profile.name} />
            <Menu
              triggerLabel={`${profile.name}: edit or delete`}
              triggerClass={buttonClass("sm", "ghost", true)}
              trigger={<Icon name="more-horizontal" size={14} class="forge-icon-muted" />}
              items={[
                {
                  kind: "item",
                  label: "Edit",
                  icon: "edit",
                  run: () => setEditing({ profile: { ...profile }, isNew: false }),
                },
                {
                  kind: "item",
                  label: "Delete",
                  icon: "trash",
                  destructive: true,
                  run: () => void removeAgentProfile(profile.id).catch(() => undefined),
                },
              ]}
            />
          </div>
        )}
      </For>

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
    setProfilesStore("profileSave", null);
    void saveAgentProfile(profile).catch((reason: unknown) => {
      setSaving(false);
      setSaveError(String(reason));
    });
  }

  createEffect(() => {
    const result = profilesStore.profileSave;
    if (!result || result.profile !== props.profile.id) return;
    setProfilesStore("profileSave", null);
    setSaving(false);
    if (result.error) setSaveError(result.error);
    else props.onSaved();
  });

  return (
    <form
      class="forge-card settings-form"
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

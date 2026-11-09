//! Settings › Agents, as the original client's `Agents` and `AgentProfiles`: what ⌘⇧A
//! starts, each provider's command and visibility, and launch profiles.

use std::path::PathBuf;

use domain::{AgentProfile, AgentProfileId, AgentProviderId, DetectionStatus, Timestamp};
use protocol::ProviderInfo;
use rune::{El, TextEdit, col, el, row, text};
use rune_kit::{Button, ButtonVariant, ControlSize, Kit, RadioItem, SegmentedControl, Switch};

use super::{group, notice, setting_row};
use crate::app::AppState;
use crate::launch::{self, DEFAULT_AGENT_KEY, DefaultAgent};

/// Command-line text into the arguments a launch passes one by one, as
/// the original client's `parseArgs`: whitespace separates, quotes group, a backslash
/// escapes outside single quotes. No shell runs them.
pub fn parse_args(source: &str) -> Result<Vec<String>, String> {
    let mut args = Vec::new();
    let mut current = String::new();
    let mut started = false;
    let mut quote: Option<char> = None;
    let mut escaped = false;
    for ch in source.chars() {
        if escaped {
            current.push(ch);
            started = true;
            escaped = false;
            continue;
        }
        if ch == '\\' && quote != Some('\'') {
            escaped = true;
            continue;
        }
        if let Some(open) = quote {
            if ch == open {
                quote = None;
            } else {
                current.push(ch);
            }
            continue;
        }
        if ch == '"' || ch == '\'' {
            quote = Some(ch);
            started = true;
            continue;
        }
        if ch.is_whitespace() {
            if started {
                args.push(std::mem::take(&mut current));
            }
            started = false;
            continue;
        }
        current.push(ch);
        started = true;
    }
    if escaped {
        return Err("A trailing backslash escapes nothing.".into());
    }
    if let Some(open) = quote {
        return Err(format!("An opening {open} is never closed."));
    }
    if started {
        args.push(current);
    }
    Ok(args)
}

fn quote_arg(arg: &str) -> String {
    if arg.is_empty() {
        return "\"\"".into();
    }
    if !arg
        .chars()
        .any(|ch| ch.is_whitespace() || matches!(ch, '"' | '\'' | '\\'))
    {
        return arg.into();
    }
    let mut quoted = String::from("\"");
    for ch in arg.chars() {
        if matches!(ch, '\\' | '"') {
            quoted.push('\\');
        }
        quoted.push(ch);
    }
    quoted.push('"');
    quoted
}

/// The inverse of `parse_args`, one line per flag, as the original client's `formatArgs`.
#[must_use]
pub fn format_args(args: &[String]) -> String {
    let mut lines: Vec<String> = Vec::new();
    let mut quoted_before = false;
    for arg in args {
        let token = quote_arg(arg);
        let quoted = token != *arg;
        let own_line = quoted || quoted_before || arg.starts_with('-');
        match lines.last_mut() {
            Some(line) if !own_line => {
                line.push(' ');
                line.push_str(&token);
            }
            _ => lines.push(token),
        }
        quoted_before = quoted;
    }
    lines.join("\n")
}

/// Splits a flag an older editor saved with its value inside, as the original client's
/// `normalizeArgs`; `--prompt=say hello` stays one argument.
#[must_use]
pub fn normalize_args(args: &[String]) -> Vec<String> {
    args.iter()
        .flat_map(|arg| {
            if arg.starts_with('-') && !arg.contains('=') && arg.contains(char::is_whitespace) {
                arg.split_whitespace().map(str::to_string).collect()
            } else {
                vec![arg.clone()]
            }
        })
        .collect()
}

pub struct ProfileForm {
    id: AgentProfileId,
    created_at: Timestamp,
    pub provider: AgentProviderId,
    pub is_new: bool,
    pub name: TextEdit,
    pub executable: TextEdit,
    pub config_dir: TextEdit,
    pub args: TextEdit,
    pub saving: bool,
    pub error: Option<String>,
}

impl ProfileForm {
    #[must_use]
    pub fn new(provider: AgentProviderId) -> Self {
        let mut name = TextEdit::new("");
        name.focused = true;
        Self {
            id: AgentProfileId::new(),
            created_at: Timestamp::now(),
            provider,
            is_new: true,
            name,
            executable: TextEdit::new(""),
            config_dir: TextEdit::new(""),
            args: TextEdit::new(""),
            saving: false,
            error: None,
        }
    }

    #[must_use]
    pub fn edit(profile: &AgentProfile) -> Self {
        let path = |value: &Option<PathBuf>| {
            TextEdit::new(
                value
                    .as_ref()
                    .map(|path| path.display().to_string())
                    .unwrap_or_default(),
            )
        };
        Self {
            id: profile.id,
            created_at: profile.created_at,
            provider: profile.provider_id.clone(),
            is_new: false,
            name: TextEdit::new(profile.name.clone()),
            executable: path(&profile.executable),
            config_dir: path(&profile.config_dir),
            args: TextEdit::new(format_args(&normalize_args(&profile.args))),
            saving: false,
            error: None,
        }
    }

    #[must_use]
    pub fn focused(&self) -> bool {
        [&self.name, &self.executable, &self.config_dir, &self.args]
            .iter()
            .any(|edit| edit.focused)
    }

    /// The profile the form describes. A provider without a config-dir
    /// variable is refused one by the daemon, so none is sent for it.
    pub fn build(&self, takes_config_dir: bool) -> Result<AgentProfile, String> {
        let name = self.name.text.trim();
        if name.is_empty() {
            return Err("A profile needs a name.".into());
        }
        let args = parse_args(&self.args.text)?;
        let path = |edit: &TextEdit| {
            let text = edit.text.trim();
            (!text.is_empty()).then(|| PathBuf::from(text))
        };
        Ok(AgentProfile {
            id: self.id,
            provider_id: self.provider.clone(),
            name: name.to_string(),
            executable: path(&self.executable),
            config_dir: if takes_config_dir {
                path(&self.config_dir)
            } else {
                None
            },
            args,
            created_at: self.created_at,
        })
    }
}

fn installed(info: &ProviderInfo) -> bool {
    matches!(info.detection.status, DetectionStatus::Installed { .. })
}

fn detection_label(info: &ProviderInfo) -> String {
    match &info.detection.status {
        DetectionStatus::Installed {
            version: Some(version),
            ..
        } => format!("v{version}"),
        DetectionStatus::Installed { .. } => "installed".into(),
        DetectionStatus::NotFound => "not found".into(),
        DetectionStatus::ProbeTimeout => "probe timed out".into(),
        DetectionStatus::Rejected { reason, .. } => format!("rejected: {reason}"),
        _ => "not probed".into(),
    }
}

/// The path detection resolved, else the name it looks for.
fn executable_hint(info: &ProviderInfo) -> (String, bool) {
    match &info.detection.status {
        DetectionStatus::Installed { executable, .. } => (executable.display().to_string(), true),
        _ => (
            info.descriptor
                .binary_candidates
                .first()
                .cloned()
                .unwrap_or_else(|| "Path to the binary".into()),
            false,
        ),
    }
}

impl AppState {
    /// Whether a provider or `profile:<id>` key is offered by the pickers.
    #[must_use]
    pub fn agent_visible(&self, key: &str) -> bool {
        self.pref(&launch::visibility_key(key)) != Some("false")
    }

    /// The default agent as it stands; one pointing at a hidden agent reads
    /// as Ask, as the original client's `defaultAgentFrom`.
    #[must_use]
    pub fn default_agent(&self) -> DefaultAgent {
        let agent = launch::parse(self.pref(DEFAULT_AGENT_KEY));
        match launch::launch_key(&agent) {
            Some(key) if !self.agent_visible(&key) => DefaultAgent::Ask,
            _ => agent,
        }
    }

    pub fn choose_default_agent(&mut self, agent: &DefaultAgent) {
        self.write_pref(DEFAULT_AGENT_KEY, &launch::value(agent));
    }

    pub fn set_agent_visible(&mut self, key: &str, visible: bool) {
        self.write_pref(
            &launch::visibility_key(key),
            if visible { "true" } else { "false" },
        );
    }

    /// Pins the typed command for `provider`, or clears the override when
    /// the field is empty.
    pub fn pin_provider_command(&mut self, provider: &AgentProviderId) {
        let path = self
            .settings
            .commands
            .get(provider.as_str())
            .map(|edit| edit.text.trim().to_string())
            .filter(|text| !text.is_empty())
            .map(PathBuf::from);
        if let Some(connection) = &self.connection {
            let _ = connection.set_provider_executable(provider.clone(), path);
        }
    }

    pub fn rescan_agents(&mut self) {
        if let Some(connection) = &self.connection {
            let _ = connection.refresh_agent_detection();
        }
    }

    pub fn save_profile(&mut self) {
        let takes_config_dir = self.settings.profile.as_ref().is_some_and(|form| {
            self.store.as_ref().is_some_and(|store| {
                store.providers.iter().any(|info| {
                    info.descriptor.id == form.provider && info.descriptor.config_dir.is_some()
                })
            })
        });
        let Some(form) = &mut self.settings.profile else {
            return;
        };
        match form.build(takes_config_dir) {
            Ok(profile) => {
                form.error = None;
                if let Some(connection) = &self.connection
                    && connection.save_agent_profile(profile).is_ok()
                {
                    form.saving = true;
                }
            }
            Err(error) => form.error = Some(error),
        }
    }

    pub fn remove_profile(&mut self, profile: AgentProfileId) {
        if let Some(connection) = &self.connection {
            let _ = connection.remove_agent_profile(profile);
        }
    }
}

fn small(label: &str, variant: ButtonVariant) -> Button {
    Button::new(label.to_string())
        .variant(variant)
        .control_size(ControlSize::Small)
}

fn default_choices(s: &AppState, k: &Kit) -> El {
    let Some(store) = &s.store else {
        return el();
    };
    let current = s.default_agent();
    let mut choices: Vec<(DefaultAgent, String, Option<String>, bool)> = vec![
        (
            DefaultAgent::Ask,
            "Ask each time".into(),
            Some("Open the launch palette and pick.".into()),
            true,
        ),
        (DefaultAgent::Shell, "Blank terminal".into(), None, true),
    ];
    for info in &store.providers {
        let id = info.descriptor.id.to_string();
        choices.push((
            DefaultAgent::Provider(id.clone()),
            info.descriptor.display_name.clone(),
            Some(detection_label(info)),
            installed(info) && s.agent_visible(&id),
        ));
    }
    for profile in &store.agent_profiles {
        let id = profile.id.to_string();
        let provider = store
            .providers
            .iter()
            .find(|info| info.descriptor.id == profile.provider_id);
        choices.push((
            DefaultAgent::Profile(id.clone()),
            profile.name.clone(),
            Some(format!("profile · {}", profile.provider_id)),
            provider.is_some_and(installed) && s.agent_visible(&format!("profile:{id}")),
        ));
    }
    let mut list = row()
        .key("default-agent")
        .role("radiogroup")
        .label("Default agent")
        .w_full()
        .gap(8.0)
        .flex_wrap();
    for (index, (agent, label, note, enabled)) in choices.into_iter().enumerate() {
        let checked = agent == current;
        let mut item = RadioItem::new(label, checked).disabled(!enabled);
        if let Some(note) = note {
            item = item.description(note);
        }
        list = list.child(
            item.on_select(move |s: &mut AppState| s.choose_default_agent(&agent))
                .build(k)
                .key(format!("default-agent-{index}"))
                .w(220.0),
        );
    }
    group(
        k,
        "agents-default",
        "Default agent",
        Some("Starts with ⌘⇧A. Ask opens the launch palette each time."),
    )
    .child(list)
}

fn provider_card(s: &AppState, info: &ProviderInfo, k: &Kit) -> El {
    let id = info.descriptor.id.clone();
    let key = id.to_string();
    let name = info.descriptor.display_name.clone();
    let visible = s.agent_visible(&key);
    let is_default = s.default_agent() == DefaultAgent::Provider(key.clone());
    let mut title =
        row()
            .gap(8.0)
            .items_center()
            .child(
                text(name.clone())
                    .font(k.ty.small_medium)
                    .color(if visible {
                        k.c.text_base
                    } else {
                        k.c.text_faint
                    }),
            );
    if is_default {
        title = title.child(
            row()
                .px(6.0)
                .py(1.0)
                .radius(8.0)
                .fill(k.c.surface_hover)
                .child(
                    text("Default")
                        .font(k.ty.text_xsmall)
                        .color(k.c.text_accent),
                ),
        );
    }
    let mut head = row().w_full().gap(8.0).items_center().child(
        col().grow(1.0).min_w(0.0).gap(2.0).child(title).child(
            text(format!("{key} · {}", detection_label(info)))
                .font(k.ty.text_xsmall)
                .color(k.c.text_faint)
                .truncate(),
        ),
    );
    if !is_default && visible && installed(info) {
        let make = DefaultAgent::Provider(key.clone());
        head = head.child(
            small("Make default", ButtonVariant::GhostMuted)
                .on_press(move |s: &mut AppState| s.choose_default_agent(&make))
                .build(k)
                .key(format!("provider-default-{key}")),
        );
    }
    let toggle_key = key.clone();
    head = head.child(
        Switch::new(visible)
            .label(format!("{name} enabled"))
            .hide_label()
            .on_toggle(move |s: &mut AppState| {
                let next = !s.agent_visible(&toggle_key);
                s.set_agent_visible(&toggle_key, next);
            })
            .build(k)
            .key(format!("provider-visible-{key}")),
    );

    let (hint, resolved) = executable_hint(info);
    let typed = s
        .settings
        .commands
        .get(key.as_str())
        .cloned()
        .unwrap_or_else(|| TextEdit::new(""));
    let empty = typed.text.trim().is_empty();
    let edit_key = key.clone();
    let field = rune_kit::TextInput::new(&typed)
        .label(format!("Executable for {name}"))
        .placeholder(hint)
        .spellcheck(false)
        .on_edit(move |s: &mut AppState| {
            s.settings
                .commands
                .entry(edit_key.clone())
                .or_insert_with(|| TextEdit::new(""))
        })
        .build(k)
        .key(format!("provider-command-{key}"))
        .grow(1.0)
        .min_w(0.0);
    let pin_id = id.clone();
    let pin = small(
        if empty { "Clear override" } else { "Pin" },
        ButtonVariant::Outline,
    )
    .on_press(move |s: &mut AppState| s.pin_provider_command(&pin_id))
    .build(k)
    .key(format!("provider-pin-{key}"));
    let mut card = col()
        .key(format!("provider-{key}"))
        .w_full()
        .gap(8.0)
        .padding(10.0)
        .radius(8.0)
        .border(1.0, k.c.border_weak)
        .child(head)
        .child(setting_row(
            k,
            "Command",
            Some(if resolved {
                "Detection resolved this path on PATH. Pin another to override it."
            } else {
                "Nothing was resolved; this is the name detection looks for."
            }),
            el(),
        ))
        .child(
            row()
                .w_full()
                .gap(6.0)
                .items_center()
                .child(field)
                .child(pin),
        );
    if !info.descriptor.default_args.is_empty() {
        card = card.child(setting_row(
            k,
            "Default arguments",
            Some("What the daemon passes at launch. Change them per profile, below."),
            text(info.descriptor.default_args.join(" "))
                .font(k.ty.mono)
                .color(k.c.text_muted),
        ));
    }
    card
}

fn providers(s: &AppState, k: &Kit) -> El {
    let list = s
        .store
        .as_ref()
        .map_or(&[][..], |store| store.providers.as_slice());
    let found = list.iter().filter(|info| installed(info)).count();
    let rescan = small("Rescan", ButtonVariant::GhostMuted)
        .icon(&crate::glyphs::REFRESH)
        .on_press(AppState::rescan_agents)
        .build(k)
        .key("agents-rescan");
    let description = format!("{found} detected. Off hides a provider from every agent picker.");
    let mut view = group(k, "agents-providers", "Providers", Some(&description))
        .child(row().w_full().child(el().grow(1.0)).child(rescan));
    if list.is_empty() {
        view = view.child(
            text("No providers in this snapshot.")
                .font(k.ty.small)
                .color(k.c.text_faint),
        );
    }
    for info in list {
        view = view.child(provider_card(s, info, k));
    }
    view
}

fn profile_rows(s: &AppState, k: &Kit) -> El {
    let Some(store) = &s.store else {
        return el();
    };
    let new = small("New profile ▾", ButtonVariant::Outline)
        .disabled(store.providers.is_empty() || s.settings.profile.is_some())
        .on_press(|s: &mut AppState| {
            s.menus.button = Some(crate::menus::ButtonMenu::NewProfile);
        })
        .build(k)
        .key(crate::menus::ButtonMenu::NewProfile.trigger());
    let mut view = group(
        k,
        "agents-profiles",
        "Launch profiles",
        Some("A custom executable, account or arguments. Profiles appear in the + menu."),
    )
    .child(row().w_full().child(el().grow(1.0)).child(new));
    if store.agent_profiles.is_empty() {
        view = view.child(
            text("No profiles yet.")
                .font(k.ty.small)
                .color(k.c.text_faint),
        );
    }
    for profile in &store.agent_profiles {
        let id = profile.id;
        let visibility = format!("profile:{id}");
        let visible = s.agent_visible(&visibility);
        let meta: Vec<String> = [
            Some(profile.provider_id.to_string()),
            profile
                .executable
                .as_ref()
                .map(|path| path.display().to_string()),
            profile
                .config_dir
                .as_ref()
                .map(|path| path.display().to_string()),
        ]
        .into_iter()
        .flatten()
        .collect();
        let editing = profile.clone();
        view = view.child(
            row()
                .key(format!("profile-{id}"))
                .w_full()
                .h(34.0)
                .gap(8.0)
                .items_center()
                .child(
                    text(profile.name.clone())
                        .font(k.ty.small_medium)
                        .color(k.c.text_base),
                )
                .child(
                    text(meta.join(" · "))
                        .font(k.ty.text_xsmall)
                        .color(k.c.text_faint)
                        .grow(1.0)
                        .min_w(0.0)
                        .truncate(),
                )
                .child(
                    Switch::new(visible)
                        .label(format!("{} enabled", profile.name))
                        .hide_label()
                        .on_toggle(move |s: &mut AppState| {
                            let next = !s.agent_visible(&visibility);
                            s.set_agent_visible(&visibility, next);
                        })
                        .build(k)
                        .key(format!("profile-visible-{id}")),
                )
                .child(
                    small("Edit", ButtonVariant::GhostMuted)
                        .on_press(move |s: &mut AppState| {
                            s.settings.profile = Some(ProfileForm::edit(&editing));
                        })
                        .build(k)
                        .key(format!("profile-edit-{id}")),
                )
                .child(
                    small("Delete", ButtonVariant::GhostMuted)
                        .on_press(move |s: &mut AppState| s.remove_profile(id))
                        .build(k)
                        .key(format!("profile-delete-{id}")),
                ),
        );
    }
    if let Some(form) = &s.settings.profile {
        view = view.child(profile_form(s, form, k));
    }
    view
}

macro_rules! form_field {
    ($field:ident) => {
        |s: &mut AppState| match s.settings.profile.as_mut() {
            Some(form) => &mut form.$field,
            None => &mut s.scratch_edit,
        }
    };
}

fn profile_form(s: &AppState, form: &ProfileForm, k: &Kit) -> El {
    let providers = s
        .store
        .as_ref()
        .map_or(&[][..], |store| store.providers.as_slice());
    let spec = providers
        .iter()
        .find(|info| info.descriptor.id == form.provider)
        .and_then(|info| info.descriptor.config_dir.clone());
    let title = if form.is_new {
        "New profile".to_string()
    } else {
        format!("Edit {}", form.name.text.trim())
    };
    let label = |words: &str| {
        text(words.to_string())
            .font(k.ty.small)
            .color(k.c.text_muted)
    };
    let note = |words: String| {
        text(words)
            .font(k.ty.text_xsmall)
            .color(k.c.text_faint)
            .wrap()
    };
    let mut view = col()
        .key("profile-form")
        .role("form")
        .label(title.clone())
        .w_full()
        .gap(8.0)
        .padding(12.0)
        .radius(8.0)
        .border(1.0, k.c.border_weak)
        .child(text(title).font(k.ty.small_medium).color(k.c.text_base));
    if form.is_new && !providers.is_empty() {
        let names: Vec<String> = providers
            .iter()
            .map(|info| info.descriptor.display_name.clone())
            .collect();
        let selected = providers
            .iter()
            .position(|info| info.descriptor.id == form.provider);
        let ids: Vec<AgentProviderId> = providers
            .iter()
            .map(|info| info.descriptor.id.clone())
            .collect();
        view = view.child(label("Provider")).child(
            SegmentedControl::new(&names, selected)
                .label("Provider")
                .fit()
                .on_pick(move |s: &mut AppState, index: usize, _tx| {
                    if let (Some(form), Some(id)) = (s.settings.profile.as_mut(), ids.get(index)) {
                        form.provider = id.clone();
                    }
                })
                .build(k)
                .key("profile-provider"),
        );
    }
    view = view
        .child(label("Name"))
        .child(
            rune_kit::TextInput::new(&form.name)
                .label("Name")
                .placeholder("Personal")
                .on_edit(form_field!(name))
                .build(k)
                .key("profile-name"),
        )
        .child(label("Executable"))
        .child(
            rune_kit::TextInput::new(&form.executable)
                .label("Executable")
                .placeholder("Inherit the detected binary")
                .spellcheck(false)
                .on_edit(form_field!(executable))
                .build(k)
                .key("profile-executable"),
        )
        .child(note(
            "A path, or the name of a wrapper on your PATH. Empty inherits the detected binary."
                .into(),
        ));
    if let Some(spec) = spec {
        view = view
            .child(label("Config directory"))
            .child(
                rune_kit::TextInput::new(&form.config_dir)
                    .label("Config directory")
                    .placeholder(".claude-personal")
                    .spellcheck(false)
                    .on_edit(form_field!(config_dir))
                    .build(k)
                    .key("profile-config-dir"),
            )
            .child(note(format!(
                "{} Relative to your home directory unless it starts with /.",
                spec.help
            )));
    }
    view = view
        .child(label("Additional arguments"))
        .child(
            rune_kit::TextArea::new(&form.args)
                .label("Additional arguments")
                .placeholder("--model opus")
                .spellcheck(false)
                .on_edit(form_field!(args))
                .build(k)
                .key("profile-args")
                .h(72.0),
        )
        .child(note(
            "Written like a command line: spaces separate arguments, and a new line is just \
             another space. Quote one that contains a space; no shell runs them."
                .into(),
        ));
    let problem = form
        .error
        .clone()
        .or_else(|| parse_args(&form.args.text).err());
    if let Some(problem) = problem {
        view = view.child(
            text(problem)
                .key("profile-error")
                .role("alert")
                .font(k.ty.small)
                .color(k.c.warning.fg),
        );
    }
    let invalid = form.name.text.trim().is_empty() || parse_args(&form.args.text).is_err();
    view.child(
        row()
            .gap(6.0)
            .child(
                Button::new(if form.saving { "Saving…" } else { "Save" })
                    .variant(ButtonVariant::Submit)
                    .control_size(ControlSize::Small)
                    .disabled(invalid || form.saving)
                    .on_press(AppState::save_profile)
                    .build(k)
                    .key("profile-save"),
            )
            .child(
                small("Cancel", ButtonVariant::Ghost)
                    .disabled(form.saving)
                    .on_press(|s: &mut AppState| s.settings.profile = None)
                    .build(k)
                    .key("profile-cancel"),
            ),
    )
}

pub fn section(s: &AppState, k: &Kit) -> El {
    col()
        .key("agents")
        .w_full()
        .gap(24.0)
        .child(default_choices(s, k))
        .children(notice(s, k))
        .child(providers(s, k))
        .child(profile_rows(s, k))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn strings(items: &[&str]) -> Vec<String> {
        items.iter().map(|item| (*item).to_string()).collect()
    }

    #[test]
    fn arguments_parse_like_a_command_line_without_a_shell() {
        assert_eq!(
            parse_args("--model opus\n--append-system-prompt \"be terse\""),
            Ok(strings(&[
                "--model",
                "opus",
                "--append-system-prompt",
                "be terse"
            ]))
        );
        assert_eq!(parse_args("'a\\b' $HOME"), Ok(strings(&["a\\b", "$HOME"])));
        assert_eq!(parse_args("\"\""), Ok(strings(&[""])));
        assert_eq!(
            parse_args("a\\"),
            Err("A trailing backslash escapes nothing.".into())
        );
        assert_eq!(
            parse_args("\"open"),
            Err("An opening \" is never closed.".into())
        );
    }

    #[test]
    fn saved_arguments_format_one_flag_per_line_and_round_trip() {
        let args = strings(&[
            "--model",
            "opus",
            "--append-system-prompt",
            "be terse",
            "-v",
        ]);
        let text = format_args(&args);
        assert_eq!(
            text,
            "--model opus\n--append-system-prompt\n\"be terse\"\n-v"
        );
        assert_eq!(parse_args(&text), Ok(args));
        assert_eq!(
            normalize_args(&strings(&["--model claude-opus", "--prompt=say hi"])),
            strings(&["--model", "claude-opus", "--prompt=say hi"])
        );
    }

    #[test]
    fn a_form_needs_a_name_and_drops_a_config_dir_its_provider_refuses() {
        let mut form = ProfileForm::new(AgentProviderId::new("cursor"));
        assert_eq!(form.build(false), Err("A profile needs a name.".into()));
        form.name = TextEdit::new(" Work ");
        form.config_dir = TextEdit::new(".cursor-work");
        form.args = TextEdit::new("--fast");
        let profile = form.build(false).expect("valid");
        assert_eq!(profile.name, "Work");
        assert_eq!(profile.config_dir, None);
        assert_eq!(profile.executable, None);
        assert_eq!(profile.args, strings(&["--fast"]));
        let kept = form.build(true).expect("valid");
        assert_eq!(kept.config_dir, Some(PathBuf::from(".cursor-work")));
        let again = ProfileForm::edit(&kept);
        assert_eq!(
            again.build(true).expect("valid"),
            kept,
            "an edit keeps the id"
        );
    }
}

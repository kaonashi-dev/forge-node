//! Settings as sections: while it is open the sidebar lists them, and the
//! centre shows one under a page header.

mod agents;
mod general;
mod keyboard;
mod look;
pub mod prefs;

use std::collections::HashMap;
use std::time::{Duration, Instant};

use rune::{El, Icon, ScrollSpec, TextEdit, col, row, text};
use rune_kit::Kit;
use rune_kit::shell::SidebarRow;

pub use agents::ProfileForm;

use crate::app::AppState;
use crate::glyphs;

/// The pages, in the order the sidebar lists them.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Section {
    #[default]
    General,
    Agents,
    Keyboard,
    SharedFiles,
    Storage,
    Stats,
    Appearance,
}

impl Section {
    pub const ALL: [Section; 7] = [
        Section::General,
        Section::Agents,
        Section::Keyboard,
        Section::SharedFiles,
        Section::Storage,
        Section::Stats,
        Section::Appearance,
    ];

    #[must_use]
    pub fn label(self) -> &'static str {
        match self {
            Section::General => "General",
            Section::Agents => "Agents",
            Section::Keyboard => "Keyboard",
            Section::SharedFiles => "Shared files",
            Section::Storage => "Storage",
            Section::Stats => "Stats & usage",
            Section::Appearance => "Appearance",
        }
    }

    #[must_use]
    pub fn summary(self) -> &'static str {
        match self {
            Section::General => "Application details, configuration files, and daemon controls.",
            Section::Agents => "Choose the default agent and how each one launches.",
            Section::Keyboard => {
                "Every action and the key it answers to. Escape twice leaves Settings — a \
                 sequence, not a chord."
            }
            Section::SharedFiles => {
                "What a new worktree needs on disk that git does not carry: .env, installed \
                 dependencies, a local certificate."
            }
            Section::Storage => "Where Forge Node keeps its config, state and managed worktrees.",
            Section::Stats => {
                "Token analytics counted off agent transcripts on disk — separate from \
                 subscription meters, which are the provider's word on allowance."
            }
            Section::Appearance => {
                "Theme, accent, density, layout and type. Changes apply as you pick them."
            }
        }
    }

    #[must_use]
    pub fn key(self) -> &'static str {
        match self {
            Section::General => "general",
            Section::Agents => "agents",
            Section::Keyboard => "keyboard",
            Section::SharedFiles => "shared-files",
            Section::Storage => "storage",
            Section::Stats => "stats",
            Section::Appearance => "appearance",
        }
    }

    fn icon(self) -> &'static Icon {
        match self {
            Section::General => &glyphs::SETTINGS,
            Section::Agents => &glyphs::AGENT,
            Section::Keyboard => &glyphs::KEYBOARD,
            Section::SharedFiles => &glyphs::SHARED_FILES,
            Section::Storage => &glyphs::FILES,
            Section::Stats => &glyphs::STATS,
            Section::Appearance => &glyphs::APPEARANCE,
        }
    }
}

/// How long the second Escape may follow the first, as the original client's `ESC_AGAIN_MS`.
pub const ESC_AGAIN: Duration = Duration::from_millis(1_200);

/// Two Escapes leave Settings. A sequence rather than a chord, so a dialog
/// or a field that takes Escape first keeps it.
#[derive(Clone, Copy, Debug, Default)]
pub struct EscAgain {
    first: Option<Instant>,
}

impl EscAgain {
    /// Whether this Escape is the second of a pair inside the window; the
    /// first one arms it.
    pub fn press(&mut self, now: Instant) -> bool {
        match self.first {
            Some(first) if now.saturating_duration_since(first) <= ESC_AGAIN => {
                self.first = None;
                true
            }
            _ => {
                self.first = Some(now);
                false
            }
        }
    }

    #[must_use]
    pub fn armed(&self) -> bool {
        self.first.is_some()
    }

    /// Disarms a first Escape that has waited too long; true when that
    /// changed what the back row says.
    pub fn expire(&mut self, now: Instant) -> bool {
        match self.first {
            Some(first) if now.saturating_duration_since(first) > ESC_AGAIN => {
                self.first = None;
                true
            }
            _ => false,
        }
    }

    pub fn reset(&mut self) {
        self.first = None;
    }
}

#[derive(Default)]
pub struct Settings {
    pub section: Section,
    pub esc: EscAgain,
    /// Factory reset waits for this to be confirmed.
    pub confirm_reset: bool,
    pub profile: Option<ProfileForm>,
    /// What each provider's Command field holds, by provider id.
    pub commands: HashMap<String, TextEdit>,
    /// The daemon's answer to the last General or Agents action.
    pub notice: Option<(bool, String)>,
    pub keyboard: keyboard::Keyboard,
}

impl Settings {
    /// Whether a field on a settings page has the keyboard, so Escape is its.
    #[must_use]
    pub fn field_focused(&self) -> bool {
        self.commands.values().any(|edit| edit.focused)
            || self.profile.as_ref().is_some_and(ProfileForm::focused)
    }
}

impl AppState {
    /// Settings › Shared files is about the window's checkout's project, and
    /// scans it once when it opens on it.
    fn open_shared_files(&mut self) {
        if let Some(project) = crate::shares::current_project(self) {
            crate::shares::scan(self, project);
        }
    }
}

impl AppState {
    /// Writes a shared preference and keeps the replica in step: the daemon
    /// broadcasts no change to `app_state`, so only a snapshot would.
    pub fn write_pref(&mut self, key: &str, value: &str) {
        let _ = self.write_pref_checked(key, value);
    }

    /// Writes a preference and reports a refused socket write, for the few
    /// callers that have somewhere to say so.
    #[must_use = "a refused write leaves the preference ahead of the daemon"]
    pub fn write_pref_checked(&mut self, key: &str, value: &str) -> Result<(), String> {
        if let Some(connection) = &self.connection {
            connection.set_app_state(key, value)?;
        }
        if let Some(store) = &mut self.store {
            match store.app_state.iter_mut().find(|(k, _)| k == key) {
                Some((_, stored)) => *stored = value.to_string(),
                None => store.app_state.push((key.to_string(), value.to_string())),
            }
        }
        if key.starts_with("ui.editor.") {
            self.apply_editor_prefs();
        }
        Ok(())
    }

    #[must_use]
    pub fn pref(&self, key: &str) -> Option<&str> {
        self.store.as_ref()?.app_state_value(key)
    }

    /// The chrome's type size, as `ui.font_size` stores it.
    #[must_use]
    pub fn chrome_font_size(&self) -> f64 {
        prefs::read_scale(self.pref(prefs::UI_FONT_SIZE_KEY), prefs::UI_FONT_SIZE)
    }

    #[must_use]
    pub fn reduce_motion(&self) -> bool {
        prefs::read_flag(self.pref(prefs::REDUCE_MOTION_KEY), false)
    }

    #[must_use]
    pub fn density(&self) -> prefs::Density {
        prefs::Density::read(self.pref(prefs::DENSITY_KEY))
    }

    #[must_use]
    pub fn trimmed(&self) -> bool {
        prefs::read_flag(self.pref(prefs::TRIM_KEY), false)
    }

    #[must_use]
    pub fn glass(&self) -> bool {
        prefs::read_flag(self.pref(prefs::GLASS_KEY), false)
    }

    #[must_use]
    pub fn chrome_metrics(&self) -> prefs::ChromeMetrics {
        prefs::ChromeMetrics::of(self.density(), self.trimmed())
    }

    /// Opens Settings, on `section` when one is named.
    pub fn open_settings(&mut self, section: Option<Section>) {
        if let Some(section) = section {
            self.settings.section = section;
        }
        self.settings.esc.reset();
        self.show_mode(crate::workbench::CenterMode::Settings);
        if self.settings.section == Section::SharedFiles {
            self.open_shared_files();
        }
    }

    pub fn close_settings(&mut self) {
        self.settings.esc.reset();
        self.settings.confirm_reset = false;
        self.show_mode(crate::workbench::CenterMode::Session);
    }

    pub fn show_section(&mut self, section: Section) {
        self.settings.section = section;
        self.settings.notice = None;
        if section == Section::SharedFiles {
            self.open_shared_files();
        }
    }

    /// An Escape that reached the app while Settings is open: the second of
    /// a pair leaves it. A modified key, or one a dialog or field could still
    /// want, is not part of the pair.
    pub fn settings_escape(&mut self, ev: &rune::KeyEvent, now: Instant) -> bool {
        let mods = ev.mods;
        if self.workbench.mode != crate::workbench::CenterMode::Settings
            || ev.key != rune::Key::Escape
            || mods.cmd
            || mods.ctrl
            || mods.alt
            || mods.shift
            || self.palette.open
            || self.settings.confirm_reset
            || self.settings.field_focused()
            || self.projects.field_focused()
        {
            return false;
        }
        if self.settings.esc.press(now) {
            self.close_settings();
        }
        true
    }

    /// The command result a settings page reports back.
    pub fn settings_result(&mut self, label: &str, ok: bool, message: &str) {
        match label {
            "SaveAgentProfile" => {
                if let Some(form) = &mut self.settings.profile {
                    form.saving = false;
                    if ok {
                        self.settings.profile = None;
                    } else {
                        form.error = Some(message.to_string());
                    }
                }
            }
            "StopDaemon"
            | "FactoryReset"
            | "RefreshAgentDetection"
            | "SetProviderExecutable"
            | "RemoveAgentProfile" => {
                self.settings.notice = Some((ok, message.to_string()));
            }
            _ => {}
        }
    }
}

/// The sidebar while Settings is open, as the original client's `SettingsNav`.
pub fn nav(s: &AppState, k: &Kit) -> El {
    let armed = s.settings.esc.armed();
    let back = SidebarRow::new(if armed {
        "Press Esc again"
    } else {
        "Back to work"
    })
    .icon(&glyphs::ARROW_LEFT)
    .height(s.chrome_metrics().row)
    .trailing(
        text("esc esc")
            .font(k.ty.text_xsmall)
            .color(k.c.text_faint)
            .a11y_hidden(),
    )
    .on_select(|s: &mut AppState, _| s.close_settings())
    .build(k)
    .key("settings-back");
    let mut list = col()
        .key("settings-nav")
        .role("navigation")
        .label("Settings sections")
        .w_full()
        .gap(2.0)
        .child(back)
        .child(
            text("Settings")
                .font(k.ty.text_xsmall_medium)
                .color(k.c.text_faint)
                .pt(12.0)
                .pb(4.0)
                .px(8.0),
        );
    for section in Section::ALL {
        let current = s.settings.section == section;
        list = list.child(
            SidebarRow::new(section.label())
                .icon(section.icon())
                .height(s.chrome_metrics().row)
                .selected(current)
                .on_select(move |s: &mut AppState, _| s.show_section(section))
                .build(k)
                .key(format!("settings-nav-{}", section.key()))
                .checked(current),
        );
    }
    list
}

/// One labelled group of a page, as the original client's `Group`.
pub(crate) fn group(k: &Kit, key: &str, title: &str, description: Option<&str>) -> El {
    let mut head = col().gap(2.0).child(
        text(title.to_string())
            .role("heading")
            .font(k.ty.small_medium)
            .color(k.c.text_base),
    );
    if let Some(description) = description {
        head = head.child(
            text(description.to_string())
                .font(k.ty.text_xsmall)
                .color(k.c.text_faint)
                .wrap(),
        );
    }
    col()
        .key(key.to_string())
        .role("group")
        .label(title.to_string())
        .w_full()
        .gap(8.0)
        .child(head)
}

/// A label and its explanation on the left, the control on the right, as
/// the original client's `Row`.
pub(crate) fn setting_row(k: &Kit, label: &str, description: Option<&str>, control: El) -> El {
    let mut words = col().gap(2.0).grow(1.0).min_w(0.0).child(
        text(label.to_string())
            .font(k.ty.small)
            .color(k.c.text_base),
    );
    if let Some(description) = description {
        words = words.child(
            text(description.to_string())
                .font(k.ty.text_xsmall)
                .color(k.c.text_faint)
                .wrap(),
        );
    }
    row()
        .w_full()
        .gap(12.0)
        .items_center()
        .child(words)
        .child(control)
}

pub(crate) fn notice(s: &AppState, k: &Kit) -> Option<El> {
    let (ok, message) = s.settings.notice.as_ref()?;
    Some(
        text(message.clone())
            .key("settings-notice")
            .role("status")
            .font(k.ty.small)
            .color(if *ok { k.c.text_muted } else { k.c.warning.fg })
            .wrap(),
    )
}

/// The centre while Settings is open: the chosen section under its header.
pub fn page(s: &AppState, k: &Kit) -> El {
    let section = s.settings.section;
    let body = match section {
        Section::General => general::section(s, k),
        Section::Agents => agents::section(s, k),
        Section::Keyboard => keyboard::section(s, k),
        Section::SharedFiles => crate::shares::page(s, k, crate::shares::current_project(s), None),
        Section::Storage => crate::storage::section(s, k),
        Section::Stats => crate::usage::section(s, k),
        Section::Appearance => look::section(s, k),
    };
    let head = col()
        .key("settings-page-head")
        .gap(4.0)
        .child(
            text(section.label())
                .role("heading")
                .font(k.ty.body_medium)
                .color(k.c.text_strong),
        )
        .child(
            text(section.summary())
                .font(k.ty.small)
                .color(k.c.text_faint)
                .wrap(),
        );
    col().key("settings").w_full().h_full().min_h(0.0).child(
        col()
            .key(format!("settings-{}", section.key()))
            .w_full()
            .grow(1.0)
            .min_h(0.0)
            .padding(24.0)
            .gap(20.0)
            .scroll(ScrollSpec {
                stick_bottom: false,
            })
            .child(head)
            .child(body),
    )
}

/// Overlays a settings page asks for.
pub fn dialogs(s: &AppState, k: &Kit, layer: &mut Vec<El>) {
    general::reset_dialog(s, k, layer);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn two_escapes_inside_the_window_leave_and_a_late_one_rearms() {
        let mut esc = EscAgain::default();
        let start = Instant::now();
        assert!(!esc.press(start));
        assert!(esc.armed());
        assert!(esc.press(start + Duration::from_millis(1_200)));
        assert!(!esc.armed(), "the pair is spent");
        assert!(!esc.press(start + Duration::from_millis(2_000)));
        assert!(
            !esc.press(start + Duration::from_millis(3_300)),
            "1300 ms later is a new first press"
        );
        assert!(!esc.expire(start + Duration::from_millis(3_400)));
        assert!(esc.expire(start + Duration::from_millis(4_600)));
        assert!(!esc.armed());
    }

    fn state() -> (AppState, flume::Receiver<crate::connection::Command>) {
        use std::path::Path;
        use std::sync::Arc;
        let store = crate::connection::fixture_store(
            &Path::new(env!("CARGO_MANIFEST_DIR")).join("fixtures/snapshot.json"),
        )
        .expect("fixture snapshot decodes");
        let metrics = Arc::new(crate::connection::Metrics::default());
        let mut state = AppState::for_bench(
            store,
            Arc::clone(&metrics),
            crate::terminal::SharedTerminal::new(),
        );
        let (handle, rx) = crate::connection::ConnectionHandle::test_handle(metrics);
        state.connection = Some(handle);
        (state, rx)
    }

    fn escape(mods: rune::Mods) -> rune::KeyEvent {
        rune::KeyEvent {
            key: rune::Key::Escape,
            mods,
        }
    }

    #[test]
    fn a_pair_of_escapes_leaves_settings_and_a_field_or_modifier_keeps_it() {
        use crate::workbench::CenterMode;
        let (mut s, _rx) = state();
        s.open_settings(Some(Section::Agents));
        assert_eq!(s.workbench.mode, CenterMode::Settings);
        assert_eq!(s.settings.section, Section::Agents);
        let now = Instant::now();
        let shift = rune::Mods {
            shift: true,
            ..rune::Mods::default()
        };
        assert!(!s.settings_escape(&escape(shift), now), "a modified Escape");
        s.settings.profile = Some(ProfileForm::new(domain::AgentProviderId::new("claude")));
        assert!(
            !s.settings_escape(&escape(rune::Mods::default()), now),
            "the focused name field keeps Escape"
        );
        s.settings.profile = None;
        assert!(s.settings_escape(&escape(rune::Mods::default()), now));
        assert!(s.settings.esc.armed());
        assert_eq!(s.workbench.mode, CenterMode::Settings);
        assert!(s.settings_escape(
            &escape(rune::Mods::default()),
            now + Duration::from_millis(400)
        ));
        assert_eq!(s.workbench.mode, CenterMode::Session);
        assert!(
            !s.settings_escape(&escape(rune::Mods::default()), now),
            "outside Settings Escape is the session's"
        );
        s.open_settings(None);
        assert_eq!(
            s.settings.section,
            Section::Agents,
            "the last section stays"
        );
    }

    #[test]
    fn a_preference_is_saved_and_read_back_before_the_next_snapshot() {
        let (mut s, rx) = state();
        assert_eq!(s.chrome_font_size(), 13.0);
        s.write_pref(prefs::UI_FONT_SIZE_KEY, "14.5");
        assert_eq!(s.chrome_font_size(), 14.5);
        match rx.try_recv() {
            Ok(crate::connection::Command::SetAppState { key, value }) => {
                assert_eq!((key.as_str(), value.as_str()), ("ui.font_size", "14.5"));
            }
            other => panic!("expected SetAppState, got {other:?}"),
        }
        s.write_pref(prefs::UI_FONT_SIZE_KEY, "15");
        assert_eq!(s.chrome_font_size(), 15.0, "the replica row is replaced");
        s.choose_default_agent(&crate::launch::DefaultAgent::Provider("claude".into()));
        assert_eq!(
            s.default_agent(),
            crate::launch::DefaultAgent::Provider("claude".into())
        );
        s.set_agent_visible("claude", false);
        assert_eq!(
            s.default_agent(),
            crate::launch::DefaultAgent::Ask,
            "a hidden default reads as Ask"
        );
    }

    #[test]
    fn each_section_renders_alone_under_the_settings_nav() {
        use rune::{Env, Runtime};
        let (s, _rx) = state();
        let mut rt = Runtime::new(
            s,
            crate::ui::view,
            (1180.0, 760.0),
            Env {
                system_dark: true,
                scale: 1.0,
            },
        );
        rt.update(|s, _| s.open_settings(None));
        for (index, section) in Section::ALL.into_iter().enumerate() {
            rt.update(move |s, _| s.show_section(section));
            rt.frame(index as f64);
            rt.frame(index as f64 + 0.5);
            let keys: Vec<String> = rt.elements().into_iter().filter_map(|e| e.key).collect();
            let has = |key: &str| keys.iter().any(|k| k == key);
            assert!(has("settings-back"), "the nav replaces the views");
            assert!(!has("sidebar-views"));
            assert!(has(&format!("settings-{}", section.key())));
            for other in Section::ALL.into_iter().filter(|o| *o != section) {
                assert!(
                    !has(&format!("settings-{}", other.key())),
                    "one at a time: {section:?} shows {other:?}"
                );
            }
        }
        rt.update(|s, _| s.close_settings());
        rt.frame(10.0);
        let keys: Vec<String> = rt.elements().into_iter().filter_map(|e| e.key).collect();
        assert!(keys.iter().any(|k| k == "sidebar-views"));
    }

    #[test]
    fn the_sections_follow_the_original_client_order() {
        let labels: Vec<&str> = Section::ALL.iter().map(|s| s.label()).collect();
        assert_eq!(
            labels,
            [
                "General",
                "Agents",
                "Keyboard",
                "Shared files",
                "Storage",
                "Stats & usage",
                "Appearance"
            ]
        );
    }
}

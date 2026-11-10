//! Settings › Configuration: what the daemon loaded from `config.toml`, read
//! once per connection. The daemon reads the file only at startup, so the page
//! says so rather than offering to edit it in place.

use rune::{El, col, row, text};
use rune_kit::{Button, ButtonVariant, ControlSize, Kit};

use crate::app::AppState;
use crate::connection::ConfigView;

#[derive(Default)]
pub struct ConfigPage {
    /// The connection generation it was asked on; a reconnect may be another daemon.
    pub asked: Option<u64>,
    pub view: Option<ConfigView>,
    pub error: Option<String>,
}

pub fn section(s: &AppState, k: &Kit) -> El {
    let page = &s.settings.config;
    let Some(view) = &page.view else {
        let words = page
            .error
            .clone()
            .unwrap_or_else(|| "Reading the configuration…".into());
        return text(words)
            .key("config-status")
            .role("status")
            .font(k.ty.small)
            .color(if page.error.is_some() {
                k.c.warning.fg
            } else {
                k.c.text_faint
            });
    };
    let path = view.path.clone();
    let file = super::group(
        k,
        "config-file",
        "File",
        Some("Edit it in any editor, then restart the daemon: it reads the file only at startup."),
    )
    .child(
        row()
            .w_full()
            .gap(crate::ds::SPACE_4)
            .items_center()
            .child(
                text(view.path.display().to_string())
                    .font(k.ty.mono)
                    .color(k.c.text_muted)
                    .grow(1.0)
                    .truncate(),
            )
            .child(
                Button::new("Open")
                    .variant(ButtonVariant::Outline)
                    .control_size(ControlSize::Small)
                    .on_press(move |_s: &mut AppState| crate::storage::reveal(&path))
                    .build(k)
                    .key("config-open"),
            ),
    );
    let prompt = super::group(
        k,
        "config-pr-prompt",
        "Pull request prompt",
        Some(
            "What the background agent behind Open PR is told, from [pull_requests] \
             system_prompt; the built-in text while that is empty. The hand-over to Forge is \
             always appended.",
        ),
    )
    .child(block(k, "config-pr-prompt-text", &view.pull_request_prompt));
    let effective = super::group(
        k,
        "config-effective",
        "Effective configuration",
        Some("Every key with the value in force, defaults included."),
    )
    .child(block(k, "config-toml", &view.toml));
    col()
        .key("config-page")
        .w_full()
        .gap(crate::ds::SPACE_7)
        .child(file)
        .child(prompt)
        .child(effective)
}

fn block(k: &Kit, key: &str, body: &str) -> El {
    col()
        .key(key.to_string())
        .w_full()
        .padding(crate::ds::SPACE_5)
        .radius(k.radius.md)
        .fill(k.c.surface_weak)
        .child(
            text(body.to_string())
                .font(k.ty.mono)
                .color(k.c.text_muted)
                .wrap(),
        )
}

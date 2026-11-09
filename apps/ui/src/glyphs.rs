//! the original client's own glyphs (the equivalent, the equivalent): one 16 pt
//! grid, a 1.5 round stroke in the current colour, so both clients draw the
//! same marks.

use rune::Icon;

use crate::app::{SessionRow, SessionStatus};

macro_rules! stroke {
    () => {
        r#"<g fill="none" stroke="currentColor" stroke-width="1.5" stroke-linecap="round" stroke-linejoin="round">"#
    };
}

/// Each glyph is `NAME "id" "region" "rest"`. The region is the part the original client's
/// `.glyph-region` fills at 20% when the glyph names the chosen thing, so a
/// chosen view never rests on colour alone; [`chosen`] draws it that way.
macro_rules! glyphs {
    ($($name:ident $id:literal $region:literal $body:literal;)*) => {
        $(
            pub static $name: Icon = Icon {
                name: $id,
                view: 16.0,
                body: concat!(stroke!(), $region, $body, "</g>"),
            };
        )*
        static CHOSEN: &[(&Icon, Icon)] = &[$(
            (
                &$name,
                Icon {
                    name: concat!($id, "-chosen"),
                    view: 16.0,
                    body: concat!(
                        stroke!(),
                        r#"<g fill="currentColor" fill-opacity="0.2">"#,
                        $region,
                        "</g>",
                        $body,
                        "</g>"
                    ),
                },
            ),
        )*];
    };
}

glyphs! {
    PROJECTS "forge-projects"
        r#"<path d="M8 2L1.8 5.2 8 8.4l6.2-3.2z"/>"#
        r#"<path d="M1.8 8L8 11.2 14.2 8M1.8 10.8L8 14l6.2-3.2"/>"#;
    FILES "forge-files"
        r#"<path d="M1.9 4.1a1.2 1.2 0 011.2-1.2h2.6l1.4 1.8h5a1.2 1.2 0 011.2 1.2v6.2a1.2 1.2 0 01-1.2 1.2H3.1a1.2 1.2 0 01-1.2-1.2z"/>"#
        "";
    HISTORY "forge-history"
        r#"<circle cx="8" cy="8" r="6"/>"#
        r#"<path d="M8 4.6V8l2.3 1.6"/>"#;
    PULL_REQUEST "forge-pull-request"
        r#"<circle cx="11.8" cy="12.2" r="1.7"/>"#
        r#"<circle cx="4.2" cy="3.8" r="1.7"/><circle cx="4.2" cy="12.2" r="1.7"/><path d="M4.2 5.5v5"/><path d="M11.8 10.5V6.4a2 2 0 00-2-2H7.6M9.2 3l-1.6 1.4 1.6 1.4"/>"#;
    GIT "forge-git"
        r#"<circle cx="11.6" cy="6.6" r="1.8"/>"#
        r#"<circle cx="4.4" cy="3.9" r="1.8"/><circle cx="4.4" cy="12.1" r="1.8"/><path d="M4.4 5.7v4.6M9.9 8a5 5 0 01-4.1 2.3"/>"#;
    WORKTREE "forge-worktree"
        r#"<rect x="7.4" y="8.4" width="6.6" height="5.4" rx="1.4"/>"#
        r#"<circle cx="4" cy="3.6" r="1.6"/><path d="M4 5.2v4.4a1.4 1.4 0 001.4 1.4h2"/>"#;
    TERMINAL "forge-terminal"
        r#"<rect x="1.8" y="2.6" width="12.4" height="10.8" rx="2.2"/>"#
        r#"<path d="M4.6 6l2 2-2 2M8.4 10.2h3"/>"#;
    CODE "forge-code"
        r#"<path d="M9.4 1.9H4.2a1.4 1.4 0 00-1.4 1.4v9.4a1.4 1.4 0 001.4 1.4h7.6a1.4 1.4 0 001.4-1.4V5.9z"/>"#
        r#"<path d="M9.4 1.9v4h3.8M6.4 8.8L5.2 10l1.2 1.2M9.6 8.8l1.2 1.2-1.2 1.2"/>"#;
    COMMIT "forge-commit"
        r#"<circle cx="8" cy="8" r="2.6"/>"#
        r#"<path d="M1.6 8h3.8M10.6 8h3.8"/>"#;
    FETCH "forge-fetch"
        r#"<path d="M2.6 10.6v1.6a1.4 1.4 0 001.4 1.4h8a1.4 1.4 0 001.4-1.4v-1.6"/>"#
        r#"<path d="M8 2.4v7.4M5 6.8L8 9.8l3-3"/>"#;
    PUSH "forge-push"
        r#"<path d="M2.6 10.6v1.6a1.4 1.4 0 001.4 1.4h8a1.4 1.4 0 001.4-1.4v-1.6"/>"#
        r#"<path d="M8 10.2V2.8M5 5.8L8 2.8l3 3"/>"#;
    PATCH "forge-patch"
        r#"<rect x="2" y="2" width="12" height="12" rx="2.4"/>"#
        r#"<path d="M5.2 6h3M6.7 4.5v3M5.2 10.6h3"/>"#;
    REVIEW "forge-review"
        ""
        r#"<path d="M2.4 4l1.3 1.3L6 3M2.4 9.4l1.3 1.3L6 8.4M8.6 4.4h5M8.6 9.8h5"/>"#;
    MERGED "forge-merged"
        r#"<circle cx="11.8" cy="8" r="1.7"/>"#
        r#"<circle cx="4.4" cy="3.8" r="1.7"/><circle cx="4.4" cy="12.2" r="1.7"/><path d="M4.4 5.5v5M4.6 5.6c.6 1.6 2.4 2.4 5.5 2.4"/>"#;
    CONFLICT "forge-conflict"
        r#"<path d="M2.6 4.4h4.6l2 3.6-2 3.6H2.6M13.4 4.4H11M13.4 11.6H11"/>"#
        "";
    HANDOFF "forge-handoff"
        r#"<path d="M13.4 9.6a1.4 1.4 0 01-1.4 1.4H5.2l-2.6 2.4V3.8a1.4 1.4 0 011.4-1.4h8a1.4 1.4 0 011.4 1.4z"/>"#
        r#"<path d="M6 6.8h4M8.4 5l1.8 1.8-1.8 1.8"/>"#;
    SPAWN_CHILD "forge-spawn-child"
        r#"<rect x="8.6" y="8.2" width="5.4" height="4.6" rx="1.2"/>"#
        r#"<circle cx="4.4" cy="3.8" r="1.7"/><path d="M4.4 5.5v3a2 2 0 002 2h1.6"/><path d="M11.3 9.4v2.2M10.2 10.5h2.2"/>"#;
    SHARED_FILES "forge-shared-files"
        ""
        r#"<path d="M6.6 9.4l2.8-2.8M5.4 7.4L4.2 8.6a2.4 2.4 0 003.4 3.4l1.2-1.2M10.6 8.6l1.2-1.2a2.4 2.4 0 00-3.4-3.4L7.2 5.2"/>"#;
    NEEDS_YOU "forge-needs-you"
        r#"<path d="M4 11.2V7.4a4 4 0 018 0v3.8l1.2 1.4H2.8z"/>"#
        r#"<path d="M6.6 13.6a1.6 1.6 0 002.8 0"/>"#;
    IDLE "forge-idle"
        r#"<path d="M12.8 9.8A5.6 5.6 0 016.2 3.2a5.6 5.6 0 106.6 6.6z"/>"#
        "";
    EXITED "forge-exited"
        r#"<rect x="3.4" y="3.4" width="9.2" height="9.2" rx="2"/>"#
        "";
    FAILED "forge-failed"
        r#"<circle cx="8" cy="8" r="6"/>"#
        r#"<path d="M5.8 5.8l4.4 4.4M10.2 5.8l-4.4 4.4"/>"#;
    SPLIT "forge-split"
        r#"<rect x="1.8" y="2.6" width="12.4" height="10.8" rx="2"/>"#
        r#"<path d="M8 2.6v10.8"/>"#;
    PALETTE "forge-palette"
        r#"<rect x="2" y="2" width="12" height="12" rx="3"/>"#
        r#"<path d="M5.2 6.4l1.6 1.6-1.6 1.6M8.4 10.2h2.4"/>"#;
    DONE "forge-done"
        ""
        r#"<path d="M3.4 8.4l3 3 6.2-6.6"/>"#;

    ARROW_LEFT "forge-arrow-left"
        ""
        r#"<path d="M13.2 8H3.4"/><path d="M7 4.5 3.5 8l3.5 3.5"/>"#;
    CHEVRON_LEFT "forge-chevron-left" "" r#"<path d="M10 4.5 6.4 8 10 11.5"/>"#;
    CHEVRON_RIGHT "forge-chevron-right" "" r#"<path d="M6 4.5 9.6 8 6 11.5"/>"#;
    CHEVRON_UP "forge-chevron-up" "" r#"<path d="M4.5 10 8 6.4 11.5 10"/>"#;
    CHEVRON_DOWN "forge-chevron-down" "" r#"<path d="M4.5 6 8 9.6 11.5 6"/>"#;
    CLOSE "forge-close" "" r#"<path d="M4.3 4.3l7.4 7.4"/><path d="M11.7 4.3l-7.4 7.4"/>"#;
    PLUS "forge-plus" "" r#"<path d="M8 3.4v9.2"/><path d="M3.4 8h9.2"/>"#;
    MINUS "forge-minus" "" r#"<path d="M3.4 8h9.2"/>"#;
    MORE_HORIZONTAL "forge-more-horizontal"
        ""
        r#"<circle cx="3.8" cy="8" r="0.75"/><circle cx="8" cy="8" r="0.75"/><circle cx="12.2" cy="8" r="0.75"/>"#;
    SEARCH "forge-search"
        ""
        r#"<circle cx="6.9" cy="6.9" r="4.1"/><path d="M10 10l3.4 3.4"/>"#;
    FILTER "forge-filter" r#"<path d="M2.8 3.4h10.4l-4 4.7v4.3l-2.4 1.4V8.1z"/>"# "";
    REFRESH "forge-refresh"
        ""
        r#"<path d="M12.9 4.7A5.4 5.4 0 0 0 3.5 6.1"/><path d="M13.2 2.7v2.6h-2.6"/><path d="M3.1 11.3a5.4 5.4 0 0 0 9.4-1.4"/><path d="M2.8 13.3v-2.6h2.6"/>"#;
    SETTINGS "forge-settings"
        ""
        r#"<path d="M13.5 8l-2.7 4.7H5.2L2.5 8l2.7-4.7h5.6z"/><circle cx="8" cy="8" r="2.1"/>"#;
    APPEARANCE "forge-appearance"
        r#"<path d="M8 2.7a5.3 5.3 0 0 1 0 10.6z"/>"#
        r#"<circle cx="8" cy="8" r="5.3"/>"#;
    STATS "forge-stats"
        ""
        r#"<path d="M4.4 12.7V8.9"/><path d="M8 12.7V4.4"/><path d="M11.6 12.7V7.2"/>"#;
    LOADER "forge-loader" "" r#"<path d="M13.5 8A5.5 5.5 0 1 0 8 13.5"/>"#;
    COPY "forge-copy"
        ""
        r#"<path d="M10.2 5.8V4.2a1.6 1.6 0 0 0-1.6-1.6H4.2a1.6 1.6 0 0 0-1.6 1.6v4.4a1.6 1.6 0 0 0 1.6 1.6h1.6"/><rect x="5.8" y="5.8" width="7.6" height="7.6" rx="1.7"/>"#;
    EXTERNAL_LINK "forge-external-link"
        ""
        r#"<path d="M13.4 9.1v2.7a1.6 1.6 0 0 1-1.6 1.6H4.2a1.6 1.6 0 0 1-1.6-1.6V4.2a1.6 1.6 0 0 1 1.6-1.6h2.7"/><path d="M9.7 2.6h3.7v3.7"/><path d="M13.1 2.9 8.8 7.2"/>"#;
    GLOBE "forge-globe"
        ""
        r#"<path d="M2.4 8a5.6 5.6 0 1 0 11.2 0 5.6 5.6 0 1 0-11.2 0z"/><path d="M8 2.4c-1.7 1.6-2.5 3.5-2.5 5.6s.8 4 2.5 5.6c1.7-1.6 2.5-3.5 2.5-5.6S9.7 4 8 2.4z"/><path d="M2.6 8h10.8"/>"#;
    PLAY "forge-play"
        ""
        r#"<path d="M5 3.4v9.2a.6.6 0 0 0 .9.5l7.3-4.6a.6.6 0 0 0 0-1L5.9 2.9a.6.6 0 0 0-.9.5z"/>"#;
    STOP "forge-stop"
        ""
        r#"<path d="M4.6 3.6h6.8a1 1 0 0 1 1 1v6.8a1 1 0 0 1-1 1H4.6a1 1 0 0 1-1-1V4.6a1 1 0 0 1 1-1z"/>"#;
    ACTIVITY "forge-activity" "" r#"<path d="M1.8 8.4h2.6l1.8-4.6 3.4 8.6 1.8-4h2.8"/>"#;
    EDIT "forge-edit"
        ""
        r#"<path d="M11.3 2.7l2 2-8.4 8.4-2.6.6.6-2.6z"/><path d="M9.8 4.2l2 2"/>"#;
    TRASH "forge-trash"
        ""
        r#"<path d="M2.8 4.2h10.4"/><path d="M6.2 4.2V3.1a1.1 1.1 0 0 1 1.1-1.1h1.4a1.1 1.1 0 0 1 1.1 1.1v1.1"/><path d="M4.5 4.4l.6 8.1a1.5 1.5 0 0 0 1.5 1.4h2.8a1.5 1.5 0 0 0 1.5-1.4l.6-8.1"/><path d="M6.9 7v3.6"/><path d="M9.1 7v3.6"/>"#;
    FOLDER_OPEN "forge-folder-open"
        ""
        r#"<path d="M2.2 11.9V4.4a1.2 1.2 0 0 1 1.2-1.2h2.5l1.4 1.7h4.8a1.2 1.2 0 0 1 1.2 1.2v1.2"/><path d="M2.2 12.6l1.8-4.1a1.3 1.3 0 0 1 1.2-.8h8a1.2 1.2 0 0 1 1.2 1.5l-1.3 3.4a1.3 1.3 0 0 1-1.2.8H3.4a1.3 1.3 0 0 1-1.2-.8z"/>"#;
    FILE "forge-file"
        ""
        r#"<path d="M9.4 1.9H4.2a1.4 1.4 0 0 0-1.4 1.4v9.4a1.4 1.4 0 0 0 1.4 1.4h7.6a1.4 1.4 0 0 0 1.4-1.4V5.9z"/><path d="M9.4 1.9v4h3.8"/>"#;
    IMAGE "forge-image"
        ""
        r#"<rect x="2.4" y="3.4" width="11.2" height="9.2" rx="1.6"/><circle cx="5.9" cy="6.7" r="1"/><path d="M2.9 11.7l3.1-3.2 2.3 2.4 2.4-2.6 2.9 3"/>"#;
    IMAGE_OFF "forge-image-off"
        ""
        r#"<rect x="2.4" y="3.4" width="11.2" height="9.2" rx="1.6"/><circle cx="5.9" cy="6.7" r="1"/><path d="M3.1 12.2l2.5-2.6 1.7 1.8"/><path d="M3 3l10 10"/>"#;
    MAXIMIZE "forge-maximize"
        ""
        r#"<path d="M9.5 2.6h3.9v3.9"/><path d="M13.1 2.9 9 7"/><path d="M6.5 13.4H2.6V9.5"/><path d="M2.9 13.1 7 9"/>"#;
    PANEL_LEFT_OPEN "forge-panel-left-open"
        ""
        r#"<rect x="1.9" y="2.9" width="12.2" height="10.2" rx="1.6"/><path d="M6.5 2.9v10.2"/><path d="M9.4 6.3 11.2 8l-1.8 1.7"/>"#;
    PANEL_LEFT_CLOSE "forge-panel-left-close"
        ""
        r#"<rect x="1.9" y="2.9" width="12.2" height="10.2" rx="1.6"/><path d="M6.5 2.9v10.2"/><path d="M10.6 6.3 8.8 8l1.8 1.7"/>"#;
    AGENT "forge-agent"
        ""
        r#"<path d="M8 2.9v2.1"/><circle cx="8" cy="2.2" r="0.65"/><rect x="4.3" y="5" width="7.4" height="7.2" rx="1.8"/><circle cx="6.6" cy="8.5" r="0.75"/><circle cx="9.4" cy="8.5" r="0.75"/><path d="M6.6 10.7h2.8"/>"#;
    SPARKLES "forge-sparkles"
        ""
        r#"<path d="M7 2.7l1.1 3.2 3.2 1.1-3.2 1.1L7 11.3 5.9 8.1 2.7 7l3.2-1.1z"/><path d="M11.9 9.9l.5 1.6 1.6.5-1.6.5-.5 1.6-.5-1.6-1.6-.5 1.6-.5z"/>"#;
    KEYBOARD "forge-keyboard"
        ""
        r#"<rect x="1.9" y="4.1" width="12.2" height="7.8" rx="1.7"/><circle cx="4.6" cy="6.5" r="0.55"/><circle cx="7" cy="6.5" r="0.55"/><circle cx="9.4" cy="6.5" r="0.55"/><circle cx="11.8" cy="6.5" r="0.55"/><path d="M5.5 9.5h5"/>"#;
}

/// `icon` with its region filled, as a chosen tab or view draws it; an icon
/// with no region, or none of these glyphs, draws as itself.
#[must_use]
pub fn chosen(icon: &'static Icon) -> &'static Icon {
    CHOSEN
        .iter()
        .find(|(plain, _)| std::ptr::eq(*plain, icon))
        .map_or(icon, |(_, chosen)| chosen)
}

/// A logotype drawn from the original client's `public/icons/*.svg`, filled in the current
/// colour, so a provider never carries a hue that would rank it above another
/// (the original client's `BrandIcon`).
macro_rules! brand {
    ($(#[$meta:meta])* $name:ident, $id:literal, $view:literal, $path:literal) => {
        $(#[$meta])*
        pub static $name: Icon = Icon {
            name: $id,
            view: $view,
            body: concat!(
                r#"<path fill="currentColor" fill-rule="evenodd" clip-rule="evenodd" d=""#,
                $path,
                r#""/>"#
            ),
        };
    };
}

brand!(
    CLAUDE,
    "forge-brand-claude",
    24.0,
    "M20.998 10.949H24v3.102h-3v3.028h-1.487V20H18v-2.921h-1.487V20H15v-2.921H9V20H7.488v-2.921H6V20H4.487v-2.921H3V14.05H0V10.95h3V5h17.998v5.949zM6 10.949h1.488V8.102H6v2.847zm10.51 0H18V8.102h-1.49v2.847z"
);
brand!(
    CURSOR,
    "forge-brand-cursor",
    24.0,
    "M11.503.131 1.891 5.678a.84.84 0 0 0-.42.726v11.188c0 .3.162.575.42.724l9.609 5.55a1 1 0 0 0 .998 0l9.61-5.55a.84.84 0 0 0 .42-.724V6.404a.84.84 0 0 0-.42-.726L12.497.131a1.01 1.01 0 0 0-.996 0M2.657 6.338h18.55c.263 0 .43.287.297.515L12.23 22.918c-.062.107-.229.064-.229-.06V12.335a.59.59 0 0 0-.295-.51l-9.11-5.257c-.109-.063-.064-.23.061-.23"
);
brand!(
    OPENCODE,
    "forge-brand-opencode",
    24.0,
    "M16 6H8v12h8V6zm4 16H4V2h16v20z"
);
brand!(
    ZED,
    "forge-brand-editor-zed",
    96.0,
    "M9 6a3 3 0 0 0-3 3v66H0V9a9 9 0 0 1 9-9h80.379c4.009 0 6.016 4.847 3.182 7.682L43.055 57.187H57V51h6v7.688a4.5 4.5 0 0 1-4.5 4.5H37.055L26.743 73.5H73.5V36h6v37.5a6 6 0 0 1-6 6H20.743L10.243 90H87a3 3 0 0 0 3-3V21h6v66a9 9 0 0 1-9 9H6.621c-4.009 0-6.016-4.847-3.182-7.682L52.757 39H39v6h-6v-7.5a4.5 4.5 0 0 1 4.5-4.5h21.257l10.5-10.5H22.5V60h-6V22.5a6 6 0 0 1 6-6h52.757L85.757 6H9Z"
);

/// The mark for a provider id, as `brandForProvider`; `None` for one with no
/// logo of its own, which keeps the generic agent glyph.
#[must_use]
pub fn brand(provider: &str) -> Option<&'static Icon> {
    match provider {
        "claude" => Some(&CLAUDE),
        "cursor" => Some(&CURSOR),
        "opencode" => Some(&OPENCODE),
        _ => None,
    }
}

/// A session's identity: its provider's mark, else its kind.
#[must_use]
pub fn for_session(kind: domain::SessionKind, provider: Option<&str>) -> &'static Icon {
    if let Some(mark) = provider.and_then(brand) {
        return mark;
    }
    match kind {
        domain::SessionKind::Agent => &AGENT,
        domain::SessionKind::Editor => &CODE,
        _ if provider.is_some() => &AGENT,
        _ => &TERMINAL,
    }
}

/// What a session's tab and row show, after the original client's `sessionGlyphFace`: the
/// session the person is in keeps its identity, one that stopped to ask is a
/// bell, a busy one a loader, and a finished agent a check. An editor's
/// identity is its file's mark.
#[must_use]
pub fn for_row(row: &SessionRow, active: bool, light: bool) -> &'static Icon {
    if !active {
        match row.status {
            Some(SessionStatus::Attention) => return &NEEDS_YOU,
            Some(SessionStatus::Busy) => return &LOADER,
            None if row.agent && !row.running => return &DONE,
            None => {}
        }
    }
    match row.file {
        Some(file) => file.icon(light),
        None => for_session(row.kind, row.provider.as_deref()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The glyph the original client's `ICONS` registry (the equivalent) names, by that name.
    fn named(name: &str) -> Option<&'static Icon> {
        Some(match name {
            "git-branch" => &GIT,
            "git-pull-request" => &PULL_REQUEST,
            "arrow-left" => &ARROW_LEFT,
            "settings" => &SETTINGS,
            "appearance" => &APPEARANCE,
            "stats" => &STATS,
            "check" => &DONE,
            "loader" => &LOADER,
            "bell" => &NEEDS_YOU,
            "filter" => &FILTER,
            "refresh" => &REFRESH,
            "copy" => &COPY,
            "external-link" => &EXTERNAL_LINK,
            "globe" => &GLOBE,
            "play" => &PLAY,
            "stop" => &STOP,
            "activity" => &ACTIVITY,
            "edit" => &EDIT,
            "trash" => &TRASH,
            "agent" => &AGENT,
            "square-terminal" => &TERMINAL,
            "panel-left-open" => &PANEL_LEFT_OPEN,
            "panel-left-close" => &PANEL_LEFT_CLOSE,
            "chevron-left" => &CHEVRON_LEFT,
            "chevron-right" => &CHEVRON_RIGHT,
            "chevron-down" => &CHEVRON_DOWN,
            "chevron-up" => &CHEVRON_UP,
            "close" => &CLOSE,
            "plus" => &PLUS,
            "minus" => &MINUS,
            "folder-open" => &FOLDER_OPEN,
            "folder" => &FILES,
            "file" => &FILE,
            "file-code" => &CODE,
            "search" => &SEARCH,
            "message-square-plus" => &HANDOFF,
            "columns-2" => &SPLIT,
            "list-checks" => &REVIEW,
            "history" => &HISTORY,
            "layers" => &PROJECTS,
            "maximize" => &MAXIMIZE,
            "more-horizontal" => &MORE_HORIZONTAL,
            "image" => &IMAGE,
            "image-off" => &IMAGE_OFF,
            "sparkles" => &SPARKLES,
            "keyboard" => &KEYBOARD,
            "folder-symlink" => &SHARED_FILES,
            "worktree" => &WORKTREE,
            "commit" => &COMMIT,
            "fetch" => &FETCH,
            "push" => &PUSH,
            "patch" => &PATCH,
            "merged" => &MERGED,
            "conflict" => &CONFLICT,
            "spawn-child" => &SPAWN_CHILD,
            "idle" => &IDLE,
            "exited" => &EXITED,
            "failed" => &FAILED,
            "command-palette" => &PALETTE,
            _ => return None,
        })
    }

    /// the equivalent's `ForgeIconName`, every name the shell may ask for.
    const REGISTRY: [&str; 59] = [
        "git-branch",
        "git-pull-request",
        "arrow-left",
        "settings",
        "appearance",
        "stats",
        "check",
        "loader",
        "bell",
        "filter",
        "refresh",
        "copy",
        "external-link",
        "globe",
        "play",
        "stop",
        "activity",
        "edit",
        "trash",
        "agent",
        "square-terminal",
        "panel-left-open",
        "panel-left-close",
        "chevron-left",
        "chevron-right",
        "chevron-down",
        "chevron-up",
        "close",
        "plus",
        "minus",
        "folder-open",
        "folder",
        "file",
        "file-code",
        "search",
        "message-square-plus",
        "columns-2",
        "list-checks",
        "history",
        "layers",
        "maximize",
        "more-horizontal",
        "image",
        "image-off",
        "sparkles",
        "keyboard",
        "folder-symlink",
        "worktree",
        "commit",
        "fetch",
        "push",
        "patch",
        "merged",
        "conflict",
        "spawn-child",
        "idle",
        "exited",
        "failed",
        "command-palette",
    ];

    #[test]
    fn every_registry_name_draws_its_own_glyph() {
        let mut seen = std::collections::HashSet::new();
        for name in REGISTRY {
            let glyph = named(name).unwrap_or_else(|| panic!("{name} has no glyph"));
            assert!(seen.insert(glyph.name), "{name} shares {}", glyph.name);
            assert!(
                rune::text::icon_coverage(glyph, 16).iter().any(|&c| c > 0),
                "{name} draws nothing"
            );
        }
        assert!(named("claude").is_none());
    }

    #[test]
    fn a_chosen_glyph_fills_its_region_and_one_without_draws_as_itself() {
        let ink = |icon: &Icon| {
            rune::text::icon_coverage(icon, 32)
                .iter()
                .map(|&c| u32::from(c))
                .sum::<u32>()
        };
        assert!(ink(chosen(&FILES)) > ink(&FILES));
        assert_eq!(ink(chosen(&CLOSE)), ink(&CLOSE));
        assert!(std::ptr::eq(chosen(&CLAUDE), &CLAUDE));
    }

    #[test]
    fn a_known_provider_wears_its_mark_and_others_their_kind() {
        use domain::SessionKind;
        assert_eq!(
            for_session(SessionKind::Agent, Some("claude")).name,
            "forge-brand-claude"
        );
        assert_eq!(
            for_session(SessionKind::Agent, Some("cursor")).name,
            "forge-brand-cursor"
        );
        assert_eq!(
            for_session(SessionKind::Agent, Some("opencode")).name,
            "forge-brand-opencode"
        );
        assert_eq!(
            for_session(SessionKind::Agent, Some("gemini")).name,
            "forge-agent"
        );
        assert_eq!(for_session(SessionKind::Shell, None).name, "forge-terminal");
        assert_eq!(for_session(SessionKind::Editor, None).name, "forge-code");
    }
}

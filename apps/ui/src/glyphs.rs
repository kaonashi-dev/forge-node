//! Tauri's own glyphs (`theme/icons/glyphs.tsx`, `uiGlyphs.tsx`): one 16 pt
//! grid, a 1.5 round stroke in the current colour, so both clients draw the
//! same marks.

use rune::Icon;

macro_rules! glyph {
    ($name:ident, $id:literal, $body:literal) => {
        pub static $name: Icon = Icon {
            name: $id,
            view: 16.0,
            body: concat!(
                r#"<g fill="none" stroke="currentColor" stroke-width="1.5" stroke-linecap="round" stroke-linejoin="round">"#,
                $body,
                "</g>"
            ),
        };
    };
}

glyph!(
    TERMINAL,
    "forge-terminal",
    r#"<rect x="1.8" y="2.6" width="12.4" height="10.8" rx="2.2"/><path d="M4.6 6l2 2-2 2M8.4 10.2h3"/>"#
);
glyph!(
    CODE,
    "forge-code",
    r#"<path d="M9.4 1.9H4.2a1.4 1.4 0 00-1.4 1.4v9.4a1.4 1.4 0 001.4 1.4h7.6a1.4 1.4 0 001.4-1.4V5.9z"/><path d="M9.4 1.9v4h3.8M6.4 8.8L5.2 10l1.2 1.2M9.6 8.8l1.2 1.2-1.2 1.2"/>"#
);
glyph!(
    AGENT,
    "forge-agent",
    r#"<path d="M8 2.2l1.5 4.3 4.3 1.5-4.3 1.5L8 13.8l-1.5-4.3L2.2 8l4.3-1.5z"/>"#
);
glyph!(
    PROJECTS,
    "forge-projects",
    r#"<path d="M8 2L1.8 5.2 8 8.4l6.2-3.2z"/><path d="M1.8 8L8 11.2 14.2 8M1.8 10.8L8 14l6.2-3.2"/>"#
);
glyph!(
    FILES,
    "forge-files",
    r#"<path d="M1.9 4.1a1.2 1.2 0 011.2-1.2h2.6l1.4 1.8h5a1.2 1.2 0 011.2 1.2v6.2a1.2 1.2 0 01-1.2 1.2H3.1a1.2 1.2 0 01-1.2-1.2z"/>"#
);
glyph!(
    GIT,
    "forge-git",
    r#"<circle cx="4.4" cy="3.9" r="1.8"/><circle cx="4.4" cy="12.1" r="1.8"/><circle cx="11.6" cy="6.6" r="1.8"/><path d="M4.4 5.7v4.6M9.9 8a5 5 0 01-4.1 2.3"/>"#
);
glyph!(
    PULL_REQUEST,
    "forge-pull-request",
    r#"<circle cx="4.2" cy="3.8" r="1.7"/><circle cx="4.2" cy="12.2" r="1.7"/><path d="M4.2 5.5v5"/><circle cx="11.8" cy="12.2" r="1.7"/><path d="M11.8 10.5V6.4a2 2 0 00-2-2H7.6M9.2 3l-1.6 1.4 1.6 1.4"/>"#
);
glyph!(
    SETTINGS,
    "forge-settings",
    r#"<path d="M13.5 8l-2.7 4.7H5.2L2.5 8l2.7-4.7h5.6z"/><circle cx="8" cy="8" r="2.1"/>"#
);
glyph!(PLUS, "forge-plus", r#"<path d="M8 3.4v9.2M3.4 8h9.2"/>"#);
glyph!(
    CLOSE,
    "forge-close",
    r#"<path d="M4.3 4.3l7.4 7.4M11.7 4.3l-7.4 7.4"/>"#
);
glyph!(
    SEARCH,
    "forge-search",
    r#"<circle cx="6.9" cy="6.9" r="4.1"/><path d="M10 10l3.4 3.4"/>"#
);
glyph!(
    ARROW_LEFT,
    "forge-arrow-left",
    r#"<path d="M12.6 8H3.4M7 4.4L3.4 8 7 11.6"/>"#
);
glyph!(
    HISTORY,
    "forge-history",
    r#"<circle cx="8" cy="8" r="6"/><path d="M8 4.6V8l2.3 1.6"/>"#
);
glyph!(
    ACTIVITY,
    "forge-activity",
    r#"<path d="M1.8 8.4h2.6l1.8-4.6 3.4 8.6 1.8-4h2.8"/>"#
);
glyph!(
    REFRESH,
    "forge-refresh",
    r#"<path d="M12.9 4.7A5.4 5.4 0 0 0 3.5 6.1M13.2 2.7v2.6h-2.6M3.1 11.3a5.4 5.4 0 0 0 9.4-1.4M2.8 13.3v-2.6h2.6"/>"#
);
glyph!(
    STOP,
    "forge-stop",
    r#"<path d="M4.6 3.6h6.8a1 1 0 0 1 1 1v6.8a1 1 0 0 1-1 1H4.6a1 1 0 0 1-1-1V4.6a1 1 0 0 1 1-1z"/>"#
);
glyph!(
    GLOBE,
    "forge-globe",
    r#"<path d="M2.4 8a5.6 5.6 0 1 0 11.2 0 5.6 5.6 0 1 0-11.2 0zM8 2.4c-1.7 1.6-2.5 3.5-2.5 5.6s.8 4 2.5 5.6c1.7-1.6 2.5-3.5 2.5-5.6S9.7 4 8 2.4zM2.6 8h10.8"/>"#
);

/// A provider's own mark, drawn from Tauri's `public/icons/provider-*.svg`:
/// filled in the current colour, so a provider never carries a hue that
/// would rank it above another (Tauri's `BrandIcon`).
macro_rules! brand {
    ($name:ident, $id:literal, $path:literal) => {
        pub static $name: Icon = Icon {
            name: $id,
            view: 24.0,
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
    "M20.998 10.949H24v3.102h-3v3.028h-1.487V20H18v-2.921h-1.487V20H15v-2.921H9V20H7.488v-2.921H6V20H4.487v-2.921H3V14.05H0V10.95h3V5h17.998v5.949zM6 10.949h1.488V8.102H6v2.847zm10.51 0H18V8.102h-1.49v2.847z"
);
brand!(
    CURSOR,
    "forge-brand-cursor",
    "M11.503.131 1.891 5.678a.84.84 0 0 0-.42.726v11.188c0 .3.162.575.42.724l9.609 5.55a1 1 0 0 0 .998 0l9.61-5.55a.84.84 0 0 0 .42-.724V6.404a.84.84 0 0 0-.42-.726L12.497.131a1.01 1.01 0 0 0-.996 0M2.657 6.338h18.55c.263 0 .43.287.297.515L12.23 22.918c-.062.107-.229.064-.229-.06V12.335a.59.59 0 0 0-.295-.51l-9.11-5.257c-.109-.063-.064-.23.061-.23"
);
brand!(
    OPENCODE,
    "forge-brand-opencode",
    "M16 6H8v12h8V6zm4 16H4V2h16v20z"
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

/// What a session's row and tab show: its provider's mark, else its kind.
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

#[cfg(test)]
mod tests {
    use super::*;

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

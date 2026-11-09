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

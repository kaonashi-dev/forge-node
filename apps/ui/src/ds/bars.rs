use rune::{Color, El, Icon, el, icon, row, text};
use rune_kit::Kit;

use super::atoms::{IconButtonSize, icon_button};
use super::metrics::{CONTROL, ICON_BUTTON, ICON_ROW, SPACE_1, SPACE_3, SPACE_4};
use crate::theme::roles::Roles;

/// How much of a tab is drawn while it is not the chosen one.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum TabShape {
    /// Glyph and title always.
    Titled,
    /// Glyph alone at `width` until chosen, then glyph and title: a view
    /// switcher whose names would not fit side by side.
    IconUntilChosen { width: f32 },
    /// Glyph alone, chosen or not, sharing the bar's width equally: a
    /// switcher too narrow for even the chosen name.
    Icon,
}

/// A tab: kind glyph, title, and a close button on the chosen one. The same
/// piece serves the session strip and the sidebar's views; handlers go on the
/// built element.
pub struct Tab {
    key: String,
    title: String,
    glyph: &'static Icon,
    tint: Option<Color>,
    shape: TabShape,
    height: f32,
    chosen: bool,
    close: Option<El>,
}

impl Tab {
    pub fn new(key: impl Into<String>, title: impl Into<String>, glyph: &'static Icon) -> Self {
        Self {
            key: key.into(),
            title: title.into(),
            glyph,
            tint: None,
            shape: TabShape::Titled,
            height: CONTROL,
            chosen: false,
            close: None,
        }
    }

    /// The glyph's ink when its state says more than the kind, as a session
    /// waiting on someone.
    #[must_use]
    pub fn tint(mut self, tint: Color) -> Self {
        self.tint = Some(tint);
        self
    }

    #[must_use]
    pub fn shape(mut self, shape: TabShape) -> Self {
        self.shape = shape;
        self
    }

    #[must_use]
    pub fn height(mut self, height: f32) -> Self {
        self.height = height;
        self
    }

    #[must_use]
    pub fn chosen(mut self, chosen: bool) -> Self {
        self.chosen = chosen;
        self
    }

    /// The close button the chosen tab carries: `key` and `label` name it,
    /// `press` closes.
    #[must_use]
    pub fn close<S: 'static>(
        mut self,
        k: &Kit,
        key: String,
        label: String,
        press: impl Fn(&mut S) + 'static,
    ) -> Self {
        self.close = Some(
            icon_button(&crate::glyphs::CLOSE, label, IconButtonSize::Mini)
                .on_press(press)
                .build(k)
                .key(key),
        );
        self
    }

    #[must_use]
    pub fn build(self, k: &Kit, roles: &Roles) -> El {
        let titled = match self.shape {
            TabShape::Titled => true,
            TabShape::IconUntilChosen { .. } => self.chosen,
            TabShape::Icon => false,
        };
        let glyph = if self.chosen {
            crate::glyphs::chosen(self.glyph)
        } else {
            self.glyph
        };
        let ink = self.tint.unwrap_or(if self.chosen {
            k.c.icon_base
        } else {
            k.c.icon_muted
        });
        let glyph_size = if titled { ICON_ROW } else { ICON_BUTTON };
        let mut tab = row()
            .key(self.key)
            .role("tab")
            .label(self.title.clone())
            .checked(self.chosen)
            .h(self.height)
            .gap(SPACE_3)
            .radius(k.radius.md)
            .center_y()
            .child(icon(glyph, glyph_size).color(ink));
        tab = match self.shape {
            TabShape::IconUntilChosen { width } if !self.chosen => tab.w(width).justify_center(),
            TabShape::IconUntilChosen { .. } => {
                tab.grow(1.0).min_w(0.0).justify_center().px(SPACE_4)
            }
            TabShape::Icon => tab.grow(1.0).min_w(0.0).justify_center(),
            TabShape::Titled => {
                tab.pl(SPACE_4 + SPACE_1)
                    .pr(if self.close.is_some() && self.chosen {
                        SPACE_1
                    } else {
                        SPACE_4 + SPACE_1
                    })
            }
        };
        if titled {
            tab = tab.child(
                text(self.title)
                    .font(if self.chosen { k.ty.ui_medium } else { k.ty.ui })
                    .color(if self.chosen {
                        k.c.text_strong
                    } else {
                        k.c.text_muted
                    })
                    .truncate()
                    .min_w(0.0)
                    .shrink()
                    .a11y_hidden(),
            );
        }
        if self.chosen {
            tab = tab.fill(roles.surface_hi).children(self.close);
        } else {
            tab = k.interactive(tab);
        }
        tab
    }
}

/// A bar with a start, a gap and an end: navigation leading, actions
/// trailing, the main one at the far end.
#[must_use]
pub fn toolbar(key: &str, leading: Vec<El>, trailing: Vec<El>) -> El {
    row()
        .key(key.to_string())
        .w_full()
        .gap(SPACE_4)
        .center_y()
        .children(leading)
        .child(el().grow(1.0).min_w(0.0))
        .children(trailing)
}

use rune::Color;
use rune_kit::Kit;

use crate::theme::roles::Roles;

/// What a status means, never which colour it is.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Tone {
    Neutral,
    Accent,
    Success,
    Warning,
    Danger,
    Info,
    NeedsYou,
}

/// A tone resolved against one theme: `fg` reads on `soft`, `solid` marks a dot.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Ink {
    pub fg: Color,
    pub soft: Color,
    pub solid: Color,
}

impl Tone {
    #[must_use]
    pub fn ink(self, k: &Kit, roles: &Roles) -> Ink {
        let of = |tone: crate::theme::roles::Tone| Ink {
            fg: tone.text,
            soft: tone.soft,
            solid: tone.solid,
        };
        match self {
            Tone::Neutral => Ink {
                fg: k.c.text_muted,
                soft: k.c.layer_03,
                solid: k.c.icon_muted,
            },
            Tone::Accent => of(roles.accent),
            Tone::Success => of(roles.success),
            Tone::Warning => of(roles.warning),
            Tone::Danger => of(roles.danger),
            Tone::Info => of(roles.info),
            Tone::NeedsYou => of(roles.needs_you),
        }
    }
}

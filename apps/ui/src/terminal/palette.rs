use domain::Color;

pub const fn rgb(r: u8, g: u8, b: u8) -> u32 {
    r as u32 | (g as u32) << 8 | (b as u32) << 16 | 0xff00_0000
}

#[derive(Clone)]
pub struct Palette {
    pub fg: u32,
    pub bg: u32,
    pub ansi: [u32; 256],
}

const FORGE_DARK: [u32; 16] = [
    rgb(0x1a, 0x19, 0x18),
    rgb(0xe2, 0x64, 0x5a),
    rgb(0x9c, 0xbf, 0x57),
    rgb(0xe0, 0xb3, 0x41),
    rgb(0x7f, 0xa9, 0xcc),
    rgb(0xb9, 0x8b, 0xc9),
    rgb(0x4f, 0xa3, 0x9b),
    rgb(0xae, 0xa6, 0x9a),
    rgb(0x5e, 0x57, 0x4f),
    rgb(0xee, 0x81, 0x77),
    rgb(0xb0, 0xce, 0x73),
    rgb(0xed, 0xc6, 0x5f),
    rgb(0x9c, 0xbf, 0xdc),
    rgb(0xc9, 0xa4, 0xd6),
    rgb(0x6f, 0xbb, 0xb3),
    rgb(0xed, 0xe5, 0xd6),
];

fn parse_hex(value: &str) -> Option<u32> {
    let hex = value.strip_prefix('#')?;
    if hex.len() != 6 {
        return None;
    }
    let n = u32::from_str_radix(hex, 16).ok()?;
    Some(rgb((n >> 16) as u8, (n >> 8) as u8, n as u8))
}

impl Palette {
    /// The terminal colours of a built-in base, or forge-dark when the base
    /// is unknown or its entry does not parse.
    pub fn for_base(base: &str) -> Self {
        let mut palette = Self::forge_dark();
        let Some(theme) = crate::theme::theme(base) else {
            return palette;
        };
        let colors = &theme.palette;
        let named: Option<Vec<u32>> = colors.ansi.iter().take(16).map(|c| parse_hex(c)).collect();
        if let (Some(fg), Some(bg), Some(named)) = (
            parse_hex(&colors.term_fg),
            parse_hex(&colors.term_bg),
            named,
        ) && named.len() == 16
        {
            palette.fg = fg;
            palette.bg = bg;
            palette.ansi[..16].copy_from_slice(&named);
        }
        palette
    }

    /// The 16 named colors and default fg/bg are `forge-dark`'s; 16..=255 is
    /// the xterm cube and ramp.
    pub fn forge_dark() -> Self {
        let mut ansi = [0u32; 256];
        ansi[..16].copy_from_slice(&FORGE_DARK);
        let level = |v: u32| if v == 0 { 0 } else { (55 + 40 * v) as u8 };
        for i in 16..232u32 {
            let n = i - 16;
            ansi[i as usize] = rgb(level(n / 36), level(n / 6 % 6), level(n % 6));
        }
        for i in 232..256u32 {
            let g = (8 + 10 * (i - 232)) as u8;
            ansi[i as usize] = rgb(g, g, g);
        }
        Palette {
            fg: rgb(0xcf, 0xc6, 0xb8),
            bg: rgb(0x16, 0x15, 0x14),
            ansi,
        }
    }

    #[inline]
    pub fn resolve(&self, color: Color, default: u32) -> u32 {
        match color {
            Color::Indexed(i) => self.ansi[i as usize],
            Color::Rgb(r, g, b) => rgb(r, g, b),
            _ => default,
        }
    }
}

/// `a` over `b` at `alpha/256`, per channel.
#[inline]
pub fn mix(a: u32, b: u32, alpha: u32) -> u32 {
    let ch = |shift: u32| {
        let (x, y) = ((a >> shift) & 0xff, (b >> shift) & 0xff);
        (x * alpha + y * (256 - alpha)) >> 8
    };
    ch(0) | ch(8) << 8 | ch(16) << 16 | 0xff00_0000
}

pub fn with_alpha(color: u32, alpha: u8) -> u32 {
    (color & 0x00ff_ffff) | (alpha as u32) << 24
}

#[cfg(test)]
mod theme_tests {
    use super::*;

    #[test]
    fn a_base_brings_its_own_terminal_colours() {
        let calm = Palette::for_base("calm-dark");
        assert_eq!(calm.bg, rgb(0x11, 0x11, 0x12), "calm-dark termBg");
        assert_eq!(calm.fg, rgb(0xc9, 0xc9, 0xcf));
        let light = Palette::for_base("calm-light");
        assert_ne!(light.bg, calm.bg);
        assert_eq!(
            Palette::for_base("no-such-base").bg,
            Palette::forge_dark().bg
        );
    }

    #[test]
    fn indexed_colours_follow_xterm_as_cell_grid_palette_does() {
        let palette = Palette::for_base("calm-dark");
        let at = |index: u8| palette.resolve(Color::Indexed(index), 0);
        assert_ne!(
            palette.fg, palette.bg,
            "INVERSE needs the two defaults apart"
        );
        assert_eq!(palette.resolve(Color::Default, palette.bg), palette.bg);
        assert_eq!(
            at(0),
            palette.ansi[0],
            "the named sixteen come from the theme"
        );
        assert_eq!(at(15), palette.ansi[15]);
        assert_eq!(at(16), rgb(0, 0, 0));
        assert_eq!(at(231), rgb(0xff, 0xff, 0xff));
        assert_eq!(at(196), rgb(0xff, 0, 0));
        assert_eq!(at(46), rgb(0, 0xff, 0));
        assert_eq!(at(21), rgb(0, 0, 0xff));
        assert_eq!(at(232), rgb(8, 8, 8), "the grey ramp");
        assert_eq!(at(255), rgb(0xee, 0xee, 0xee));
        assert_eq!(palette.resolve(Color::Rgb(1, 2, 3), 0), rgb(1, 2, 3));
    }
}

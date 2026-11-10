use std::collections::HashMap;
use std::sync::{Mutex, MutexGuard, OnceLock};
use std::time::Instant;

use fontique::{
    Blob, Collection, CollectionOptions, FallbackKey, GenericFamily, QueryStatus,
    Script as IsoScript, SourceCache, SourceCacheOptions,
};
use swash::scale::image::Content;
use swash::scale::{Render, ScaleContext, Source, StrikeWith};
use swash::shape::ShapeContext;
use swash::text::{Category, Codepoint, Script};
use swash::zeno::Format;
use swash::{CacheKey, FontRef};

pub const ATLAS_SIZE: u32 = 2048;
const FONTS: [&[u8]; 3] = [
    include_bytes!("../../assets/GeistMono-Regular.ttf"),
    include_bytes!("../../assets/GeistMono-Bold.ttf"),
    include_bytes!("../../assets/GeistMono-Italic.ttf"),
];

pub const REGULAR: usize = 0;
pub const BOLD: usize = 1;
pub const ITALIC: usize = 2;
const PRIMARY_FACES: usize = 3;

/// A cached "draws nothing" run (blank, or only invisible scalars).
pub const NO_RUN: u32 = u32::MAX - 1;
const UNSET: u32 = u32::MAX;
pub const MAX_GLYPHS_PER_CELL: usize = 3;
const SYMBOL_FAMILIES: [&str; 5] = [
    "Apple Symbols",
    "Arial Unicode MS",
    "Heiti SC",
    "Hiragino Sans GB",
    "Apple SD Gothic Neo",
];

#[derive(Clone, Copy, Default)]
pub struct Slot {
    pub x: u16,
    pub y: u16,
    pub w: u16,
    pub h: u16,
    pub left: i16,
    pub top: i16,
}

/// One rasterized glyph placed from the cell's left edge and the baseline, in device pixels.
#[derive(Clone, Copy)]
pub struct Placed {
    pub slot: Slot,
    pub x: f32,
    pub y: f32,
}

#[derive(Clone, Copy, Debug)]
#[allow(
    dead_code,
    reason = "the fields are asserted by tests and read when debugging a run"
)]
pub struct Run {
    pub start: u32,
    pub len: u8,
    /// 0..3 are the bundled Geist Mono styles; larger is a system fallback face.
    pub face: u16,
    pub notdef: bool,
    /// A color glyph (emoji) drawn as a monochrome mask; see [`flatten_color`].
    pub flattened: bool,
    pub truncated: bool,
}

pub struct Upload {
    pub x: u32,
    pub y: u32,
    pub w: u32,
    pub h: u32,
    pub data: Vec<u8>,
}

struct FallbackFace {
    blob: Blob<u8>,
    offset: u32,
    key: CacheKey,
}

impl FallbackFace {
    fn font(&self) -> FontRef<'_> {
        FontRef {
            data: self.blob.data(),
            offset: self.offset,
            key: self.key,
        }
    }
}

struct Fallback {
    collection: Collection,
    cache: SourceCache,
    faces: Vec<FallbackFace>,
    by_char: HashMap<char, Option<usize>>,
    /// Installed Nerd Font families, best first; listed once, on first need.
    nerd: Option<Vec<String>>,
}

/// The Nerd Font a prompt icon is looked for in first.
const PREFERRED_NERD: &str = "JetBrainsMono Nerd Font Mono";

/// Installed Nerd Font families, in the order a prompt icon is looked for:
/// `PREFERRED_NERD`, then any monospaced one (its icons fit a cell), then the
/// rest. Prompt glyphs live in the Private Use Area, which no script
/// fallback and no system symbol face covers.
fn nerd_families<'a>(names: impl Iterator<Item = &'a str>) -> Vec<String> {
    let mut found: Vec<&str> = names.filter(|name| name.contains("Nerd Font")).collect();
    found.sort_by_key(|name| (*name != PREFERRED_NERD, !name.ends_with("Mono"), *name));
    found.dedup();
    found.into_iter().map(str::to_string).collect()
}

fn invisible(c: char) -> bool {
    matches!(c, '\u{200b}'..='\u{200d}' | '\u{fe00}'..='\u{fe0f}' | '\u{2060}')
}

pub fn is_extender(c: char) -> bool {
    invisible(c)
        || matches!(
            c.category(),
            Category::Mark
                | Category::SpacingMark
                | Category::EnclosingMark
                | Category::NonspacingMark
        )
        || ('\u{1f3fb}'..='\u{1f3ff}').contains(&c)
}

fn iso_script(c: char) -> IsoScript {
    let mut tag = c.script().to_opentype().to_be_bytes();
    tag[0] = tag[0].to_ascii_uppercase();
    IsoScript::from_bytes(tag)
}

fn wants_emoji(c: char, text: &str) -> bool {
    text.contains('\u{fe0f}') || (c.is_emoji() && c as u32 >= 0x1f000)
}

impl Fallback {
    fn new() -> Self {
        Fallback {
            collection: Collection::new(CollectionOptions {
                shared: false,
                system_fonts: true,
            }),
            cache: SourceCache::new(SourceCacheOptions::default()),
            faces: Vec::new(),
            by_char: HashMap::new(),
            nerd: None,
        }
    }

    fn covers(face: &FallbackFace, c: char) -> bool {
        face.font().charmap().map(c) != 0
    }

    fn find(&mut self, c: char, emoji: bool) -> Option<usize> {
        if let Some(found) = self.by_char.get(&c) {
            return *found;
        }
        let found = self
            .faces
            .iter()
            .position(|f| Self::covers(f, c))
            .or_else(|| self.load(c, emoji));
        self.by_char.insert(c, found);
        found
    }

    fn load(&mut self, c: char, emoji: bool) -> Option<usize> {
        let mut query = self.collection.query(&mut self.cache);
        if emoji {
            query.set_families([GenericFamily::Emoji]);
        } else {
            query.set_fallbacks(FallbackKey::new(iso_script(c), None));
        }
        let mut hit = probe(&mut query, c);
        drop(query);
        if hit.is_none() && !emoji {
            let mut query = self.collection.query(&mut self.cache);
            query.set_families(SYMBOL_FAMILIES);
            hit = probe(&mut query, c);
        }
        if hit.is_none() && !emoji {
            let families = match &self.nerd {
                Some(families) => families.clone(),
                None => {
                    let families = nerd_families(self.collection.family_names());
                    self.nerd = Some(families.clone());
                    families
                }
            };
            if !families.is_empty() {
                let mut query = self.collection.query(&mut self.cache);
                query.set_families(families.iter().map(String::as_str));
                hit = probe(&mut query, c);
            }
        }
        let face = hit?;
        if let Some(i) = self
            .faces
            .iter()
            .position(|f| f.blob.id() == face.blob.id() && f.offset == face.offset)
        {
            return Some(i);
        }
        self.faces.push(face);
        Some(self.faces.len() - 1)
    }
}

fn probe(query: &mut fontique::Query<'_>, c: char) -> Option<FallbackFace> {
    let mut hit = None;
    query.matches_with(|font| {
        if let Some(r) = FontRef::from_index(font.blob.data(), font.index as usize)
            && r.charmap().map(c) != 0
        {
            hit = Some(FallbackFace {
                blob: font.blob.clone(),
                offset: r.offset,
                key: r.key,
            });
            return QueryStatus::Stop;
        }
        QueryStatus::Continue
    });
    hit
}

static FALLBACK: OnceLock<Mutex<Fallback>> = OnceLock::new();

fn fallback() -> MutexGuard<'static, Fallback> {
    FALLBACK
        .get_or_init(|| Mutex::new(Fallback::new()))
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

/// Scans the system fonts on a background thread so the first CJK or emoji cell does not pay for it on the render thread.
pub fn prewarm_fallbacks() {
    std::thread::spawn(|| {
        let mut f = fallback();
        for (c, emoji) in [
            ('漢', false),
            ('か', false),
            ('한', false),
            ('😀', true),
            ('❤', true),
        ] {
            f.find(c, emoji);
        }
    });
}

pub struct Shaper {
    pub cell_w: f32,
    pub cell_h: f32,
    pub baseline: f32,
    pub scale: f32,
    pub generation: u64,
    pub misses: u64,
    pub shapes: u64,
    pub rasters: u64,
    pub notdef_runs: u64,
    pub shape_us: f64,
    pub shape_us_max: f64,
    pub fallback_us: f64,
    pub pending: Vec<Upload>,
    fonts: [FontRef<'static>; PRIMARY_FACES],
    size_px: f32,
    ctx: ScaleContext,
    shape_ctx: ShapeContext,
    ascii: [[u32; 128]; PRIMARY_FACES],
    maps: [HashMap<Box<str>, u32>; PRIMARY_FACES * 2],
    runs: Vec<Run>,
    arena: Vec<Placed>,
    slots: HashMap<(u16, u16, u32), (Slot, bool)>,
    shelf_x: u32,
    shelf_y: u32,
    shelf_h: u32,
}

/// Color glyphs are flattened to coverage: alpha scaled by a floor plus luminance, tinted with the cell's foreground.
/// The floor keeps dark features (eyes, outlines) visible instead of cutting holes.
pub fn flatten_color(rgba: &[u8]) -> Vec<u8> {
    rgba.as_chunks::<4>()
        .0
        .iter()
        .map(|p| {
            let luma = (0.2126 * p[0] as f32 + 0.7152 * p[1] as f32 + 0.0722 * p[2] as f32) / 255.0;
            (p[3] as f32 * (0.25 + 0.75 * luma)).round() as u8
        })
        .collect()
}

impl Shaper {
    pub fn new(cell_pt: (f32, f32), scale: f32) -> Shaper {
        let fonts = FONTS.map(|bytes| FontRef::from_index(bytes, 0).expect("embedded font parses"));
        let font = fonts[REGULAR];
        let metrics = font.metrics(&[]);
        let upem = metrics.units_per_em as f32;
        let adv = font
            .glyph_metrics(&[])
            .advance_width(font.charmap().map('M'));
        let cell_w = (cell_pt.0 * scale).round();
        let cell_h = (cell_pt.1 * scale).round();
        let size_px = cell_w * upem / adv;
        let content = (metrics.ascent + metrics.descent) * size_px / upem;
        let baseline = ((cell_h - content) / 2.0 + metrics.ascent * size_px / upem).round();
        Shaper {
            cell_w,
            cell_h,
            baseline,
            scale,
            generation: 0,
            misses: 0,
            shapes: 0,
            rasters: 0,
            notdef_runs: 0,
            shape_us: 0.0,
            shape_us_max: 0.0,
            fallback_us: 0.0,
            pending: Vec::new(),
            fonts,
            size_px,
            ctx: ScaleContext::new(),
            shape_ctx: ShapeContext::new(),
            ascii: [[UNSET; 128]; PRIMARY_FACES],
            maps: std::array::from_fn(|_| HashMap::new()),
            runs: Vec::new(),
            arena: Vec::new(),
            slots: HashMap::new(),
            shelf_x: 1,
            shelf_y: 1,
            shelf_h: 0,
        }
    }

    #[must_use]
    pub fn fallback_faces(&self) -> usize {
        FALLBACK
            .get()
            .map_or(0, |f| f.lock().map_or(0, |f| f.faces.len()))
    }

    #[must_use]
    pub fn run_at(&self, id: u32) -> Run {
        self.runs[id as usize]
    }

    #[must_use]
    pub fn placed(&self, run: &Run, k: usize) -> Placed {
        self.arena[run.start as usize + k]
    }

    /// The glyph run for one grapheme cluster in a cell `span` columns wide. A hit is an array index or one hash lookup and allocates nothing.
    #[inline]
    pub fn run(&mut self, text: &str, style: usize, span: usize) -> u32 {
        let bytes = text.as_bytes();
        if span == 1 && bytes.len() == 1 && bytes[0] < 128 {
            let id = self.ascii[style][bytes[0] as usize];
            if id != UNSET {
                return id;
            }
        } else if let Some(&id) = self.maps[style * 2 + span - 1].get(text) {
            return id;
        }
        let id = self.shape(text, style, span);
        if span == 1 && bytes.len() == 1 && bytes[0] < 128 {
            self.ascii[style][bytes[0] as usize] = id;
        } else {
            self.maps[style * 2 + span - 1].insert(text.into(), id);
        }
        id
    }

    fn shape(&mut self, text: &str, style: usize, span: usize) -> u32 {
        let started = Instant::now();
        self.misses += 1;
        self.shapes += 1;
        let Some(base) = text
            .chars()
            .find(|&c| !is_extender(c))
            .or_else(|| text.chars().find(|c| !invisible(*c)))
        else {
            return NO_RUN;
        };
        if base == ' ' && text.chars().all(|c| c == ' ' || invisible(c)) {
            return NO_RUN;
        }
        let face = self.choose_face(text, style, base);
        let span_px = span as f32 * self.cell_w;
        let (mut px, fallback) = (self.size_px, face >= PRIMARY_FACES);
        let mut glyphs = self.shape_glyphs(face, text, base, px);
        let natural: f32 = glyphs.iter().map(|g| g.3).sum();
        if fallback && natural > span_px + 0.5 {
            px *= span_px / natural;
            glyphs = self.shape_glyphs(face, text, base, px);
        }
        let advance: f32 = glyphs.iter().map(|g| g.3).sum();
        let offset = if fallback {
            ((span_px - advance) / 2.0).max(0.0)
        } else {
            0.0
        };
        let generation = self.generation;
        let (mut start, mut notdef, mut flattened) = self.place(face, &glyphs, px, offset);
        if self.generation != generation {
            // A full atlas was reset mid-run: the glyphs placed before it point at evicted texels.
            self.arena.clear();
            (start, notdef, flattened) = self.place(face, &glyphs, px, offset);
        }
        let len = self.arena.len() as u32 - start;
        if len == 0 {
            return NO_RUN;
        }
        if notdef {
            self.notdef_runs += 1;
        }
        self.runs.push(Run {
            start,
            len: len.min(255) as u8,
            face: face as u16,
            notdef,
            flattened,
            truncated: len as usize > MAX_GLYPHS_PER_CELL,
        });
        let elapsed = started.elapsed().as_secs_f64() * 1e6;
        self.shape_us += elapsed;
        self.shape_us_max = self.shape_us_max.max(elapsed);
        (self.runs.len() - 1) as u32
    }

    fn place(
        &mut self,
        face: usize,
        glyphs: &[(u16, f32, f32, f32)],
        px: f32,
        offset: f32,
    ) -> (u32, bool, bool) {
        let start = self.arena.len() as u32;
        let (mut pen, mut notdef, mut flattened) = (0.0f32, false, false);
        for &(gid, gx, gy, adv) in glyphs {
            notdef |= gid == 0;
            let (slot, flat) = self.glyph_slot(face, gid, px);
            flattened |= flat;
            if slot.w != 0 {
                let x = (offset + pen + gx + slot.left as f32).round();
                let y = -(slot.top as f32) - gy.round();
                self.arena.push(Placed { slot, x, y });
            }
            pen += adv;
        }
        (start, notdef, flattened)
    }

    fn choose_face(&mut self, text: &str, style: usize, base: char) -> usize {
        let emoji = wants_emoji(base, text);
        if !emoji {
            let map = self.fonts[style].charmap();
            if text
                .chars()
                .filter(|&c| !invisible(c))
                .all(|c| map.map(c) != 0)
            {
                return style;
            }
        }
        let started = Instant::now();
        let found = fallback().find(base, emoji);
        self.fallback_us += started.elapsed().as_secs_f64() * 1e6;
        found.map_or(style, |i| PRIMARY_FACES + i)
    }

    fn with_font<R>(&mut self, face: usize, f: impl FnOnce(&mut Self, FontRef<'_>) -> R) -> R {
        if face < PRIMARY_FACES {
            let font = self.fonts[face];
            f(self, font)
        } else {
            let (blob, offset, key) = {
                let guard = fallback();
                let face = &guard.faces[face - PRIMARY_FACES];
                (face.blob.clone(), face.offset, face.key)
            };
            f(
                self,
                FontRef {
                    data: blob.data(),
                    offset,
                    key,
                },
            )
        }
    }

    fn shape_glyphs(
        &mut self,
        face: usize,
        text: &str,
        base: char,
        px: f32,
    ) -> Vec<(u16, f32, f32, f32)> {
        let script: Script = base.script();
        self.with_font(face, |this, font| {
            let mut out = Vec::new();
            let mut shaper = this.shape_ctx.builder(font).script(script).size(px).build();
            shaper.add_str(text);
            shaper.shape_with(|cluster| {
                out.extend(cluster.glyphs.iter().map(|g| (g.id, g.x, g.y, g.advance)))
            });
            out
        })
    }

    fn glyph_slot(&mut self, face: usize, gid: u16, px: f32) -> (Slot, bool) {
        let key = (face as u16, gid, px.to_bits());
        if let Some(found) = self.slots.get(&key) {
            return *found;
        }
        self.misses += 1;
        self.rasters += 1;
        let image = self.with_font(face, |this, font| {
            let mut scaler = this.ctx.builder(font).size(px).hint(false).build();
            Render::new(&[
                Source::ColorOutline(0),
                Source::ColorBitmap(StrikeWith::BestFit),
                Source::Outline,
            ])
            .format(Format::Alpha)
            .render(&mut scaler, gid)
        });
        let result = match image {
            Some(img)
                if img.placement.width > 0
                    && img.placement.height > 0
                    && matches!(img.content, Content::Mask | Content::Color) =>
            {
                let (w, h) = (img.placement.width, img.placement.height);
                let flat = img.content == Content::Color;
                let data = if flat {
                    flatten_color(&img.data)
                } else {
                    img.data
                };
                (
                    self.pack(w, h, data, img.placement.left, img.placement.top),
                    flat,
                )
            }
            _ => (Slot::default(), false),
        };
        self.slots.insert(key, result);
        result
    }

    fn pack(&mut self, w: u32, h: u32, data: Vec<u8>, left: i32, top: i32) -> Slot {
        if self.shelf_x + w + 1 > ATLAS_SIZE {
            self.shelf_x = 1;
            self.shelf_y += self.shelf_h + 1;
            self.shelf_h = 0;
        }
        if self.shelf_y + h + 1 > ATLAS_SIZE {
            self.reset();
        }
        let (x, y) = (self.shelf_x, self.shelf_y);
        self.shelf_x += w + 1;
        self.shelf_h = self.shelf_h.max(h);
        self.pending.push(Upload { x, y, w, h, data });
        Slot {
            x: x as u16,
            y: y as u16,
            w: w as u16,
            h: h as u16,
            left: left as i16,
            top: top as i16,
        }
    }

    fn reset(&mut self) {
        self.ascii = [[UNSET; 128]; PRIMARY_FACES];
        self.maps.iter_mut().for_each(HashMap::clear);
        self.runs.clear();
        self.arena.clear();
        self.slots.clear();
        self.pending.clear();
        self.shelf_x = 1;
        self.shelf_y = 1;
        self.shelf_h = 0;
        self.generation += 1;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn shaper() -> Shaper {
        Shaper::new((7.0, 14.0), 2.0)
    }

    #[test]
    fn a_combining_mark_is_drawn_not_dropped() {
        let mut s = shaper();
        let id = s.run("x\u{301}", REGULAR, 1);
        let run = s.run_at(id);
        assert!(run.len >= 2, "base and mark are separate glyphs: {run:?}");
        let (base, mark) = (s.placed(&run, 0), s.placed(&run, 1));
        assert!(
            mark.y < base.y,
            "the mark sits above the base: {} vs {}",
            mark.y,
            base.y
        );
        let plain = s.run("e", REGULAR, 1);
        let accented = s.run("e\u{301}", REGULAR, 1);
        assert_ne!(s.arena[s.run_at(plain).start as usize].slot.w, 0);
        let (p, a) = (s.run_at(plain), s.run_at(accented));
        let ink = |s: &Shaper, r: &Run| {
            (0..r.len as usize)
                .map(|k| s.placed(r, k).slot.h as u32 * s.placed(r, k).slot.w as u32)
                .sum::<u32>()
        };
        assert!(ink(&s, &a) > ink(&s, &p), "e + acute has more ink than e");
        assert!(!run.notdef && !a.notdef);
    }

    #[test]
    fn a_full_atlas_during_a_run_restarts_it_in_the_fresh_atlas() {
        let mut s = shaper();
        for c in 'a'..='z' {
            s.run(&c.to_string(), REGULAR, 1);
        }
        let generation = s.generation;
        s.pending.clear();
        s.shelf_y = ATLAS_SIZE - 2;
        let id = s.run("x\u{301}", REGULAR, 1);
        assert_eq!(s.generation, generation + 1, "the run filled the atlas");
        let run = s.run_at(id);
        assert!(run.len >= 2, "{run:?}");
        assert_eq!(run.start, 0);
        for k in 0..run.len as usize {
            let slot = s.placed(&run, k).slot;
            assert!(
                s.pending
                    .iter()
                    .any(|u| (u.x, u.y) == (slot.x as u32, slot.y as u32)),
                "glyph {k} points at a texel uploaded after the reset"
            );
        }
        assert_eq!(s.run("x\u{301}", REGULAR, 1), id);
    }

    #[test]
    fn the_shaped_cache_reuses_runs_without_new_work() {
        let mut s = shaper();
        let first = s.run("e\u{301}", REGULAR, 1);
        let ascii = s.run("a", REGULAR, 1);
        let blank = s.run(" ", REGULAR, 1);
        assert_eq!(blank, NO_RUN);
        let (shapes, rasters, arena) = (s.shapes, s.rasters, s.arena.len());
        for _ in 0..1000 {
            assert_eq!(s.run("e\u{301}", REGULAR, 1), first);
            assert_eq!(s.run("a", REGULAR, 1), ascii);
            assert_eq!(s.run(" ", REGULAR, 1), blank, "blank cells are cached too");
        }
        assert_eq!(
            (s.shapes, s.rasters, s.arena.len()),
            (shapes, rasters, arena)
        );
        assert_ne!(
            s.run("e\u{301}", BOLD, 1),
            first,
            "style is part of the key"
        );
        assert_ne!(
            s.run("漢", REGULAR, 2),
            s.run("漢", REGULAR, 1),
            "span is part of the key"
        );
    }

    #[test]
    fn a_cache_hit_allocates_nothing() {
        let mut s = shaper();
        s.run("é", REGULAR, 1);
        s.run("漢", REGULAR, 2);
        s.run("a", REGULAR, 1);
        let before = crate::terminal::alloc::count();
        for _ in 0..100 {
            s.run("é", REGULAR, 1);
            s.run("漢", REGULAR, 2);
            s.run("a", REGULAR, 1);
        }
        assert_eq!(crate::terminal::alloc::count(), before);
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn nerd_fonts_are_found_by_family_with_the_original_client_face_first() {
        let installed = [
            "Geist Mono",
            "CaskaydiaCove Nerd Font",
            "CaskaydiaCove Nerd Font Mono",
            "JetBrainsMono Nerd Font Mono",
            "Menlo",
        ];
        assert_eq!(
            nerd_families(installed.into_iter()),
            vec![
                "JetBrainsMono Nerd Font Mono".to_string(),
                "CaskaydiaCove Nerd Font Mono".to_string(),
                "CaskaydiaCove Nerd Font".to_string(),
            ]
        );
        assert!(nerd_families(["Menlo"].into_iter()).is_empty());
    }

    /// Machine-dependent, as the CJK case is: with a Nerd Font installed, a
    /// prompt's branch glyph (U+E0A0) comes from it rather than a box.
    #[test]
    fn a_prompt_icon_comes_from_an_installed_nerd_font() {
        let mut fallback = Fallback::new();
        let installed = nerd_families(fallback.collection.family_names());
        if installed.is_empty() {
            eprintln!("no Nerd Font installed; nothing to find");
            return;
        }
        assert!(fallback.find('\u{e0a0}', false).is_some(), "{installed:?}");
    }

    #[test]
    fn cjk_uses_a_fallback_face_and_fits_its_two_columns() {
        let mut s = shaper();
        let id = s.run("漢", REGULAR, 2);
        let run = s.run_at(id);
        assert!(
            run.face as usize >= PRIMARY_FACES,
            "Geist Mono has no CJK: {run:?}"
        );
        assert!(!run.notdef, "{run:?}");
        let p = s.placed(&run, 0);
        assert!(
            p.x >= 0.0 && p.x + p.slot.w as f32 <= 2.0 * s.cell_w + 0.5,
            "inside the wide span: {} + {}",
            p.x,
            p.slot.w
        );
        assert!(s.fallback_faces() >= 1);
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn emoji_policy_is_a_monochrome_mask_from_the_system_color_font() {
        let mut s = shaper();
        for text in ["😀", "👨\u{200d}👩\u{200d}👧", "❤\u{fe0f}"] {
            let id = s.run(text, REGULAR, 2);
            assert_ne!(id, NO_RUN, "{text:?}");
            let run = s.run_at(id);
            assert!(
                !run.notdef,
                "{text:?} must not be a missing-glyph box: {run:?}"
            );
            assert!(run.face as usize >= PRIMARY_FACES, "{text:?}");
            assert!(
                run.flattened,
                "color emoji are flattened to a mask (policy): {text:?}"
            );
        }
    }

    #[test]
    fn flattening_keeps_dark_features_visible_and_transparent_pixels_clear() {
        let mask = flatten_color(&[255, 220, 0, 255, 0, 0, 0, 255, 255, 255, 255, 0]);
        assert!(mask[0] > 200 && mask[1] >= 60 && mask[1] < 80 && mask[2] == 0);
    }

    #[test]
    fn no_renderer_path_takes_only_the_first_scalar_of_a_cell() {
        let renderer = include_str!("renderer.rs");
        assert!(
            !renderer.contains("chars().next()"),
            "cells draw whole graphemes through Shaper::run"
        );
        assert!(renderer.contains(".run(&cell.text"));
    }
}

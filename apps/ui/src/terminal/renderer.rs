use std::sync::Arc;
use std::time::Instant;

use bytemuck::{Pod, Zeroable};
use domain::{Cell, CellFlags, CursorShape};
use rune::compose::{Compositor, Frame};
use rune::render::EffectSite;

use crate::AppState;
use crate::connection::Metrics;
use crate::terminal::SharedTerminal;
use crate::terminal::alloc;
use crate::terminal::atlas::{Atlas, BOLD, ITALIC, MAX_GLYPHS_PER_CELL, NO_RUN, REGULAR};
use crate::terminal::fixture::is_wide_cluster;
use crate::terminal::input::PREEDIT_MAX_CHARS;
use crate::terminal::links::Link;
use crate::terminal::palette::{Palette, mix};
use crate::terminal::pane::TerminalUi;
use crate::terminal::selection::{Selection, span_on};
use crate::terminal::shaping::is_extender;
use crate::terminal::stats::Shared;
use client::RowDamage;

/// The primary pane's effect mark; the split column's is the next one.
pub const MARK_BASE: u32 = 0x100;
pub const PANES: usize = 2;
/// Rune's isolated-layer target (`render::layers::FORMAT` is crate-private): an
/// effect slot inside a partial-opacity ancestor is drawn in this format.
const LAYER_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Rgba16Float;
/// Underline slots for a ⌘-hovered link: one per row a soft wrap joins.
const LINK_SLOTS: usize = 17;
/// Geist Mono at a 1.35 line height. 8 × 18 pt is 13.3 pt type rather
/// than 13: whole device pixels at 1× and 2×, so the drawn cell and the IME
/// caret rect never round apart.
pub const CELL_PT: (f32, f32) = (8.0, 18.0);

const GLYPHS: usize = MAX_GLYPHS_PER_CELL;
const R_BG: usize = 0;
const R_GLYPH: usize = 1;
const R_UNDER: usize = R_GLYPH + GLYPHS;
const R_STRIKE: usize = R_UNDER + 1;
const REGIONS: usize = R_STRIKE + 1;
const DIM_ALPHA: u32 = 150;

#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
pub struct Inst {
    xy: [f32; 2],
    wh: [f32; 2],
    uv: [u16; 4],
    color: u32,
    pad: u32,
}

#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
struct Params {
    origin: [f32; 2],
    frame: [f32; 2],
    atlas: [f32; 2],
    srgb_blend: f32,
    pad: f32,
}

const NONE: Inst = Inst {
    xy: [0.0; 2],
    wh: [0.0; 2],
    uv: [0; 4],
    color: 0,
    pad: 0,
};

enum Uploaded {
    Unset,
    Blank,
    Row(Arc<Vec<Cell>>),
}

struct PaneGpu {
    buffer: wgpu::Buffer,
    insts: Vec<Inst>,
    uploaded: Vec<Uploaded>,
    cols: usize,
    rows: usize,
    uploaded_scroll: Option<usize>,
    extras_key: u64,
}

impl PaneGpu {
    fn cells(&self) -> usize {
        self.cols * self.rows
    }

    fn extras_at(&self) -> usize {
        self.cells() * REGIONS
    }

    fn total(cols: usize, rows: usize) -> usize {
        cols * rows * REGIONS + extras_len(rows)
    }
}

fn extras_len(rows: usize) -> usize {
    rows + 1 + PREEDIT_MAX_CHARS * (2 + GLYPHS) + LINK_SLOTS
}

struct Gpu {
    key: rune::compose::DeviceKey,
    layout: wgpu::PipelineLayout,
    module: wgpu::ShaderModule,
    pipelines: Vec<(wgpu::TextureFormat, wgpu::RenderPipeline)>,
    atlas: Atlas,
    bind: wgpu::BindGroup,
    cell_pt: (f32, f32),
    panes: [Option<PaneGpu>; PANES],
}

pub struct Terminal {
    gpu: Option<Gpu>,
    palette: Palette,
    /// The theme base `palette` was built from; a change repaints every row.
    palette_base: String,
    stats: Shared,
    metrics: Option<Arc<Metrics>>,
    shared: Arc<SharedTerminal>,
    last_atlas_gen: u64,
}

impl Terminal {
    pub fn new(shared: Arc<SharedTerminal>, stats: Shared, metrics: Option<Arc<Metrics>>) -> Self {
        Terminal {
            gpu: None,
            palette: Palette::for_base(crate::theme::DEFAULT_BASE),
            palette_base: crate::theme::DEFAULT_BASE.to_string(),
            stats,
            metrics,
            shared,
            last_atlas_gen: 0,
        }
    }
}

fn make_gpu(
    device: &wgpu::Device,
    key: rune::compose::DeviceKey,
    scale: f32,
    cell_pt: (f32, f32),
) -> Gpu {
    let bgl = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
        label: Some("terminal"),
        entries: &[wgpu::BindGroupLayoutEntry {
            binding: 0,
            visibility: wgpu::ShaderStages::FRAGMENT,
            ty: wgpu::BindingType::Texture {
                sample_type: wgpu::TextureSampleType::Float { filterable: false },
                view_dimension: wgpu::TextureViewDimension::D2,
                multisampled: false,
            },
            count: None,
        }],
    });
    let layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
        label: Some("terminal"),
        bind_group_layouts: &[Some(&bgl)],
        immediate_size: size_of::<Params>() as u32,
    });
    let module = device.create_shader_module(wgpu::ShaderModuleDescriptor {
        label: Some("terminal"),
        source: wgpu::ShaderSource::Wgsl(include_str!("shader.wgsl").into()),
    });
    let atlas = Atlas::new(device, cell_pt, scale);
    let bind = device.create_bind_group(&wgpu::BindGroupDescriptor {
        label: Some("terminal"),
        layout: &bgl,
        entries: &[wgpu::BindGroupEntry {
            binding: 0,
            resource: wgpu::BindingResource::TextureView(&atlas.view),
        }],
    });
    Gpu {
        key,
        layout,
        module,
        pipelines: Vec::new(),
        atlas,
        bind,
        cell_pt,
        panes: [None, None],
    }
}

fn make_pane_gpu(device: &wgpu::Device, cols: usize, rows: usize) -> PaneGpu {
    let total = PaneGpu::total(cols, rows);
    PaneGpu {
        buffer: device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("terminal instances"),
            size: (total * size_of::<Inst>()) as u64,
            usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        }),
        insts: vec![NONE; total],
        uploaded: (0..rows).map(|_| Uploaded::Unset).collect(),
        cols,
        rows,
        uploaded_scroll: None,
        extras_key: u64::MAX,
    }
}

/// A fresh atlas has no rows from the old one, so every row and the extras
/// must be encoded again before the next draw.
fn reset_pane_uploads(pg: &mut PaneGpu) {
    pg.uploaded.iter_mut().for_each(|u| *u = Uploaded::Unset);
    pg.extras_key = u64::MAX;
}

/// The atlas holds glyphs rasterized at the window's scale only. A slot an
/// ancestor scales (an enter animation) would stretch them, so it draws
/// nothing until that scale returns to 1; the atlas is never rebuilt for it.
fn scale_matches(site: f32, atlas: f32) -> bool {
    (site - atlas).abs() <= 1e-4
}

fn pipeline(gpu: &mut Gpu, device: &wgpu::Device, format: wgpu::TextureFormat) -> usize {
    if let Some(i) = gpu.pipelines.iter().position(|(f, _)| *f == format) {
        return i;
    }
    let attrs = [
        wgpu::VertexAttribute {
            format: wgpu::VertexFormat::Float32x2,
            offset: 0,
            shader_location: 0,
        },
        wgpu::VertexAttribute {
            format: wgpu::VertexFormat::Float32x2,
            offset: 8,
            shader_location: 1,
        },
        wgpu::VertexAttribute {
            format: wgpu::VertexFormat::Uint16x4,
            offset: 16,
            shader_location: 2,
        },
        wgpu::VertexAttribute {
            format: wgpu::VertexFormat::Uint32,
            offset: 24,
            shader_location: 3,
        },
    ];
    let pipe = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
        label: Some("terminal"),
        layout: Some(&gpu.layout),
        vertex: wgpu::VertexState {
            module: &gpu.module,
            entry_point: Some("vs"),
            compilation_options: Default::default(),
            buffers: &[Some(wgpu::VertexBufferLayout {
                array_stride: size_of::<Inst>() as u64,
                step_mode: wgpu::VertexStepMode::Instance,
                attributes: &attrs,
            })],
        },
        primitive: wgpu::PrimitiveState {
            topology: wgpu::PrimitiveTopology::TriangleStrip,
            ..Default::default()
        },
        depth_stencil: None,
        multisample: Default::default(),
        fragment: Some(wgpu::FragmentState {
            module: &gpu.module,
            entry_point: Some("fs"),
            compilation_options: Default::default(),
            targets: &[Some(wgpu::ColorTargetState {
                format,
                blend: Some(wgpu::BlendState::PREMULTIPLIED_ALPHA_BLENDING),
                write_mask: wgpu::ColorWrites::ALL,
            })],
        }),
        multiview_mask: None,
        cache: None,
    });
    gpu.pipelines.push((format, pipe));
    gpu.pipelines.len() - 1
}

fn solid(x: f32, y: f32, w: f32, h: f32, color: u32) -> Inst {
    Inst {
        xy: [x, y],
        wh: [w, h],
        uv: [0; 4],
        color,
        pad: 0,
    }
}

/// Intersects a glyph quad with `clip` (x0, y0, x1, y1), trimming its atlas rectangle with it.
fn clipped(xy: [f32; 2], wh: [f32; 2], uv: [u16; 4], clip: [f32; 4], color: u32) -> Inst {
    let (x0, y0) = (xy[0].max(clip[0]), xy[1].max(clip[1]));
    let (x1, y1) = ((xy[0] + wh[0]).min(clip[2]), (xy[1] + wh[1]).min(clip[3]));
    if x1 <= x0 || y1 <= y0 {
        return NONE;
    }
    let (dx, dy) = ((x0 - xy[0]) as u16, (y0 - xy[1]) as u16);
    Inst {
        xy: [x0, y0],
        wh: [x1 - x0, y1 - y0],
        uv: [uv[0] + dx, uv[1] + dy, (x1 - x0) as u16, (y1 - y0) as u16],
        color,
        pad: 0,
    }
}

/// Writes the glyphs of a shaped run for the cell at (x, y) into `out`, clipped to the cell's column span and row.
fn put_run(atlas: &Atlas, id: u32, cell: [f32; 4], color: u32, out: &mut [Inst]) {
    out.fill(NONE);
    if id == NO_RUN {
        return;
    }
    let run = atlas.run_at(id);
    let clip = [cell[0], cell[1], cell[0] + cell[2], cell[1] + cell[3]];
    for (k, slot) in out.iter_mut().enumerate().take(run.len as usize) {
        let p = atlas.placed(&run, k);
        let s = p.slot;
        *slot = clipped(
            [cell[0] + p.x, cell[1] + atlas.baseline + p.y],
            [s.w as f32, s.h as f32],
            [s.x, s.y, s.w, s.h],
            clip,
            color,
        );
    }
}

/// Fixed slots per cell (background, up to three glyphs, underline, strike-through); a hit in the shaped-run cache allocates nothing.
fn encode_row(
    atlas: &mut Atlas,
    pal: &Palette,
    cells: &[Cell],
    r: usize,
    dims: (usize, usize),
    insts: &mut [Inst],
) {
    let (cols, total_cells) = dims;
    let (cw, ch, base, scale) = (atlas.cell_w, atlas.cell_h, atlas.baseline, atlas.scale);
    let y = r as f32 * ch;
    let line = scale.max(1.0);
    for c in 0..cols {
        let i = r * cols + c;
        let Some(cell) = cells.get(c) else {
            for region in 0..REGIONS {
                insts[region * total_cells + i] = NONE;
            }
            continue;
        };
        let x = c as f32 * cw;
        let span = if cell.flags.contains(CellFlags::WIDE_CHAR) {
            2
        } else {
            1
        };
        let w = span as f32 * cw;
        let mut fg = pal.resolve(cell.fg, pal.fg);
        let mut bg = pal.resolve(cell.bg, pal.bg);
        if cell.flags.contains(CellFlags::INVERSE) {
            std::mem::swap(&mut fg, &mut bg);
        }
        if cell.flags.contains(CellFlags::DIM) {
            fg = mix(fg, bg, DIM_ALPHA);
        }
        insts[R_BG * total_cells + i] = if bg == pal.bg {
            NONE
        } else {
            solid(x, y, w, ch, bg)
        };
        let spacer = cell.flags.contains(CellFlags::WIDE_SPACER);
        let draw_text = !spacer && !cell.flags.contains(CellFlags::HIDDEN);
        let id = if draw_text {
            let face = if cell.flags.contains(CellFlags::BOLD) {
                BOLD
            } else if cell.flags.contains(CellFlags::ITALIC) {
                ITALIC
            } else {
                REGULAR
            };
            atlas.run(&cell.text, face, span)
        } else {
            NO_RUN
        };
        let mut glyphs = [NONE; GLYPHS];
        put_run(atlas, id, [x, y, w, ch], fg, &mut glyphs);
        for (k, g) in glyphs.iter().enumerate() {
            insts[(R_GLYPH + k) * total_cells + i] = *g;
        }
        insts[R_UNDER * total_cells + i] = if cell.flags.contains(CellFlags::UNDERLINE) && !spacer {
            solid(x, y + base + line * 1.5, w, line, fg)
        } else {
            NONE
        };
        insts[R_STRIKE * total_cells + i] = if cell.flags.contains(CellFlags::STRIKEOUT) && !spacer
        {
            solid(x, y + (ch * 0.5).round(), w, line, fg)
        } else {
            NONE
        };
    }
}

fn fnv(bytes: &[u8]) -> u64 {
    bytes.iter().fold(0xcbf2_9ce4_8422_2325, |h, &b| {
        (h ^ b as u64).wrapping_mul(0x100_0000_01b3)
    })
}

/// What a pane draws over its cells, read from its `TerminalUi`.
#[derive(Clone, Copy)]
struct Overlay<'a> {
    scroll: usize,
    selection: Option<Selection>,
    preedit: &'a str,
    cursor_on: bool,
    link: Option<&'a Link>,
}

impl<'a> Overlay<'a> {
    fn of(ui: &'a TerminalUi) -> Self {
        Overlay {
            scroll: ui.scroll,
            selection: ui.selection,
            preedit: &ui.preedit,
            cursor_on: ui.cursor_on,
            link: ui.link.as_ref(),
        }
    }
}

fn extras_key(grid: &client::CellGrid, o: Overlay<'_>, scale: f32) -> u64 {
    let scroll = o.scroll;
    let viewport = crate::terminal::pane::Viewport { grid, scroll };
    let sel = o.selection.map(|s| s.range(&viewport));
    let mut h = fnv(o.preedit.as_bytes());
    let mix64 = |h: u64, v: u64| (h ^ v).wrapping_mul(0x9e37_79b9_7f4a_7c15).rotate_left(23);
    if let Some(((c0, l0), (c1, l1))) = sel {
        for v in [c0 as u64, l0 as u64, c1 as u64, l1 as u64, 1] {
            h = mix64(h, v);
        }
    }
    if let Some(link) = o.link {
        for &(line, from, to) in &link.spans {
            for v in [line as u64, from as u64, to as u64, 2] {
                h = mix64(h, v);
            }
        }
    }
    let cursor_on = o.cursor_on;
    let cur = grid.cursor;
    for v in [
        cur.col as u64,
        cur.line as u64,
        cur.visible as u64,
        cur.shape as u64,
        cursor_on as u64,
        scroll as u64,
        viewport.top_abs() as u64,
        scale.to_bits() as u64,
    ] {
        h = mix64(h, v);
    }
    h
}

fn encode_extras(
    atlas: &mut Atlas,
    grid: &client::CellGrid,
    o: Overlay<'_>,
    palette: &Palette,
    out: &mut [Inst],
) {
    let Overlay {
        scroll,
        selection,
        preedit,
        cursor_on,
        link,
    } = o;
    let (cw, ch, scale) = (atlas.cell_w, atlas.cell_h, atlas.scale);
    let viewport = crate::terminal::pane::Viewport { grid, scroll };
    let (cols, rows) = (viewport.cols(), viewport.rows());
    out.fill(NONE);
    if let Some(sel) = selection {
        let range = sel.range(&viewport);
        let top = viewport.top_abs();
        for (v, slot) in out.iter_mut().enumerate().take(rows) {
            if let Some((from, to)) = span_on(range, top + v as i64, cols) {
                *slot = solid(
                    from as f32 * cw,
                    v as f32 * ch,
                    (to - from) as f32 * cw,
                    ch,
                    palette.selection,
                );
            }
        }
    }
    let cur = grid.cursor;
    let (cc, cl) = (cur.col as usize, cur.line as usize);
    if scroll == 0 && cur.visible && cur.shape != CursorShape::Hidden && cursor_on {
        let wide = grid
            .visible
            .get(cl)
            .and_then(|row| row.cells.get(cc))
            .is_some_and(|cell| cell.flags.contains(domain::CellFlags::WIDE_CHAR));
        let [x, y, w, h] = caret_rect(cc, cl, wide, (cw, ch), cur.shape, scale);
        out[rows] = solid(x, y, w, h, palette.cursor);
    }
    if !preedit.is_empty() && scroll == 0 {
        let (bg, rest) = out[rows + 1..].split_at_mut(PREEDIT_MAX_CHARS);
        let (gl, under) = rest.split_at_mut(PREEDIT_MAX_CHARS * GLYPHS);
        let mut starts = preedit
            .char_indices()
            .filter(|&(i, c)| i == 0 || !is_extender(c))
            .map(|(i, _)| i)
            .chain(std::iter::once(preedit.len()))
            .peekable();
        let (mut col, mut k) = (cc, 0);
        while let Some(from) = starts.next() {
            let Some(&to) = starts.peek() else { break };
            let cluster = &preedit[from..to];
            let span = if is_wide_cluster(cluster) { 2 } else { 1 };
            if col + span > cols || k >= PREEDIT_MAX_CHARS {
                break;
            }
            let (x, y) = (col as f32 * cw, cl as f32 * ch);
            bg[k] = solid(x, y, span as f32 * cw, ch, palette.preedit_bg);
            let id = atlas.run(cluster, REGULAR, span);
            put_run(
                atlas,
                id,
                [x, y, span as f32 * cw, ch],
                palette.preedit_fg,
                &mut gl[k * GLYPHS..(k + 1) * GLYPHS],
            );
            under[k] = solid(
                x,
                y + ch - scale.max(1.0) * 1.5,
                span as f32 * cw,
                scale.max(1.0),
                palette.preedit_fg,
            );
            col += span;
            k += 1;
        }
    }
    if let Some(link) = link {
        let at = out.len() - LINK_SLOTS;
        let top = viewport.top_abs();
        let line = scale.max(1.0);
        let spans = link
            .spans
            .iter()
            .filter(|(abs, ..)| *abs >= top && *abs < top + rows as i64);
        for (slot, &(abs, from, to)) in out[at..].iter_mut().zip(spans) {
            let y = (abs - top) as f32 * ch;
            *slot = solid(
                from as f32 * cw,
                y + atlas.baseline + line * 1.5,
                (to - from + 1) as f32 * cw,
                line,
                palette.fg,
            );
        }
    }
}

/// The caret's rectangle in device pixels: a beam
/// or underline is one logical pixel snapped onto the device grid, so a
/// fractional cell width never blurs it; a block covers a wide cell whole;
/// an underline stays inside its cell.
#[must_use]
pub fn caret_rect(
    col: usize,
    line: usize,
    wide: bool,
    (cw, ch): (f32, f32),
    shape: CursorShape,
    scale: f32,
) -> [f32; 4] {
    let stroke = scale.round().max(1.0);
    let x = (col as f32 * cw).round();
    let y = line as f32 * ch;
    match shape {
        CursorShape::Beam => [x, y, stroke, ch],
        CursorShape::Underline => [x, (y + ch - stroke).round(), cw, stroke],
        _ => [col as f32 * cw, y, if wide { cw * 2.0 } else { cw }, ch],
    }
}

fn row_state<'a>(
    grid: &'a client::CellGrid,
    scroll: usize,
    v: usize,
    up: &Uploaded,
) -> (bool, Option<&'a Arc<Vec<Cell>>>) {
    let base = grid.scrollback_len as i64;
    let abs = base - scroll as i64 + v as i64;
    let row = if abs < 0 {
        None
    } else if abs >= base {
        grid.visible.get((abs - base) as usize)
    } else {
        grid.scrollback_row(abs - base)
    };
    let clean = match (row, up) {
        (Some(r), Uploaded::Row(prev)) => Arc::ptr_eq(&r.cells, prev),
        (None, Uploaded::Blank) => true,
        _ => false,
    };
    (clean, row.map(|r| &r.cells))
}

#[derive(Default)]
struct Encoded {
    work: Option<(usize, usize)>,
    extras_dirty: bool,
    rows_uploaded: u64,
    allocs: u64,
}

/// Diffs one pane's replica against what its buffer holds and re-encodes
/// only the rows that changed, as the single-pane path always did.
fn encode_pane(
    gpu: &mut Gpu,
    idx: usize,
    device: &wgpu::Device,
    palette: &Palette,
    shared: &SharedTerminal,
    o: Overlay<'_>,
    repaint: bool,
) -> Encoded {
    let mut done = Encoded::default();
    let scale = gpu.atlas.scale;
    let mut slot = shared.lock();
    let damage = std::mem::take(&mut slot.damage);
    let Some(grid) = slot.grid.as_ref() else {
        gpu.panes[idx] = None;
        return done;
    };
    let (cols, rows) = (
        grid.visible.first().map_or(0, |row| row.cells.len()),
        grid.visible.len(),
    );
    if cols == 0 || rows == 0 {
        return done;
    }
    if gpu.panes[idx]
        .as_ref()
        .is_none_or(|pane| pane.cols != cols || pane.rows != rows)
    {
        gpu.panes[idx] = Some(make_pane_gpu(device, cols, rows));
    }
    let Some(pg) = gpu.panes[idx].as_mut() else {
        return done;
    };
    let scroll = o.scroll;
    let total_cells = pg.cells();
    let full = repaint || matches!(damage, RowDamage::Full) || pg.uploaded_scroll != Some(scroll);
    let damaged = damage.rows();
    let e0 = alloc::count();
    let mut run: Option<usize> = None;
    let mut work: Option<(usize, usize)> = None;
    for v in 0..=rows {
        if v == rows {
            if let Some(start) = run.take() {
                work = Some(match work {
                    Some((a, b)) => (a.min(start), b.max(rows)),
                    None => (start, rows),
                });
            }
            break;
        }
        let forced = full || damaged.is_some_and(|rows| rows.contains(&(v as u16)));
        let (clean, cells) = row_state(grid, scroll, v, &pg.uploaded[v]);
        if forced || !clean {
            match cells {
                Some(arc) => {
                    encode_row(
                        &mut gpu.atlas,
                        palette,
                        arc,
                        v,
                        (cols, total_cells),
                        &mut pg.insts,
                    );
                    pg.uploaded[v] = Uploaded::Row(Arc::clone(arc));
                }
                None => {
                    encode_row(
                        &mut gpu.atlas,
                        palette,
                        &[],
                        v,
                        (cols, total_cells),
                        &mut pg.insts,
                    );
                    pg.uploaded[v] = Uploaded::Blank;
                }
            }
            done.rows_uploaded += 1;
            run.get_or_insert(v);
        } else if let Some(start) = run.take() {
            work = Some(match work {
                Some((a, b)) => (a.min(start), b.max(v)),
                None => (start, v),
            });
        }
    }
    let key = extras_key(grid, o, scale);
    if repaint || key != pg.extras_key {
        let at = pg.extras_at();
        encode_extras(&mut gpu.atlas, grid, o, palette, &mut pg.insts[at..]);
        pg.extras_key = key;
        done.extras_dirty = true;
    }
    pg.uploaded_scroll = Some(scroll);
    done.work = work;
    done.allocs = alloc::count() - e0;
    done
}

impl Compositor<AppState> for Terminal {
    fn features(&self) -> wgpu::Features {
        wgpu::Features::IMMEDIATES
    }

    fn animating(&self) -> bool {
        self.shared.animating() || self.shared.beside().is_some_and(|split| split.animating())
    }

    fn render(&mut self, f: &mut Frame<'_, AppState>) {
        let epochs = [
            Some(self.shared.render_epoch()),
            self.shared.beside().map(|split| split.render_epoch()),
        ];
        let t0 = Instant::now();
        let a0 = alloc::count();
        let mut encode_allocs = 0u64;
        let (device, queue) = (f.device, f.queue);
        let scale = f.rt.env.scale;
        let cell_pt = f.rt.state.cell_pt();
        let key = f.device_key();
        let stale = match &self.gpu {
            Some(g) => g.key != key || g.atlas.scale != scale || g.cell_pt != cell_pt,
            None => true,
        };
        if stale {
            self.gpu = Some(make_gpu(device, key, scale, cell_pt));
            self.last_atlas_gen = 0;
        }
        let shared = Arc::clone(&self.shared);
        let split = shared.beside().cloned();
        let Some(gpu) = self.gpu.as_mut() else {
            return;
        };
        let misses0 = gpu.atlas.misses;

        // Compared as `&str`, so a frame whose theme did not change allocates nothing.
        let base = f.rt.state.theme_base(f.rt.env.system_dark);
        let repaint = base != self.palette_base;
        if repaint {
            self.palette = Palette::for_base(base);
            self.palette_base = base.to_string();
        }
        let mut rows_uploaded = 0u64;
        let mut encoded: [Encoded; PANES] = Default::default();
        let state = &f.rt.state;
        for (idx, done) in encoded.iter_mut().enumerate().take(state.pane_count()) {
            let replica = match (idx, &split) {
                (0, _) => &*shared,
                (_, Some(split)) => &**split,
                _ => continue,
            };
            let overlay = Overlay::of(state.pane(idx));
            *done = encode_pane(gpu, idx, device, &self.palette, replica, overlay, repaint);
            rows_uploaded += done.rows_uploaded;
            encode_allocs += done.allocs;
        }

        gpu.atlas.flush(queue);
        for (done, pg) in encoded.iter().zip(&gpu.panes) {
            let Some(pg) = pg else {
                continue;
            };
            if let Some((start, end)) = done.work {
                let total_cells = pg.cells();
                for region in 0..REGIONS {
                    let (a, b) = (
                        region * total_cells + start * pg.cols,
                        region * total_cells + end * pg.cols,
                    );
                    f.write_buffer(
                        &pg.buffer,
                        (a * size_of::<Inst>()) as u64,
                        bytemuck::cast_slice(&pg.insts[a..b]),
                    );
                }
            }
            if done.extras_dirty {
                let at = pg.extras_at();
                f.write_buffer(
                    &pg.buffer,
                    (at * size_of::<Inst>()) as u64,
                    bytemuck::cast_slice(&pg.insts[at..]),
                );
            }
        }
        if gpu.atlas.generation != self.last_atlas_gen {
            self.last_atlas_gen = gpu.atlas.generation;
            for pg in gpu.panes.iter_mut().flatten() {
                reset_pane_uploads(pg);
            }
            shared.request_redraw();
            if let Some(split) = &split {
                split.request_redraw();
            }
        }
        pipeline(gpu, device, f.format);
        pipeline(gpu, device, LAYER_FORMAT);
        let new_glyphs = gpu.atlas.misses != misses0;
        let a1 = alloc::count();
        let t1 = Instant::now();

        let clear = f.rt.clear_color();
        f.ui.prepare(device, queue, &mut f.rt.text.atlas, &f.rt.instances, f.size);
        let mut draws = 0u64;
        let mut drawn_instances = 0u64;
        let mut hook_allocs = 0u64;
        let gpu = &*gpu;
        let atlas_scale = gpu.atlas.scale;
        let cell_w = gpu.atlas.cell_w;
        let cell_h = gpu.atlas.cell_h;
        f.ui.draw_with(
            f.encoder,
            f.target,
            clear,
            &mut |pass: &mut wgpu::RenderPass<'_>, s: &EffectSite<'_>| {
                let idx = s.mark.wrapping_sub(MARK_BASE) as usize;
                let replica = match (idx, &split) {
                    (0, _) => &*shared,
                    (1, Some(split)) => &**split,
                    _ => return,
                };
                if !scale_matches(s.scale, atlas_scale) {
                    return;
                }
                let r = s.instance.rect;
                if let Some((cols, rows)) = grid_cells(r[2], r[3], cell_w, cell_h)
                    && replica.set_measured(cols, rows)
                {
                    replica.request_redraw();
                }
                let Some(pg) = gpu.panes[idx].as_ref() else {
                    return;
                };
                let Some(pipe) = gpu
                    .pipelines
                    .iter()
                    .find(|(format, _)| *format == s.format)
                    .map(|(_, pipe)| pipe)
                else {
                    return;
                };
                let h0 = alloc::count();
                let r = s.instance.rect;
                let params = Params {
                    origin: [(r[0] - s.origin[0]).round(), (r[1] - s.origin[1]).round()],
                    frame: [s.size[0] as f32, s.size[1] as f32],
                    atlas: [crate::terminal::atlas::ATLAS_SIZE as f32; 2],
                    srgb_blend: if s.srgb_blend { 1.0 } else { 0.0 },
                    pad: 0.0,
                };
                let count = pg.insts.len() as u32;
                pass.set_pipeline(pipe);
                pass.set_bind_group(0, &gpu.bind, &[]);
                pass.set_vertex_buffer(0, pg.buffer.slice(..));
                pass.set_immediates(0, bytemuck::bytes_of(&params));
                pass.draw(0..4, 0..count);
                draws += 1;
                drawn_instances += count as u64;
                hook_allocs += alloc::count() - h0;
            },
        );
        let t2 = Instant::now();

        for (replica, epoch) in [Some(&shared), split.as_ref()].into_iter().zip(epochs) {
            if let (Some(replica), Some(epoch)) = (replica, epoch) {
                replica.mark_rendered_to(epoch);
            }
        }
        let us = |a: Instant, b: Instant| b.duration_since(a).as_secs_f64() * 1e6;
        let mut st = self
            .stats
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        st.frames += 1;
        st.draw_calls += draws;
        st.instances_drawn += drawn_instances;
        st.rows_uploaded += rows_uploaded;
        st.glyph_misses = gpu.atlas.misses;
        st.shapes = gpu.atlas.shapes;
        st.rasters = gpu.atlas.rasters;
        st.notdef_runs = gpu.atlas.notdef_runs;
        st.fallback_faces = gpu.atlas.fallback_faces();
        st.shape_us = gpu.atlas.shape_us;
        st.shape_us_max = gpu.atlas.shape_us_max;
        st.fallback_us = gpu.atlas.fallback_us;
        let build_allocs = a1 - a0;
        if st.frames > 2 {
            st.build_allocs += build_allocs;
            st.encode_allocs += encode_allocs;
            st.steady_encode_allocs += if new_glyphs { 0 } else { encode_allocs };
            st.build_allocs_max = st.build_allocs_max.max(build_allocs);
            st.hook_allocs += hook_allocs;
            if new_glyphs {
                st.frames_with_misses += 1;
            } else {
                st.steady_build_allocs += build_allocs;
            }
            let (build, encode, update) = (us(t0, t1), us(t1, t2), f.update_ms * 1000.0);
            st.update_us.push(update);
            st.build_us.push(build);
            st.encode_us.push(encode);
            st.total_us.push(update + build + encode);
            if st.frames.is_multiple_of(60)
                && let Some(p95) = st.p95_us()
                && let Some(metrics) = &self.metrics
            {
                metrics.note_frame_p95(p95);
            }
        }
    }

    fn summary(&mut self, _device: &wgpu::Device) -> Option<String> {
        Some(
            self.stats
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .report(),
        )
    }
}

impl Drop for Terminal {
    fn drop(&mut self) {
        if let Ok(mut st) = self.stats.lock()
            && st.frames > 0
        {
            eprintln!("{}", st.report());
        }
    }
}

/// Whole cells a pane of `width` × `height` holds, floored. A pane with no
/// whole cell keeps the last measure (`None`) rather than 1 × 1: resizing the
/// PTY to one cell would reflow it.
#[must_use]
pub fn grid_cells(width: f32, height: f32, cell_w: f32, cell_h: f32) -> Option<(u16, u16)> {
    let whole = |length: f32, cell: f32| {
        if cell > 0.0 {
            (length / cell).floor().clamp(0.0, f32::from(u16::MAX)) as u16
        } else {
            0
        }
    };
    let (cols, rows) = (whole(width, cell_w), whole(height, cell_h));
    (cols > 0 && rows > 0).then_some((cols, rows))
}

#[cfg(test)]
mod upload_tests {
    use super::*;

    #[test]
    fn a_new_atlas_generation_reuploads_every_row_and_the_extras() {
        let Ok(gpu) = rune::render::Gpu::try_with_features(
            wgpu::Instance::default(),
            None,
            wgpu::Features::empty(),
        ) else {
            return;
        };
        let mut pane = make_pane_gpu(&gpu.device, 4, 2);
        pane.uploaded[0] = Uploaded::Blank;
        pane.extras_key = 7;
        reset_pane_uploads(&mut pane);
        assert!(pane.uploaded.iter().all(|u| matches!(u, Uploaded::Unset)));
        assert_eq!(pane.extras_key, u64::MAX);
    }

    #[test]
    fn a_slot_at_another_scale_draws_nothing() {
        assert!(scale_matches(2.0, 2.0));
        assert!(scale_matches(2.0 + 1e-5, 2.0));
        assert!(!scale_matches(1.5, 2.0));
    }
}

#[cfg(test)]
mod grid_tests {
    #[test]
    fn the_caret_is_snapped_and_sized_as_caret_rect_says() {
        use domain::CursorShape;
        let cell = (7.8_f32, 16.0_f32);
        let beam = |col, scale: f32| {
            super::caret_rect(
                col,
                0,
                false,
                (cell.0 * scale, cell.1 * scale),
                CursorShape::Beam,
                scale,
            )
        };
        assert_eq!(beam(0, 1.0)[2], 1.0, "one device pixel at scale 1");
        assert_eq!(beam(0, 2.0)[2], 2.0, "two at scale 2");
        let x = beam(1, 2.0)[0];
        assert_eq!(x, (7.8_f32 * 2.0).round(), "snapped onto the device grid");
        assert_eq!(x.fract(), 0.0);
        let narrow = super::caret_rect(0, 0, false, cell, CursorShape::Block, 1.0);
        let wide = super::caret_rect(0, 0, true, cell, CursorShape::Block, 1.0);
        assert_eq!(narrow[2], cell.0);
        assert_eq!(wide[2], cell.0 * 2.0, "a block covers a wide cell");
        assert_eq!(wide[3], cell.1);
        let device = (cell.0 * 2.0, cell.1 * 2.0);
        let under = super::caret_rect(3, 2, false, device, CursorShape::Underline, 2.0);
        let (top, bottom) = (2.0 * device.1, 3.0 * device.1);
        assert!(
            under[1] >= top && under[1] + under[3] <= bottom,
            "inside its cell"
        );
        assert_eq!(under[3], 2.0);
    }

    #[test]
    fn a_pane_floors_to_whole_cells_as_grid_size_does() {
        assert_eq!(super::grid_cells(80.0, 180.0, 8.0, 18.0), Some((10, 10)));
        assert_eq!(super::grid_cells(87.9, 197.0, 8.0, 18.0), Some((10, 10)));
        assert_eq!(
            super::grid_cells(0.0, 0.0, 8.0, 18.0),
            None,
            "keeps the last measure"
        );
        assert_eq!(super::grid_cells(-40.0, 100.0, 8.0, 18.0), None);
        assert_eq!(
            super::grid_cells(80.0, 180.0, 0.0, 18.0),
            None,
            "no cell, no grid"
        );
    }
}

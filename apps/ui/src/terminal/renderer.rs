use std::sync::Arc;
use std::time::Instant;

use bytemuck::{Pod, Zeroable};
use domain::{Cell, CellFlags, Cursor, CursorShape};
use rune::compose::{Compositor, Frame};
use rune::render::EffectSite;

use crate::AppState;
use crate::terminal::SharedTerminal;
use crate::terminal::alloc;
use crate::terminal::atlas::{Atlas, BOLD, ITALIC, MAX_GLYPHS_PER_CELL, NO_RUN, REGULAR};
use crate::terminal::fixture::is_wide_cluster;
use crate::terminal::input::PREEDIT_MAX_CHARS;
use crate::terminal::palette::{Palette, mix, rgb, with_alpha};
use crate::terminal::pane::TerminalUi;
use crate::terminal::selection::{Selection, span_on};
use crate::terminal::shaping::is_extender;
use crate::terminal::stats::Shared;
use client::RowDamage;

pub const MARK_BASE: u32 = 0x100;
pub const CELL_PT: (f32, f32) = (7.0, 14.0);

const GLYPHS: usize = MAX_GLYPHS_PER_CELL;
const R_BG: usize = 0;
const R_GLYPH: usize = 1;
const R_UNDER: usize = R_GLYPH + GLYPHS;
const R_STRIKE: usize = R_UNDER + 1;
const REGIONS: usize = R_STRIKE + 1;
const SELECTION: u32 = 0x66f6_823b;
const CURSOR: u32 = 0xaad4_d4d4;
const PREEDIT_BG: u32 = rgb(0x2c, 0x34, 0x44);
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
    rows + 1 + PREEDIT_MAX_CHARS * (2 + GLYPHS)
}

struct Gpu {
    key: rune::compose::DeviceKey,
    layout: wgpu::PipelineLayout,
    module: wgpu::ShaderModule,
    pipelines: Vec<(wgpu::TextureFormat, wgpu::RenderPipeline)>,
    atlas: Atlas,
    bind: wgpu::BindGroup,
    pane: Option<PaneGpu>,
}

pub struct Terminal {
    gpu: Option<Gpu>,
    palette: Palette,
    stats: Shared,
    shared: Arc<SharedTerminal>,
    last_atlas_gen: u64,
}

impl Terminal {
    pub fn new(shared: Arc<SharedTerminal>, stats: Shared) -> Self {
        Terminal {
            gpu: None,
            palette: Palette::forge_dark(),
            stats,
            shared,
            last_atlas_gen: 0,
        }
    }
}

fn make_gpu(device: &wgpu::Device, key: rune::compose::DeviceKey, scale: f32) -> Gpu {
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
    let atlas = Atlas::new(device, CELL_PT, scale);
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
        pane: None,
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

#[allow(clippy::too_many_arguments)]
fn extras_key(
    grid: &client::CellGrid,
    scroll: usize,
    selection: Option<Selection>,
    preedit: &str,
    cursor_on: bool,
    scale: f32,
) -> u64 {
    let viewport = crate::terminal::pane::Viewport { grid, scroll };
    let sel = selection.map(|s| s.range(&viewport));
    let mut h = fnv(preedit.as_bytes());
    let mix64 = |h: u64, v: u64| (h ^ v).wrapping_mul(0x9e37_79b9_7f4a_7c15).rotate_left(23);
    if let Some(((c0, l0), (c1, l1))) = sel {
        for v in [c0 as u64, l0 as u64, c1 as u64, l1 as u64, 1] {
            h = mix64(h, v);
        }
    }
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
    scroll: usize,
    selection: Option<Selection>,
    preedit: &str,
    cursor_on: bool,
    out: &mut [Inst],
) {
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
                    SELECTION,
                );
            }
        }
    }
    let cur = grid.cursor;
    let (cc, cl) = (cur.col as usize, cur.line as usize);
    if scroll == 0 && cur.visible && cur.shape != CursorShape::Hidden && cursor_on {
        let (x, y) = (cc as f32 * cw, cl as f32 * ch);
        let line = (2.0_f32 * scale).max(1.0);
        out[rows] = match cur.shape {
            CursorShape::Underline => solid(x, y + ch - line, cw, line, CURSOR),
            CursorShape::Beam => solid(x, y, line, ch, CURSOR),
            _ => solid(x, y, cw, ch, CURSOR),
        };
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
            bg[k] = solid(x, y, span as f32 * cw, ch, PREEDIT_BG);
            let id = atlas.run(cluster, REGULAR, span);
            put_run(
                atlas,
                id,
                [x, y, span as f32 * cw, ch],
                with_alpha(rgb(0xff, 0xff, 0xff), 255),
                &mut gl[k * GLYPHS..(k + 1) * GLYPHS],
            );
            under[k] = solid(
                x,
                y + ch - scale.max(1.0) * 1.5,
                span as f32 * cw,
                scale.max(1.0),
                rgb(0xff, 0xff, 0xff),
            );
            col += span;
            k += 1;
        }
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

impl Compositor<AppState> for Terminal {
    fn features(&self) -> wgpu::Features {
        wgpu::Features::IMMEDIATES
    }

    fn animating(&self) -> bool {
        self.shared.animating()
    }

    fn render(&mut self, f: &mut Frame<'_, AppState>) {
        let epoch = self.shared.render_epoch();
        let t0 = Instant::now();
        let a0 = alloc::count();
        let mut encode_allocs = 0u64;
        let (device, queue) = (f.device, f.queue);
        let scale = f.rt.env.scale;
        let key = f.device_key();
        let stale = match &self.gpu {
            Some(g) => g.key != key || g.atlas.scale != scale,
            None => true,
        };
        if stale {
            self.gpu = Some(make_gpu(device, key, scale));
            self.last_atlas_gen = 0;
        }
        let shared = Arc::clone(&self.shared);
        let gpu = self.gpu.as_mut().unwrap();
        let misses0 = gpu.atlas.misses;

        let terminal: &TerminalUi = &f.rt.state.terminal;
        let scroll = terminal.scroll;
        let selection = terminal.selection;
        let preedit = terminal.preedit.clone();
        let cursor_on = terminal.cursor_on;
        let mut rows_uploaded = 0u64;
        let mut extras_dirty = false;
        let mut work: Option<(usize, usize)> = None;

        {
            let mut slot = shared.lock();
            let damage = std::mem::take(&mut slot.damage);
            if let Some(grid) = slot.grid.as_ref() {
                let (cols, rows) = (
                    grid.visible.first().map_or(0, |row| row.cells.len()),
                    grid.visible.len(),
                );
                if cols > 0 && rows > 0 {
                    if gpu
                        .pane
                        .as_ref()
                        .is_none_or(|pane| pane.cols != cols || pane.rows != rows)
                    {
                        gpu.pane = Some(make_pane_gpu(device, cols, rows));
                    }
                    let pg = gpu.pane.as_mut().unwrap();
                    let total_cells = pg.cells();
                    let full =
                        matches!(damage, RowDamage::Full) || pg.uploaded_scroll != Some(scroll);
                    let damaged = damage.rows();
                    let e0 = alloc::count();
                    let mut run: Option<usize> = None;
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
                                        &self.palette,
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
                                        &self.palette,
                                        &[],
                                        v,
                                        (cols, total_cells),
                                        &mut pg.insts,
                                    );
                                    pg.uploaded[v] = Uploaded::Blank;
                                }
                            }
                            rows_uploaded += 1;
                            run.get_or_insert(v);
                        } else if let Some(start) = run.take() {
                            work = Some(match work {
                                Some((a, b)) => (a.min(start), b.max(v)),
                                None => (start, v),
                            });
                        }
                    }
                    let key = extras_key(grid, scroll, selection, &preedit, cursor_on, scale);
                    if key != pg.extras_key {
                        let at = pg.extras_at();
                        encode_extras(
                            &mut gpu.atlas,
                            grid,
                            scroll,
                            selection,
                            &preedit,
                            cursor_on,
                            &mut pg.insts[at..],
                        );
                        pg.extras_key = key;
                        extras_dirty = true;
                    }
                    pg.uploaded_scroll = Some(scroll);
                    encode_allocs += alloc::count() - e0;
                }
            } else {
                gpu.pane = None;
            }
        }

        gpu.atlas.flush(queue);
        if let (Some((start, end)), Some(pg)) = (work, gpu.pane.as_ref()) {
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
        if extras_dirty && let Some(pg) = gpu.pane.as_ref() {
            let at = pg.extras_at();
            f.write_buffer(
                &pg.buffer,
                (at * size_of::<Inst>()) as u64,
                bytemuck::cast_slice(&pg.insts[at..]),
            );
        }
        if gpu.atlas.generation != self.last_atlas_gen {
            self.last_atlas_gen = gpu.atlas.generation;
            if let Some(pg) = gpu.pane.as_mut() {
                pg.uploaded.iter_mut().for_each(|u| *u = Uploaded::Unset);
            }
        }
        let pipe = pipeline(gpu, device, f.format);
        let new_glyphs = gpu.atlas.misses != misses0;
        let a1 = alloc::count();
        let t1 = Instant::now();

        let clear = f.rt.clear_color();
        f.ui.prepare(device, queue, &mut f.rt.text.atlas, &f.rt.instances, f.size);
        let mut draws = 0u64;
        let mut drawn_instances = 0u64;
        let mut hook_allocs = 0u64;
        let gpu = &*gpu;
        let cell_w = gpu.atlas.cell_w;
        let cell_h = gpu.atlas.cell_h;
        f.ui.draw_with(
            f.encoder,
            f.target,
            clear,
            &mut |pass: &mut wgpu::RenderPass<'_>, s: &EffectSite<'_>| {
                if s.mark == MARK_BASE {
                    let r = s.instance.rect;
                    let cols = ((r[2] / cell_w).floor() as i64).max(0) as u16;
                    let rows = ((r[3] / cell_h).floor() as i64).max(0) as u16;
                    if cols > 0 && rows > 0 {
                        shared.set_measured(cols, rows);
                    }
                }
                let Some(pg) = gpu.pane.as_ref() else {
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
                pass.set_pipeline(&gpu.pipelines[pipe].1);
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

        shared.mark_rendered_to(epoch);
        let us = |a: Instant, b: Instant| b.duration_since(a).as_secs_f64() * 1e6;
        let mut st = self.stats.lock().unwrap();
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
        }
    }

    fn summary(&mut self, _device: &wgpu::Device) -> Option<String> {
        Some(self.stats.lock().unwrap().report())
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

#[allow(dead_code)]
fn _cursor_shape(cursor: Cursor) -> CursorShape {
    cursor.shape
}

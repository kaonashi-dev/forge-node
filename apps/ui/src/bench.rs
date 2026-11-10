//! Headless measurements: the fixture session switch and echo, the integrated
//! 125 Hz terminal stream and the 88-row editor. `scripts/perf.sh` adds
//! startup/RSS, and writes the combined report.
//!
//! The terminal stream applies one synthetic delta per frame through
//! `SharedTerminal` (the path the fixture worker uses) and draws with the real
//! compositor on a headless GPU, so build and encode are the shipped code.

use std::path::Path;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use client::Store;
use domain::{
    EditorDecoration, EditorDecorationKind, EditorFold, EditorFrame, EditorMark, EditorPlace,
    EditorRange, EditorRow, EditorScope, EditorSeverity, EditorSpan, SessionKind, TerminalId,
};
use rune::Compositor;
use rune::{Env, Runtime};

use crate::app::AppState;
use crate::connection::{self, Boot, Metrics, Mode};
use crate::terminal::SharedTerminal;
use crate::terminal::fixture::Feed;
use crate::terminal::renderer::Terminal;
use crate::terminal::stats::Stats;
use crate::ui;

pub const RUNE_REV: &str = "938ba14f09bb3381946b3486ef8aa36363a3e8b2";
pub const RATE_HZ: f64 = 125.0;
/// The integrated pane at the 1180×760 window: 49 rows of a 128-column shell.
pub const TERMINAL_SIZE: (u16, u16) = (128, 49);
/// W2's visible-row count, and the window that shows all of them.
pub const EDITOR_ROWS: u32 = 88;
pub const EDITOR_VIEWPORT: (f32, f32) = (1200.0, 1890.0);

#[derive(Clone, Copy, Debug, Default, PartialEq, serde::Serialize)]
pub struct Sample {
    pub p50: f64,
    pub p95: f64,
    pub max: f64,
}

impl Sample {
    pub fn of(values: &mut [f64]) -> Self {
        if values.is_empty() {
            return Self::default();
        }
        values.sort_by(f64::total_cmp);
        let at = |p: f64| values[rank(values.len(), p)];
        let round = |ms: f64| (ms * 1000.0).round() / 1000.0;
        Self {
            p50: round(at(50.0)),
            p95: round(at(95.0)),
            max: round(values[values.len() - 1]),
        }
    }
}

/// Zero-based index of the `p`th percentile, by rank (the ceiling).
fn rank(len: usize, p: f64) -> usize {
    (((len as f64) * p / 100.0).ceil() as usize)
        .saturating_sub(1)
        .min(len - 1)
}

#[derive(serde::Serialize)]
pub struct Machine {
    pub os: &'static str,
    pub arch: &'static str,
    pub profile: &'static str,
    pub rust: String,
    pub rune: &'static str,
    pub unix_ms: u64,
}

impl Machine {
    fn now() -> Self {
        Self {
            os: std::env::consts::OS,
            arch: std::env::consts::ARCH,
            profile: if cfg!(debug_assertions) {
                "debug"
            } else {
                "release"
            },
            rust: option_env!("RUSTUP_TOOLCHAIN")
                .unwrap_or("1.99.1")
                .to_string(),
            rune: RUNE_REV,
            unix_ms: SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .map_or(0, |elapsed| elapsed.as_millis() as u64),
        }
    }
}

#[derive(serde::Serialize)]
pub struct FixtureReport {
    pub iterations: usize,
    pub sessions: usize,
    pub switch_ms: Sample,
    pub view_ms: Sample,
    pub echo_ms: Sample,
    pub socket_ops: u64,
    pub ui_socket_ops: u64,
}

pub fn run(store: Store, iterations: usize) -> FixtureReport {
    let metrics = Arc::new(Metrics::default());
    metrics.note_ui_thread();
    let shared = SharedTerminal::new();
    let mut state = AppState::for_bench(store.clone(), Arc::clone(&metrics), Arc::clone(&shared));
    if let Some(row) = state.active_row().cloned()
        && let Some(terminal) = row.terminal_id
    {
        state
            .terminal
            .activate_fixture(1, row.id, terminal, (80, 24));
        state.terminal.focus_terminal = true;
    }
    let sessions = state.rows.len();
    assert!(sessions > 1, "benchmark needs at least two sessions");
    let mut runtime = Runtime::new(
        state,
        ui::view,
        (1180.0, 760.0),
        Env {
            system_dark: true,
            scale: 2.0,
        },
    );
    runtime.poll = Some(crate::app::poll);
    runtime.frame(0.0);
    for step in 0..20 {
        let key = format!("session-row-{}", step % sessions);
        let (x, y) = runtime
            .find(&key)
            .unwrap_or_else(|| panic!("warmup row {key} is not laid out"));
        runtime.pointer_down(x, y);
        runtime.pointer_up(x, y);
        runtime.frame(1.0 / 60.0);
    }

    let mut switch = Vec::with_capacity(iterations);
    let mut view = Vec::with_capacity(iterations);
    for step in 1..=iterations {
        let index = step % sessions;
        let key = format!("session-row-{index}");
        let (x, y) = runtime
            .find(&key)
            .unwrap_or_else(|| panic!("benchmark row {key} is not laid out"));
        let started = Instant::now();
        runtime.pointer_down(x, y);
        runtime.pointer_up(x, y);
        runtime.frame(1.0 / 60.0);
        switch.push(started.elapsed().as_secs_f64() * 1000.0);
        if let Some(ms) = runtime.state.view_ms.get() {
            view.push(ms);
        }
    }

    let mut echo = bench_echo(store, iterations.min(60), &metrics);

    FixtureReport {
        iterations,
        sessions,
        switch_ms: Sample::of(&mut switch),
        view_ms: Sample::of(&mut view),
        echo_ms: Sample::of(&mut echo),
        socket_ops: metrics
            .socket_ops
            .load(std::sync::atomic::Ordering::Relaxed),
        ui_socket_ops: metrics
            .ui_socket_ops
            .load(std::sync::atomic::Ordering::Relaxed),
    }
}

fn bench_echo(store: Store, iterations: usize, metrics: &Arc<Metrics>) -> Vec<f64> {
    let shared = SharedTerminal::new();
    let boot = Arc::new(Boot {
        mode: Mode::Fixture {
            snapshot: Arc::new(store.clone()),
            reconnect_after: None,
        },
        metrics: Arc::clone(metrics),
        terminal: Arc::clone(&shared),
    });
    let sink: connection::EventSink = Arc::new(|_| {});
    let handle = connection::spawn(boot, sink, Arc::new(|| {}));
    let mut state = AppState::for_bench(store, Arc::clone(metrics), Arc::clone(&shared));
    let row = state.active_row().cloned().expect("active row");
    let terminal_id = row.terminal_id.expect("fixture terminal");
    state
        .terminal
        .activate_fixture(1, row.id, terminal_id, (80, 24));
    state.terminal.focus_terminal = true;
    state.connection = Some(handle);
    shared.set_measured(80, 24);
    let mut runtime = Runtime::new(
        state,
        ui::view,
        (1180.0, 760.0),
        Env {
            system_dark: true,
            scale: 2.0,
        },
    );
    runtime.poll = Some(crate::app::poll);
    runtime.frame(0.0);
    let _ = echo_once(&mut runtime, &shared, "abcdefghijklmnopqrstuvwxyz");
    let notify = shared.subscribe();

    let mut expected = String::new();
    let mut samples = Vec::with_capacity(iterations);
    for step in 0..iterations {
        let text = format!("{}", (b'a' + (step % 26) as u8) as char);
        expected.push_str(&text);
        let started = Instant::now();
        runtime.update(|state, tx| {
            state.terminal_key(
                &rune::KeyEvent {
                    key: rune::Key::Text(text),
                    mods: rune::Mods::default(),
                },
                tx,
            );
        });
        let deadline = Instant::now() + Duration::from_secs(2);
        loop {
            let line_matches = {
                let slot = shared.lock();
                slot.grid.as_ref().is_some_and(|grid| {
                    grid.visible.last().is_some_and(|last| {
                        let line: String =
                            last.cells.iter().map(|cell| cell.text.as_str()).collect();
                        line.trim_end().ends_with(&format!("fixture> {expected}"))
                    })
                })
            };
            if line_matches {
                break;
            }
            let left = deadline.saturating_duration_since(Instant::now());
            if left.is_zero() {
                panic!("fixture did not echo {expected:?} within 2 s");
            }
            match notify.recv_timeout(left) {
                Ok(_) => {}
                Err(flume::RecvTimeoutError::Timeout) => {
                    panic!("fixture did not echo {expected:?} within 2 s")
                }
                Err(flume::RecvTimeoutError::Disconnected) => {
                    panic!("fixture notifier disconnected")
                }
            }
        }
        runtime.frame(1.0 / 60.0);
        samples.push(started.elapsed().as_secs_f64() * 1000.0);
    }
    samples
}

fn echo_once(runtime: &mut Runtime<AppState>, shared: &SharedTerminal, text: &str) -> bool {
    runtime.update(|state, tx| {
        state.terminal_key(
            &rune::KeyEvent {
                key: rune::Key::Text(text.to_string()),
                mods: rune::Mods::default(),
            },
            tx,
        );
    });
    let deadline = Instant::now() + Duration::from_secs(2);
    while Instant::now() < deadline {
        let echoed = {
            let slot = shared.lock();
            slot.grid.as_ref().is_some_and(|grid| {
                grid.visible.last().is_some_and(|last| {
                    let line: String = last.cells.iter().map(|cell| cell.text.as_str()).collect();
                    line.trim_end().ends_with(&format!("fixture> {text}"))
                })
            })
        };
        if echoed {
            runtime.update(|state, tx| {
                state.terminal_key(
                    &rune::KeyEvent {
                        key: rune::Key::Enter,
                        mods: rune::Mods::default(),
                    },
                    tx,
                );
            });
            runtime.frame(1.0 / 60.0);
            return true;
        }
        std::thread::yield_now();
    }
    false
}

#[derive(serde::Serialize)]
pub struct TerminalReport {
    pub blocked: Option<String>,
    pub frames: usize,
    pub rate_hz: f64,
    pub cols: u16,
    pub rows: u16,
    pub feed: &'static str,
    pub deltas_applied: u64,
    pub delta_apply_ms: Option<Sample>,
    pub update_ms: Option<Sample>,
    pub build_ms: Option<Sample>,
    pub encode_ms: Option<Sample>,
    pub frame_cpu_ms: Option<Sample>,
    pub host_frame_ms: Option<Sample>,
    pub total_cpu_ms: Option<Sample>,
    pub paced_loop_interval_ms: Option<Sample>,
    pub view_ms: Option<Sample>,
    pub view_commits: u64,
    pub rows_uploaded: u64,
    pub draw_calls: u64,
    pub instances_per_draw: u64,
    pub glyph_misses: u64,
    pub build_allocations: u64,
    pub encode_allocations: u64,
    pub gpu: String,
    pub gpu_init_ms: f64,
    pub socket_ops: u64,
    pub ui_socket_ops: u64,
    pub notes: Vec<String>,
}

impl TerminalReport {
    fn blocked(error: String) -> Self {
        Self {
            blocked: Some(error),
            frames: 0,
            rate_hz: RATE_HZ,
            cols: TERMINAL_SIZE.0,
            rows: TERMINAL_SIZE.1,
            feed: "rich full scroll",
            deltas_applied: 0,
            delta_apply_ms: None,
            update_ms: None,
            build_ms: None,
            encode_ms: None,
            frame_cpu_ms: None,
            host_frame_ms: None,
            total_cpu_ms: None,
            paced_loop_interval_ms: None,
            view_ms: None,
            view_commits: 0,
            rows_uploaded: 0,
            draw_calls: 0,
            instances_per_draw: 0,
            glyph_misses: 0,
            build_allocations: 0,
            encode_allocations: 0,
            gpu: "unavailable".to_string(),
            gpu_init_ms: 0.0,
            socket_ops: 0,
            ui_socket_ops: 0,
            notes: Vec::new(),
        }
    }
}

pub fn terminal_125(store: Store, frames: usize) -> TerminalReport {
    match terminal_125_inner(store, frames) {
        Ok(report) => report,
        Err(error) => TerminalReport::blocked(error),
    }
}

fn terminal_125_inner(store: Store, frames: usize) -> Result<TerminalReport, String> {
    let metrics = Arc::new(Metrics::default());
    metrics.note_ui_thread();
    let shared = SharedTerminal::new();
    let mut state = AppState::for_bench(store, Arc::clone(&metrics), Arc::clone(&shared));
    let row = state
        .active_row()
        .cloned()
        .ok_or("the fixture snapshot has no active session")?;
    let terminal_id = row
        .terminal_id
        .ok_or("the active fixture session has no terminal")?;
    state
        .terminal
        .activate_fixture(1, row.id, terminal_id, TERMINAL_SIZE);
    state.terminal.focus_terminal = true;
    let mut runtime = Runtime::new(
        state,
        ui::view,
        (1180.0, 760.0),
        Env {
            system_dark: true,
            scale: 2.0,
        },
    );
    runtime.track_marks = true;
    runtime.frame(0.0);

    let scale = 2.0_f32;
    let (width, height) = (
        (1180.0 * scale).round() as u32,
        (760.0 * scale).round() as u32,
    );
    let stats = Arc::new(Mutex::new(Stats::with_capacity(frames + 16)));
    let mut compositor = Terminal::new(Arc::clone(&shared), Arc::clone(&stats), None);
    let gpu_started = Instant::now();
    let gpu = rune::render::Gpu::try_with_features(
        rune::render::Gpu::instance(),
        None,
        compositor.features(),
    )
    .map_err(|error| format!("headless GPU unavailable: {error}"))?;
    let target = rune::render::Offscreen::new(&gpu.device, width, height);
    let mut renderer = rune::render::Renderer::new(&gpu.device, rune::render::OFFSCREEN_FORMAT);
    let gpu_init_ms = gpu_started.elapsed().as_secs_f64() * 1000.0;
    let info = gpu.adapter.get_info();

    let mut feed = Feed::new(true, true);
    let warmup = 48.min(frames.max(12));
    for _ in 0..warmup {
        apply_tick(&shared, &mut feed, 1, terminal_id, &metrics)?;
        runtime.frame(1.0 / RATE_HZ);
        render(
            &gpu,
            &target,
            &mut renderer,
            &mut runtime,
            &mut compositor,
            0.0,
        )?;
    }
    let baseline = {
        let mut stats = stats
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let baseline = StatsBaseline {
            rows_uploaded: stats.rows_uploaded,
            draw_calls: stats.draw_calls,
            instances_drawn: stats.instances_drawn,
            glyph_misses: stats.glyph_misses,
            build_allocs: stats.build_allocs,
            encode_allocs: stats.encode_allocs,
        };
        stats.update_us.clear();
        stats.build_us.clear();
        stats.encode_us.clear();
        stats.total_us.clear();
        stats.tick_us.clear();
        baseline
    };

    let mut delta_ms = Vec::with_capacity(frames);
    let mut host_ms = Vec::with_capacity(frames);
    let mut interval_ms = Vec::with_capacity(frames.saturating_sub(1));
    let mut view_ms = Vec::new();
    let commits_before = runtime.stats.commits;
    let mut previous_start: Option<Instant> = None;
    for _ in 0..frames {
        let started = Instant::now();
        if let Some(previous) = previous_start {
            interval_ms.push(started.duration_since(previous).as_secs_f64() * 1000.0);
        }
        previous_start = Some(started);
        delta_ms.push(apply_tick(&shared, &mut feed, 1, terminal_id, &metrics)?);
        let commits = runtime.stats.commits;
        host_ms.push(render(
            &gpu,
            &target,
            &mut renderer,
            &mut runtime,
            &mut compositor,
            1.0 / RATE_HZ,
        )?);
        if runtime.stats.commits != commits
            && let Some(ms) = runtime.state.view_ms.get()
        {
            view_ms.push(ms);
        }
        let due = started + Duration::from_secs_f64(1.0 / RATE_HZ);
        if let Some(wait) = due.checked_duration_since(Instant::now()) {
            std::thread::sleep(wait);
        }
    }
    let view_commits = runtime.stats.commits.saturating_sub(commits_before);
    gpu.device.poll(wgpu::PollType::wait_indefinitely()).ok();

    let stats = stats
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let mut update: Vec<f64> = stats.update_us.iter().map(|us| us / 1000.0).collect();
    let mut build: Vec<f64> = stats.build_us.iter().map(|us| us / 1000.0).collect();
    let mut encode: Vec<f64> = stats.encode_us.iter().map(|us| us / 1000.0).collect();
    let mut frame_cpu: Vec<f64> = stats.total_us.iter().map(|us| us / 1000.0).collect();
    if update.len() < frames || build.len() < frames || encode.len() < frames {
        return Err(format!(
            "compositor recorded {} frames, expected {frames}",
            update.len().min(build.len()).min(encode.len())
        ));
    }
    update.truncate(frames);
    build.truncate(frames);
    encode.truncate(frames);
    frame_cpu.truncate(frames);
    let mut total: Vec<f64> = frame_cpu
        .iter()
        .zip(&delta_ms)
        .map(|(frame, delta)| frame + delta)
        .collect();
    let rows_uploaded = stats.rows_uploaded.saturating_sub(baseline.rows_uploaded);
    let draw_calls = stats.draw_calls.saturating_sub(baseline.draw_calls);
    let drawn = stats
        .instances_drawn
        .saturating_sub(baseline.instances_drawn);
    let glyph_misses = stats.glyph_misses.saturating_sub(baseline.glyph_misses);
    let build_allocations = stats.build_allocs.saturating_sub(baseline.build_allocs);
    let encode_allocations = stats.encode_allocs.saturating_sub(baseline.encode_allocs);
    let deltas_applied = metrics
        .terminal_deltas
        .load(std::sync::atomic::Ordering::Relaxed)
        .saturating_sub(warmup as u64);
    drop(stats);

    Ok(TerminalReport {
        blocked: None,
        frames,
        rate_hz: RATE_HZ,
        cols: TERMINAL_SIZE.0,
        rows: TERMINAL_SIZE.1,
        feed: "rich full scroll (every row rebuilt every frame)",
        deltas_applied,
        delta_apply_ms: Some(Sample::of(&mut delta_ms)),
        update_ms: Some(Sample::of(&mut update)),
        build_ms: Some(Sample::of(&mut build)),
        encode_ms: Some(Sample::of(&mut encode)),
        frame_cpu_ms: Some(Sample::of(&mut frame_cpu)),
        host_frame_ms: Some(Sample::of(&mut host_ms)),
        total_cpu_ms: Some(Sample::of(&mut total)),
        paced_loop_interval_ms: Some(Sample::of(&mut interval_ms)),
        view_ms: (!view_ms.is_empty()).then(|| Sample::of(&mut view_ms)),
        view_commits,
        rows_uploaded,
        draw_calls,
        instances_per_draw: drawn.checked_div(draw_calls).unwrap_or(0),
        glyph_misses,
        build_allocations,
        encode_allocations,
        gpu: format!("{} ({:?})", info.name, info.backend),
        gpu_init_ms: (gpu_init_ms * 1000.0).round() / 1000.0,
        socket_ops: metrics
            .socket_ops
            .load(std::sync::atomic::Ordering::Relaxed),
        ui_socket_ops: metrics
            .ui_socket_ops
            .load(std::sync::atomic::Ordering::Relaxed),
        notes: vec![
            "one synthetic delta per frame through SharedTerminal::apply_delta, the path the fixture worker uses".to_string(),
            "the shell view is not rebuilt per delta; the compositor draws the pane".to_string(),
            "build/encode are the compositor's CPU clocks; GPU time is not included".to_string(),
            "frames are paced to 125 Hz; paced_loop_interval_ms includes the sleep and macOS timer overshoot, not display cadence".to_string(),
        ],
    })
}

struct StatsBaseline {
    rows_uploaded: u64,
    draw_calls: u64,
    instances_drawn: u64,
    glyph_misses: u64,
    build_allocs: u64,
    encode_allocs: u64,
}

/// Builds the next delta from the shared grid and applies it, as `fixture_tick` does.
fn apply_tick(
    shared: &SharedTerminal,
    feed: &mut Feed,
    generation: u64,
    terminal_id: TerminalId,
    metrics: &Metrics,
) -> Result<f64, String> {
    let delta = {
        let slot = shared.lock();
        let grid = slot.grid.as_ref().ok_or("the fixture grid disappeared")?;
        feed.delta(grid)
    };
    let started = Instant::now();
    let _ = shared.apply_delta(generation, terminal_id, &delta);
    metrics
        .terminal_deltas
        .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    Ok(started.elapsed().as_secs_f64() * 1000.0)
}

fn render(
    gpu: &rune::render::Gpu,
    target: &rune::render::Offscreen,
    renderer: &mut rune::render::Renderer,
    runtime: &mut Runtime<AppState>,
    compositor: &mut Terminal,
    dt: f64,
) -> Result<f64, String> {
    let started = Instant::now();
    let _ = runtime.frame(dt);
    let update_ms = started.elapsed().as_secs_f64() * 1000.0;
    let mut encoder = gpu
        .device
        .create_command_encoder(&wgpu::CommandEncoderDescriptor {
            label: Some("forge-ui-bench"),
        });
    {
        let mut frame = rune::Frame {
            device: &gpu.device,
            queue: &gpu.queue,
            encoder: &mut encoder,
            target: &target.view,
            format: rune::render::OFFSCREEN_FORMAT,
            size: (target.width, target.height),
            dt,
            update_ms,
            rt: runtime,
            ui: renderer,
        };
        compositor.render(&mut frame);
    }
    let _ = renderer.submit(&gpu.queue, encoder);
    Ok(started.elapsed().as_secs_f64() * 1000.0)
}

#[derive(serde::Serialize)]
pub struct EditorReport {
    pub blocked: Option<String>,
    pub rows: usize,
    pub visible_rows: usize,
    pub rate_hz: f64,
    pub folds: usize,
    pub mount_ms: f64,
    pub mount_view_ms: f64,
    pub mount_rows_built: u64,
    pub caret_view_ms: Option<Sample>,
    pub caret_frame_ms: Option<Sample>,
    pub caret_rows_rebuilt: u64,
    pub key_to_paint_cpu_ms: Option<Sample>,
    pub key_to_paint_rows_rebuilt: u64,
    pub socket_ops: u64,
    pub ui_socket_ops: u64,
    pub display_measured: bool,
    pub notes: Vec<String>,
}

impl EditorReport {
    fn blocked(error: String) -> Self {
        Self {
            blocked: Some(error),
            rows: 0,
            visible_rows: 0,
            rate_hz: RATE_HZ,
            folds: 0,
            mount_ms: 0.0,
            mount_view_ms: 0.0,
            mount_rows_built: 0,
            caret_view_ms: None,
            caret_frame_ms: None,
            caret_rows_rebuilt: 0,
            key_to_paint_cpu_ms: None,
            key_to_paint_rows_rebuilt: 0,
            socket_ops: 0,
            ui_socket_ops: 0,
            display_measured: false,
            notes: Vec::new(),
        }
    }
}

pub fn editor(store: Store, frames: usize) -> EditorReport {
    match editor_inner(store, frames) {
        Ok(report) => report,
        Err(error) => EditorReport::blocked(error),
    }
}

fn editor_inner(store: Store, frames: usize) -> Result<EditorReport, String> {
    let metrics = Arc::new(Metrics::default());
    metrics.note_ui_thread();
    let shared = SharedTerminal::new();
    let mut state = AppState::for_bench(store, Arc::clone(&metrics), Arc::clone(&shared));
    let row = state
        .rows
        .first()
        .cloned()
        .ok_or("the fixture snapshot has no sessions")?;
    let session = row.id;
    state.rows_mut()[0].kind = SessionKind::Editor;
    state.editor_surface = protocol::EditorSurface::Dom;
    state.active = Some(session);
    state.active_text = Some(row.id_text.clone());
    state.terminal.focus_terminal = false;
    state.editor.set_session(Some(session));
    let frame = editor_frame_fixture();
    let top = crate::editor::visual_base(&frame) as f32 * crate::editor::ROW_H;
    state
        .editor
        .scroll
        .set([top, EDITOR_ROWS as f32 * crate::editor::ROW_H]);
    state.editor.apply_frame(session, frame.clone());

    let mut runtime = Runtime::new(
        state,
        ui::view,
        EDITOR_VIEWPORT,
        Env {
            system_dark: true,
            scale: 2.0,
        },
    );
    let started = Instant::now();
    runtime.frame(0.0);
    let mount_ms = started.elapsed().as_secs_f64() * 1000.0;
    let mount_view_ms = runtime.state.view_ms.get().unwrap_or(0.0);
    let mount_rows_built = runtime.state.editor.row_builds.get();
    let viewport = runtime.state.editor.scroll.get()[1];
    let visible_rows = (viewport / crate::editor::ROW_H).floor().max(0.0) as usize;

    let mut caret_frame = frame.clone();
    let mut caret_frame_ms = Vec::with_capacity(frames);
    let mut caret_view_ms = Vec::with_capacity(frames);
    for step in 0..frames {
        caret_frame.doc_version = caret_frame.doc_version.saturating_add(1);
        caret_frame.caret.column = 4 + (step % 40) as u32;
        let next = caret_frame.clone();
        let started = Instant::now();
        runtime.update(|state, _tx| {
            state.editor.apply_frame(session, next);
        });
        runtime.frame(1.0 / RATE_HZ);
        caret_frame_ms.push(started.elapsed().as_secs_f64() * 1000.0);
        if let Some(ms) = runtime.state.view_ms.get() {
            caret_view_ms.push(ms);
        }
    }
    let caret_rows_rebuilt = runtime
        .state
        .editor
        .row_builds
        .get()
        .saturating_sub(mount_rows_built);

    let mut keys = Vec::with_capacity(frames);
    let mut key_frame = caret_frame;
    for step in 0..frames {
        key_frame.doc_version = key_frame.doc_version.saturating_add(1);
        key_frame.caret.line = frame.first_line + 30 + (step % 8) as u32;
        let next = key_frame.clone();
        let started = Instant::now();
        runtime.update(|state, tx| {
            state.editor_key(
                &rune::KeyEvent {
                    key: rune::Key::Right,
                    mods: rune::Mods::default(),
                },
                tx,
            );
            state.editor.apply_frame(session, next);
        });
        runtime.frame(1.0 / RATE_HZ);
        keys.push(started.elapsed().as_secs_f64() * 1000.0);
    }
    let key_rows_rebuilt = runtime
        .state
        .editor
        .row_builds
        .get()
        .saturating_sub(mount_rows_built + caret_rows_rebuilt);

    Ok(EditorReport {
        blocked: None,
        rows: frame.rows.len(),
        visible_rows,
        rate_hz: RATE_HZ,
        folds: frame.rows.iter().filter(|row| row.fold.is_some()).count(),
        mount_ms: (mount_ms * 1000.0).round() / 1000.0,
        mount_view_ms: (mount_view_ms * 1000.0).round() / 1000.0,
        mount_rows_built,
        caret_view_ms: Some(Sample::of(&mut caret_view_ms)),
        caret_frame_ms: Some(Sample::of(&mut caret_frame_ms)),
        caret_rows_rebuilt,
        key_to_paint_cpu_ms: Some(Sample::of(&mut keys)),
        key_to_paint_rows_rebuilt: key_rows_rebuilt,
        socket_ops: metrics
            .socket_ops
            .load(std::sync::atomic::Ordering::Relaxed),
        ui_socket_ops: metrics
            .ui_socket_ops
            .load(std::sync::atomic::Ordering::Relaxed),
        display_measured: false,
        notes: vec![
            "key_to_paint_cpu_ms is a CPU proxy: key handler, one state update and one 125 Hz frame; display and vsync are not measured".to_string(),
            "caret-only frames change doc_version and the caret, never a row fingerprint".to_string(),
            "rows_rebuilt counts text-row memo builds (EditorUi::row_builds)".to_string(),
        ],
    })
}

/// The 88-row editor window W2 measured: protocol 37 fingerprints and folds,
/// mixed scopes, wide graphemes, marks, diagnostics and decorations.
pub fn editor_frame_fixture() -> EditorFrame {
    let rows: Vec<EditorRow> = (0..EDITOR_ROWS)
        .map(|index| {
            let line = 100 + index;
            let spans = fixture_spans(index);
            let mut row = EditorRow {
                line,
                truncated: index == 61,
                spans: Arc::new(spans),
                mark: match index % 13 {
                    0 => Some(EditorMark::Modified),
                    7 => Some(EditorMark::Added),
                    11 => Some(EditorMark::Deleted),
                    _ => None,
                },
                diagnostic: match index % 23 {
                    0 => Some(EditorSeverity::Error),
                    5 => Some(EditorSeverity::Warning),
                    9 => Some(EditorSeverity::Info),
                    _ => None,
                },
                fold: if index % 16 == 0 {
                    Some(if index % 32 == 0 { 7 } else { 0 })
                } else {
                    None
                },
                fingerprint: 0,
            };
            row.fingerprint = row.content_fingerprint();
            row
        })
        .collect();
    let folds: Vec<EditorFold> = rows
        .iter()
        .filter_map(|row| {
            row.fold
                .filter(|hidden| *hidden > 0)
                .map(|hidden| EditorFold {
                    header: row.line,
                    hidden,
                })
        })
        .collect();
    EditorFrame {
        buffer_id: 7,
        doc_version: 1,
        first_line: 100,
        total_lines: 520,
        hidden_lines: 140,
        hidden_above: 9,
        rows: Arc::new(rows),
        clipped: false,
        folded: folds,
        caret: EditorPlace {
            line: 130,
            column: 12,
        },
        selection: vec![EditorRange {
            from: EditorPlace {
                line: 105,
                column: 4,
            },
            to: EditorPlace {
                line: 107,
                column: 18,
            },
        }],
        extra_carets: Vec::new(),
        decorations: vec![
            EditorDecoration {
                kind: EditorDecorationKind::Match,
                range: EditorRange {
                    from: EditorPlace {
                        line: 112,
                        column: 4,
                    },
                    to: EditorPlace {
                        line: 112,
                        column: 9,
                    },
                },
            },
            EditorDecoration {
                kind: EditorDecorationKind::Bracket,
                range: EditorRange {
                    from: EditorPlace {
                        line: 130,
                        column: 12,
                    },
                    to: EditorPlace {
                        line: 130,
                        column: 13,
                    },
                },
            },
        ],
    }
}

fn fixture_spans(index: u32) -> Vec<EditorSpan> {
    let line = 100 + index;
    let plain = |text: String| EditorSpan {
        text,
        scope: EditorScope::Plain,
    };
    let scoped = |text: &str, scope| EditorSpan {
        text: text.to_string(),
        scope,
    };
    match index % 8 {
        0 => vec![
            scoped("pub fn ", EditorScope::Keyword),
            scoped(&format!("handler_{line}"), EditorScope::Function),
            scoped("(frame: &EditorFrame) -> Result<()> {", EditorScope::Plain),
        ],
        1 => vec![
            scoped("    // ", EditorScope::Comment),
            scoped(
                &format!("row {line}: memoised on (line, fingerprint)"),
                EditorScope::Comment,
            ),
        ],
        2 => vec![
            scoped("    let ", EditorScope::Keyword),
            scoped("width", EditorScope::Plain),
            scoped(" = ", EditorScope::Plain),
            scoped("88", EditorScope::Number),
            scoped(" * ROW_H;", EditorScope::Plain),
        ],
        3 => vec![
            scoped("    if ", EditorScope::ControlKeyword),
            scoped("caret.line == line", EditorScope::Plain),
            scoped(" { ", EditorScope::Plain),
            scoped("paint_selection", EditorScope::Function),
            scoped("(frame); }", EditorScope::Plain),
        ],
        4 => vec![
            plain("    漢字とかな 한글 ".to_string()),
            scoped("é\u{301}", EditorScope::String),
            plain(" — wide graphemes stay in one row".to_string()),
        ],
        5 => vec![
            scoped("\t", EditorScope::Plain),
            scoped("match ", EditorScope::ControlKeyword),
            scoped("kind", EditorScope::Plain),
            scoped(" { ", EditorScope::Plain),
            scoped("Fold", EditorScope::Type),
            scoped(" => ", EditorScope::Plain),
            scoped("toggle", EditorScope::Function),
            scoped("(), _ => {} }", EditorScope::Plain),
        ],
        6 => vec![
            scoped("    ", EditorScope::Plain),
            scoped("const MAX_ROWS", EditorScope::Constant),
            scoped(": ", EditorScope::Plain),
            scoped("usize", EditorScope::Type),
            scoped(" = 512;", EditorScope::Plain),
        ],
        _ => vec![
            scoped("    self.", EditorScope::Plain),
            scoped("reveal_caret", EditorScope::Function),
            scoped("(); // ", EditorScope::Comment),
            scoped(&format!("line {line}"), EditorScope::Comment),
        ],
    }
}

#[derive(serde::Serialize)]
pub struct AllReport {
    pub schema: u32,
    pub machine: Machine,
    pub fixture: FixtureReport,
    pub terminal_125: TerminalReport,
    pub editor: EditorReport,
}

pub fn all(store: Store, iterations: usize, frames: usize) -> AllReport {
    AllReport {
        schema: 1,
        machine: Machine::now(),
        fixture: run(store.clone(), iterations),
        terminal_125: terminal_125(store.clone(), frames),
        editor: editor(store, frames),
    }
}

pub fn write_json<T: serde::Serialize>(path: &Path, report: &T) -> Result<(), String> {
    let mut text = serde_json::to_string_pretty(report).map_err(|error| error.to_string())?;
    text.push('\n');
    std::fs::write(path, text).map_err(|error| format!("{}: {error}", path.display()))
}

pub fn fixture_path() -> std::path::PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("fixtures/snapshot.json")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::connection::fixture_store;

    #[test]
    fn session_switch_stays_under_the_w3_budget() {
        let store = fixture_store(&fixture_path()).expect("fixture snapshot decodes");
        let report = run(store, 120);
        eprintln!(
            "session switch: p50 {:.2} ms, p95 {:.2} ms, max {:.2} ms; echo p50 {:.2} ms p95 {:.2} ms",
            report.switch_ms.p50,
            report.switch_ms.p95,
            report.switch_ms.max,
            report.echo_ms.p50,
            report.echo_ms.p95
        );
        assert!(
            report.switch_ms.p95 < 50.0,
            "session switch p95 {:.2} ms exceeds 50 ms",
            report.switch_ms.p95
        );
        assert!(report.ui_socket_ops == 0);
    }

    #[test]
    fn percentiles_use_the_ceiling_rank() {
        let mut values: Vec<f64> = (1..=100).map(|value| value as f64).collect();
        let sample = Sample::of(&mut values);
        assert_eq!(sample.p50, 50.0);
        assert_eq!(sample.p95, 95.0);
        assert_eq!(sample.max, 100.0);
        assert_eq!(Sample::of(&mut []), Sample::default());
    }

    #[test]
    fn the_editor_fixture_is_a_real_88_row_window() {
        let frame = editor_frame_fixture();
        assert_eq!(frame.rows.len(), EDITOR_ROWS as usize);
        assert_eq!(frame.first_line, 100);
        assert!(frame.hidden_lines > 0 && frame.hidden_above > 0);
        assert!(frame.rows.iter().all(|row| row.fingerprint != 0));
        assert!(
            frame.rows.iter().any(|row| row.fold.is_some()),
            "folds are part of the protocol-37 fixture"
        );
        assert!(
            frame
                .rows
                .iter()
                .any(|row| row.text().chars().any(|ch| ch as u32 > 0x2e80)),
            "wide graphemes exercise shaping"
        );
        let same = editor_frame_fixture();
        assert_eq!(
            frame
                .rows
                .iter()
                .map(|row| row.fingerprint)
                .collect::<Vec<_>>(),
            same.rows
                .iter()
                .map(|row| row.fingerprint)
                .collect::<Vec<_>>(),
            "fingerprints are stable for an unchanged frame"
        );
        let mut changed = frame.rows[0].clone();
        changed.spans = Arc::new(vec![EditorSpan {
            text: "different".to_string(),
            scope: EditorScope::Plain,
        }]);
        assert_ne!(changed.content_fingerprint(), frame.rows[0].fingerprint);
    }

    #[test]
    fn the_terminal_stream_applies_one_delta_per_frame() {
        let store = fixture_store(&fixture_path()).expect("fixture snapshot decodes");
        let metrics = Arc::new(Metrics::default());
        let shared = SharedTerminal::new();
        let mut state = AppState::for_bench(store, Arc::clone(&metrics), Arc::clone(&shared));
        let row = state.active_row().cloned().expect("active row");
        let terminal = row.terminal_id.expect("fixture terminal");
        state
            .terminal
            .activate_fixture(1, row.id, terminal, TERMINAL_SIZE);
        let mut feed = Feed::new(true, true);
        for step in 0..25 {
            let delta = apply_tick(&shared, &mut feed, 1, terminal, &metrics)
                .expect("a delta for the shared grid");
            assert!(delta >= 0.0);
            let slot = shared.lock();
            assert_eq!(slot.grid.as_ref().unwrap().last_seq, step + 2);
        }
        assert_eq!(
            metrics
                .terminal_deltas
                .load(std::sync::atomic::Ordering::Relaxed),
            25
        );
        assert_eq!(
            metrics
                .ui_socket_ops
                .load(std::sync::atomic::Ordering::Relaxed),
            0
        );
    }

    #[test]
    fn the_report_sections_serialize() {
        let report = AllReport {
            schema: 1,
            machine: Machine::now(),
            fixture: FixtureReport {
                iterations: 1,
                sessions: 2,
                switch_ms: Sample {
                    p50: 1.0,
                    p95: 2.0,
                    max: 3.0,
                },
                view_ms: Sample::default(),
                echo_ms: Sample::default(),
                socket_ops: 0,
                ui_socket_ops: 0,
            },
            terminal_125: TerminalReport::blocked("no gpu in this test".to_string()),
            editor: EditorReport::blocked("no editor in this test".to_string()),
        };
        let value: serde_json::Value = serde_json::to_value(&report).expect("serializes");
        assert_eq!(value["schema"], 1);
        assert!(value["terminal_125"]["blocked"].is_string());
        assert!(value["editor"]["blocked"].is_string());
        assert!(value["fixture"]["switch_ms"]["p95"].is_number());
    }
}

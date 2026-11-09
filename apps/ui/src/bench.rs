//! Headless measurements. Session switching drives the real view through
//! `Runtime`; terminal echo drives the fixture worker's real delta path and
//! then one rendered frame, so both numbers include the paint-side work the
//! integrated app does.

use std::path::Path;
use std::sync::Arc;
use std::time::{Duration, Instant};

use client::Store;
use rune::{Env, Runtime};

use crate::app::AppState;
use crate::connection::{self, Boot, Metrics, Mode};
use crate::terminal::SharedTerminal;
use crate::ui;

#[derive(serde::Serialize)]
pub struct Report {
    pub iterations: usize,
    pub sessions: usize,
    pub switch_p50_ms: f64,
    pub switch_p95_ms: f64,
    pub switch_max_ms: f64,
    pub view_p50_ms: f64,
    pub view_p95_ms: f64,
    pub echo_p50_ms: f64,
    pub echo_p95_ms: f64,
    pub echo_max_ms: f64,
    pub socket_ops: u64,
    pub ui_socket_ops: u64,
}

pub fn run(store: Store, iterations: usize) -> Report {
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

    let mut echo = bench_echo(store, iterations.min(200), &metrics);

    Report {
        iterations,
        sessions,
        switch_p50_ms: percentile(&mut switch, 50),
        switch_p95_ms: percentile(&mut switch, 95),
        switch_max_ms: switch.last().copied().unwrap_or_default(),
        view_p50_ms: percentile(&mut view, 50),
        view_p95_ms: percentile(&mut view, 95),
        echo_p50_ms: percentile(&mut echo, 50),
        echo_p95_ms: percentile(&mut echo, 95),
        echo_max_ms: echo.last().copied().unwrap_or_default(),
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

    let mut samples = Vec::with_capacity(iterations);
    for step in 0..iterations {
        let started = Instant::now();
        let text = format!("{}", (b'a' + (step % 26) as u8) as char);
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
        let mut echoed = false;
        while Instant::now() < deadline {
            {
                let slot = shared.lock();
                if let Some(grid) = slot.grid.as_ref()
                    && let Some(last) = grid.visible.last()
                {
                    let line: String = last.cells.iter().map(|cell| cell.text.as_str()).collect();
                    if line.contains("fixture>") {
                        echoed = true;
                    }
                }
            }
            if echoed {
                break;
            }
            std::thread::sleep(Duration::from_millis(1));
        }
        runtime.frame(1.0 / 60.0);
        samples.push(started.elapsed().as_secs_f64() * 1000.0);
    }
    samples
}

fn percentile(samples: &mut [f64], p: usize) -> f64 {
    if samples.is_empty() {
        return 0.0;
    }
    samples.sort_by(f64::total_cmp);
    let rank = (samples.len() * p).div_ceil(100);
    samples[rank.saturating_sub(1).min(samples.len() - 1)]
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
            report.switch_p50_ms,
            report.switch_p95_ms,
            report.switch_max_ms,
            report.echo_p50_ms,
            report.echo_p95_ms
        );
        assert!(
            report.switch_p95_ms < 50.0,
            "session switch p95 {:.2} ms exceeds 50 ms",
            report.switch_p95_ms
        );
        assert!(report.ui_socket_ops == 0);
    }
}

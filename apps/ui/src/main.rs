mod actions_editor;
mod agent_ops;
mod app;
mod appearance;
mod bench;
mod branch_picker;
mod chrome;
mod code_review;
mod commit;
mod connection;
mod ds;
mod editor;
mod file_icons;
mod fuzzy;
mod glyphs;
mod handoff;
mod history;
mod keymap;
mod launch;
mod menus;
mod modal;
mod native_menus;
mod open_pr;
mod pages;
mod palette;
mod pr_detail;
mod pr_filters;
mod pr_review;
mod probe;
mod processes;
mod project_ops;
mod projects;
mod rail;
mod session_changes;
mod session_label;
mod settings;
mod shares;
mod spec_run;
mod storage;
mod sync;
mod tabs;
mod terminal;
#[cfg(test)]
mod test_support;
mod theme;
mod toasts;
mod ui;
mod updates;
mod usage;
mod workbench;

use std::path::PathBuf;
use std::sync::{Arc, OnceLock};
use std::time::Duration;

use client::Store;
use rune::{App, AppSpec, Chrome};

use app::AppState;
use connection::{Boot, Metrics, Mode};
use terminal::{SharedTerminal, stats::Stats};

static BOOT: OnceLock<Arc<Boot>> = OnceLock::new();
static INITIAL: OnceLock<Option<Store>> = OnceLock::new();
static OPEN: OnceLock<Option<String>> = OnceLock::new();

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum BenchMode {
    Switch,
    All,
    Terminal,
    Editor,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct BenchRequest {
    mode: BenchMode,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Action {
    Run,
    Bench(BenchRequest),
    Startup,
    Probe,
    Gallery,
}

struct Options {
    fixture: bool,
    fixture_path: PathBuf,
    reconnect: Option<Duration>,
    spawn_daemon: bool,
    socket: Option<PathBuf>,
    daemon_bin: Option<PathBuf>,
    action: Action,
    bench_iterations: Option<usize>,
    bench_frames: Option<usize>,
    bench_report: Option<PathBuf>,
    open: Option<String>,
    rest: Vec<String>,
}

impl Options {
    fn parse() -> Result<Options, String> {
        let mut options = Options {
            fixture: true,
            fixture_path: bench::fixture_path(),
            reconnect: Some(Duration::from_millis(2500)),
            spawn_daemon: false,
            socket: std::env::var_os("FORGE_SOCKET").map(PathBuf::from),
            daemon_bin: std::env::var_os("FORGE_DAEMON_BIN").map(PathBuf::from),
            action: Action::Run,
            bench_iterations: None,
            bench_frames: None,
            bench_report: None,
            open: None,
            rest: Vec::new(),
        };
        let mut args = std::env::args().skip(1).peekable();
        while let Some(arg) = args.next() {
            match arg.as_str() {
                "--daemon" | "--live" => options.fixture = false,
                "--fixture" => {
                    options.fixture = true;
                    if let Some(path) = args.next_if(|next| !next.starts_with("--")) {
                        options.fixture_path = PathBuf::from(path);
                    }
                }
                "--spawn-daemon" => options.spawn_daemon = true,
                "--no-reconnect" => options.reconnect = None,
                "--fixture-reconnect-ms" => {
                    let value = args.next().ok_or("--fixture-reconnect-ms needs a value")?;
                    let ms: u64 = value
                        .parse()
                        .map_err(|_| format!("invalid --fixture-reconnect-ms {value}"))?;
                    options.reconnect = Some(Duration::from_millis(ms));
                }
                "--socket" => {
                    let value = args.next().ok_or("--socket needs a path")?;
                    options.socket = Some(PathBuf::from(value));
                    options.fixture = false;
                }
                "--daemon-bin" => {
                    let value = args.next().ok_or("--daemon-bin needs a path")?;
                    options.daemon_bin = Some(PathBuf::from(value));
                }
                "--bench" => {
                    let value = args.next_if(|next| !next.starts_with("--"));
                    let mode = match value.as_deref() {
                        None | Some("switch") => BenchMode::Switch,
                        Some("all") => BenchMode::All,
                        Some("terminal") | Some("terminal-125") => BenchMode::Terminal,
                        Some("editor") => BenchMode::Editor,
                        Some(value) => {
                            let iterations: usize = value
                                .parse()
                                .map_err(|_| format!("invalid --bench {value}"))?;
                            options.bench_iterations = Some(iterations);
                            BenchMode::Switch
                        }
                    };
                    options.action = Action::Bench(BenchRequest { mode });
                }
                "--bench-iterations" => {
                    let value = args.next().ok_or("--bench-iterations needs a count")?;
                    let count: usize = value
                        .parse()
                        .map_err(|_| format!("invalid --bench-iterations {value}"))?;
                    options.bench_iterations = Some(count);
                }
                "--bench-frames" => {
                    let value = args.next().ok_or("--bench-frames needs a count")?;
                    let count: usize = value
                        .parse()
                        .map_err(|_| format!("invalid --bench-frames {value}"))?;
                    options.bench_frames = Some(count);
                }
                "--bench-report" => {
                    let value = args.next().ok_or("--bench-report needs a path")?;
                    options.bench_report = Some(PathBuf::from(value));
                }
                "--bench-startup" => options.action = Action::Startup,
                "--probe" => options.action = Action::Probe,
                "--gallery" => options.action = Action::Gallery,
                "--open" => {
                    let value = args.next().ok_or("--open needs a path")?;
                    options.open = Some(value);
                    options.fixture = false;
                }
                "--help" | "-h" => {
                    print_help();
                    std::process::exit(0);
                }
                value if value.starts_with("--fixture=") => {
                    options.fixture = true;
                    options.fixture_path = PathBuf::from(&value["--fixture=".len()..]);
                }
                other => options.rest.push(other.to_string()),
            }
        }
        Ok(options)
    }
}

#[global_allocator]
static ALLOC: terminal::alloc::Counting = terminal::alloc::Counting;

fn main() {
    terminal::shaping::prewarm_fallbacks();
    if let Err(error) = run() {
        eprintln!("forge-ui: {error}");
        std::process::exit(2);
    }
}

fn run() -> Result<(), String> {
    let options = Options::parse()?;
    let metrics = Arc::new(Metrics::default());
    metrics.note_ui_thread();

    if options.action == Action::Gallery {
        App::new(AppSpec::new(
            "Forge Node · Design system",
            (1180.0, 900.0),
            || (),
            ds::gallery::view,
        ))
        .run();
        return Ok(());
    }

    if options.action == Action::Probe {
        if options.fixture {
            return Err("--probe measures a real daemon; pass --daemon".to_string());
        }
        let socket = resolve_socket(options.socket)?;
        let report = probe::run(&socket, connection::CLIENT_VERSION)?;
        println!(
            "{}",
            serde_json::to_string_pretty(&report).map_err(|error| error.to_string())?
        );
        return Ok(());
    }

    if options.fixture {
        let store = connection::fixture_store(&options.fixture_path)?;
        match options.action {
            Action::Bench(request) => {
                let iterations = options.bench_iterations.unwrap_or(500);
                let frames = options.bench_frames.unwrap_or_else(|| match request.mode {
                    BenchMode::Terminal | BenchMode::Editor => {
                        options.bench_iterations.unwrap_or(300)
                    }
                    _ => 300,
                });
                let value = match request.mode {
                    BenchMode::Switch => json(&bench::run(store, iterations))?,
                    BenchMode::Terminal => json(&bench::terminal_125(store, frames))?,
                    BenchMode::Editor => json(&bench::editor(store, frames))?,
                    BenchMode::All => json(&bench::all(store, iterations, frames))?,
                };
                if let Some(path) = &options.bench_report {
                    bench::write_json(path, &value)?;
                }
                println!(
                    "{}",
                    serde_json::to_string_pretty(&value).map_err(|error| error.to_string())?
                );
                return Ok(());
            }
            Action::Startup => {
                let mode = Mode::Fixture {
                    snapshot: Arc::new(store.clone()),
                    reconnect_after: None,
                };
                let boot = Arc::new(Boot {
                    mode,
                    metrics: Arc::clone(&metrics),
                    terminal: SharedTerminal::new(),
                });
                return run_app(boot, Some(store), true, options.rest, true);
            }
            _ => {}
        }
        let mode = Mode::Fixture {
            snapshot: Arc::new(store.clone()),
            reconnect_after: options.reconnect,
        };
        let boot = Arc::new(Boot {
            mode,
            metrics: Arc::clone(&metrics),
            terminal: SharedTerminal::new(),
        });
        return run_app(boot, Some(store), true, options.rest, false);
    }

    if matches!(options.action, Action::Bench(_) | Action::Startup) {
        return Err("--bench and --bench-startup are fixture-only; omit --daemon".to_string());
    }
    let _ = OPEN.set(options.open);
    let socket = resolve_socket(options.socket)?;
    let mode = Mode::Live {
        socket,
        daemon: options.daemon_bin,
        spawn: options.spawn_daemon,
    };
    let boot = Arc::new(Boot {
        mode,
        metrics: Arc::clone(&metrics),
        terminal: SharedTerminal::new(),
    });
    run_app(boot, None, false, options.rest, false)
}

fn json<T: serde::Serialize>(value: &T) -> Result<serde_json::Value, String> {
    serde_json::to_value(value).map_err(|error| error.to_string())
}

fn resolve_socket(explicit: Option<PathBuf>) -> Result<PathBuf, String> {
    if let Some(path) = explicit {
        return Ok(path);
    }
    client::resolve_socket(&client::SocketQuery::from_env()).map_err(|error| error.to_string())
}

fn init() -> AppState {
    let boot = BOOT.get().expect("boot state installed before App::new");
    let initial = INITIAL.get().and_then(Option::clone);
    let fixture = matches!(boot.mode, Mode::Fixture { .. });
    let mut state = AppState::new(
        Arc::clone(&boot.metrics),
        Arc::clone(&boot.terminal),
        fixture,
        initial,
    );
    state.open_path = OPEN.get().cloned().flatten();
    state.updates.fixture = fixture;
    if fixture && state.active_row().and_then(|row| row.terminal_id).is_some() {
        state.generation = 1;
        state.status = app::LinkStatus::Connected;
        let row = state.active_row().cloned().expect("active row");
        state
            .terminal
            .activate_fixture(1, row.id, row.terminal_id.expect("terminal"), (80, 24));
        state.terminal.focus_terminal = true;
    }
    state
}

fn app_key(s: &mut AppState, ev: &rune::KeyEvent, tx: &mut rune::Tx) -> bool {
    if s.editor_chord(ev, tx) || s.global_key(ev) {
        return true;
    }
    if s.active_line_editor().is_some() {
        return s.editor_key(ev, tx);
    }
    s.terminal_key(ev, tx)
}

fn window_focus(s: &mut AppState, focused: bool, tx: &mut rune::Tx) {
    s.terminal_window_focus(focused, tx);
}

fn ime(s: &mut AppState, ev: &rune::Ime, tx: &mut rune::Tx) -> bool {
    if s.editor_ime(ev) {
        return true;
    }
    let taken = s.terminal_ime(ev, tx);
    if taken {
        eprintln!("[ime] {}", crate::terminal::input::ime_label(ev));
    }
    taken
}

fn run_app(
    boot: Arc<Boot>,
    initial: Option<Store>,
    fixture: bool,
    rest: Vec<String>,
    startup_marker: bool,
) -> Result<(), String> {
    let stats = Arc::new(std::sync::Mutex::new(Stats::with_capacity(1 << 16)));
    let _ = BOOT.set(Arc::clone(&boot));
    let _ = INITIAL.set(initial);
    let spec = AppSpec {
        poll: Some(app::poll),
        on_key: Some(app_key),
        families: (editor::metrics::SANS, editor::metrics::MONO_FAMILIES),
        ..AppSpec::new(
            if fixture {
                "Forge Node · Rune (fixture)"
            } else {
                "Forge Node · Rune"
            },
            (1180.0, 760.0),
            || unreachable!(),
            ui::view,
        )
    };
    let app = native_menus::install(App::new(spec));
    let handle = app.handle();
    native_menus::install_handle(handle.clone());
    updates::install_handle(handle.clone());
    updates::start_schedule(handle.clone(), fixture);
    let sink: connection::EventSink = {
        let handle = handle.clone();
        Arc::new(move |events| {
            let handle = handle.clone();
            handle.update(move |state, tx| state.apply_events(events, tx));
        })
    };
    let wake: connection::WakeSink = {
        let handle = handle.clone();
        Arc::new(move || handle.wake())
    };
    let images = rune::ImageCache::new(Arc::clone(&wake));
    let connection = connection::spawn(Arc::clone(&boot), sink, wake);
    handle.update(move |state, _| {
        state.connection = Some(connection);
        state.images = Some(images);
    });
    let compositor = terminal::renderer::Terminal::new(
        Arc::clone(&boot.terminal),
        stats,
        Some(Arc::clone(&boot.metrics)),
    );
    let app = app
        .init(init)
        .on_ime(ime)
        .on_focus(window_focus)
        .on_modifiers(|s: &mut AppState, mods, tx| s.terminal_modifiers(mods, tx))
        .compositor(compositor)
        .chrome(Chrome {
            full_size_content: true,
            // Centred in the title row (`ui::TITLE_H`), as in Rex.
            traffic_lights: Some((13.0, 7.0)),
            drag_regions: true,
        })
        .args(rest);
    if startup_marker {
        // The wrapper times launch → this line and samples RSS while the event loop stays up.
        let mut announced = false;
        let app = app.on_frame_encoded(move |frame| {
            if announced || frame.outcome() != rune::FrameOutcome::Drawn {
                return;
            }
            announced = true;
            let marker = serde_json::json!({
                "pid": std::process::id(),
                "seq": frame.seq(),
                "refresh_hz": frame.refresh_hz(),
                "update_ms": frame.update_ms(),
                "encode_ms": frame.encode_ms(),
            });
            eprintln!("forge-ui-first-frame {marker}");
        });
        app.try_run()
            .map_err(|error| format!("startup window: {error}"))
    } else {
        app.run();
        Ok(())
    }
}

fn print_help() {
    println!(
        "\
forge-ui — the native Rune client of Forge Node

Usage: forge-ui [OPTIONS]

Options:
  --fixture [PATH]          run from a recorded snapshot (default fixtures/snapshot.json)
  --daemon                  connect to a real forge-daemon (default socket rules)
  --socket PATH             daemon socket; implies --daemon
  --spawn-daemon            start FORGE_DAEMON_BIN when the socket is absent
  --daemon-bin PATH         forge-daemon binary for --spawn-daemon
  --fixture-reconnect-ms N  simulate a fixture disconnect after N ms (default 2500)
  --no-reconnect            keep the fixture connection up
  --probe                   connect, snapshot, add a project and attach a terminal
  --gallery                 every design-system piece in a dark and a light base
  --open PATH               open PATH (relative to the first workspace) in an editor; implies --daemon
  --bench [N|switch|all|terminal-125|editor]
                            headless measurements; `all` runs every section
  --bench-iterations N      session-switch iterations (default 500)
  --bench-frames N          terminal/editor frames at 125 Hz (default 300)
  --bench-report PATH       also write the benchmark JSON to PATH
  --bench-startup           run the fixture app and mark the first drawn frame
  -h, --help                this help

Rune's own --snap/--tree/--script/--trace flags pass through.

Examples:
  forge-ui
  forge-ui --daemon --socket /tmp/forge-ui/daemon.sock
  forge-ui --fixture --bench 300
  forge-ui --fixture --bench all --bench-report /tmp/perf.json
  forge-ui --fixture --script scripts/session-switch.json --tree /tmp/tree.json --at 4
  forge-ui --fixture --snap /tmp/forge-ui.png --at 1"
    );
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::connection::fixture_store;
    use rune::render::KIND_EFFECT;
    use rune::{Env, Runtime};

    fn runtime_with_shared() -> (Runtime<AppState>, Arc<SharedTerminal>) {
        let store = fixture_store(&bench::fixture_path()).expect("fixture snapshot decodes");
        let metrics = Arc::new(Metrics::default());
        metrics.note_ui_thread();
        let shared = SharedTerminal::new();
        let mut state = AppState::for_bench(store, metrics, Arc::clone(&shared));
        let row = state.active_row().cloned().expect("active row");
        state
            .terminal
            .activate_fixture(1, row.id, row.terminal_id.expect("terminal"), (80, 24));
        state.terminal.focus_terminal = true;
        shared.set_measured(80, 24);
        let mut rt = Runtime::new(
            state,
            ui::view,
            (1180.0, 760.0),
            Env {
                system_dark: true,
                scale: 2.0,
            },
        );
        rt.poll = Some(app::poll);
        rt.frame(0.0);
        (rt, shared)
    }

    fn runtime() -> Runtime<AppState> {
        runtime_with_shared().0
    }

    fn runtime_sized(width: f32, height: f32) -> Runtime<AppState> {
        let store = fixture_store(&bench::fixture_path()).expect("fixture snapshot decodes");
        let metrics = Arc::new(Metrics::default());
        metrics.note_ui_thread();
        let shared = SharedTerminal::new();
        let mut state = AppState::for_bench(store, metrics, Arc::clone(&shared));
        let row = state.active_row().cloned().expect("active row");
        state
            .terminal
            .activate_fixture(1, row.id, row.terminal_id.expect("terminal"), (80, 24));
        state.terminal.focus_terminal = true;
        shared.set_measured(80, 24);
        let mut rt = Runtime::new(
            state,
            ui::view,
            (width, height),
            Env {
                system_dark: true,
                scale: 2.0,
            },
        );
        rt.poll = Some(app::poll);
        rt.frame(0.0);
        rt.frame(1.0 / 60.0);
        rt
    }

    fn places(count: usize) -> Vec<crate::palette::Entry> {
        (0..count)
            .map(|index| crate::palette::Entry::Session {
                id: domain::SessionId::new(),
                label: format!("Place {index}"),
                note: "main".into(),
                agent: None,
                kind: domain::SessionKind::Shell,
                waiting: false,
            })
            .collect()
    }

    fn test_gpu() -> Option<(
        rune::render::Gpu,
        rune::render::Offscreen,
        rune::render::Renderer,
    )> {
        use rune::Compositor;
        let mut descriptor = wgpu::InstanceDescriptor::new_without_display_handle();
        descriptor.backends = wgpu::Backends::all();
        descriptor
            .flags
            .remove(wgpu::InstanceFlags::VALIDATION_INDIRECT_CALL);
        let probe = terminal::renderer::Terminal::new(
            SharedTerminal::new(),
            Arc::new(std::sync::Mutex::new(terminal::stats::Stats::default())),
            None,
        );
        let gpu = rune::render::Gpu::try_with_features(
            wgpu::Instance::new(descriptor),
            None,
            probe.features(),
        )
        .ok()?;
        let target = rune::render::Offscreen::new(&gpu.device, 2360, 1520);
        let renderer = rune::render::Renderer::new(&gpu.device, rune::render::OFFSCREEN_FORMAT);
        Some((gpu, target, renderer))
    }

    #[test]
    fn a_line_editor_draws_its_rows_and_the_composition_at_the_caret() {
        let mut rt = runtime();
        rt.update(|s, _| {
            s.editor_surface = client::EditorSurface::Dom;
            let id = s.rows[0].id;
            let first = &mut s.rows_mut()[0];
            first.kind = domain::SessionKind::Editor;
            first.terminal_id = None;
            s.active = Some(id);
            s.editor.set_session(Some(id));
            s.editor.scroll.set([0.0, 400.0]);
            let lines = [(0, "fn main() {"), (1, "}")];
            let _ = s
                .editor
                .apply_frame(id, editor::tests::frame(0, &lines, 2, (0, 0)));
            s.editor.preedit = "にほ".into();
        });
        rt.frame(0.1);
        rt.frame(0.2);
        let rect = |key: &str| {
            rt.elements()
                .into_iter()
                .find(|element| !element.exiting && element.key.as_deref() == Some(key))
                .map(|element| element.rect)
        };
        assert!(rect("row:0").is_some() && rect("row:1").is_some());
        let caret = rect("caret").expect("the caret");
        let preedit = rect("editor-preedit").expect("the composition");
        assert!(
            (preedit[0] - caret[0]).abs() < 1.0,
            "{preedit:?} vs {caret:?}"
        );
        assert!((preedit[1] - (caret[1] - 1.0)).abs() < 1.0);
    }

    #[test]
    fn one_terminal_pane_is_one_effect_slot() {
        let rt = runtime();
        let slots = rt
            .instances
            .iter()
            .filter(|instance| instance.params[1] == KIND_EFFECT)
            .count();
        assert_eq!(slots, 1, "one terminal pane means one effect slot");
    }

    #[test]
    fn a_sidebar_click_moves_keyboard_input_to_the_terminal() {
        let mut rt = runtime();
        let (x, y) = rt.find("session-row-2").expect("session row is laid out");
        rt.pointer_down(x, y);
        rt.pointer_up(x, y);
        rt.frame(1.0 / 60.0);
        rt.key(rune::KeyEvent {
            key: rune::Key::Text("z".to_string()),
            mods: rune::Mods::default(),
        });
        rt.frame(1.0 / 60.0);
        assert_eq!(
            rt.state.last_focus.as_deref(),
            Some("terminal-focus-proxy"),
            "the row is select-only; the press focuses the terminal proxy"
        );
        let (_, summary) = rt.state.last_input.as_ref().expect("bytes were sent");
        assert!(
            summary.contains("\"z\""),
            "typed byte reached the terminal: {summary}"
        );
    }

    #[test]
    fn the_terminal_cursor_owns_the_input_method() {
        let rt = runtime();
        let cursor = rt.state.terminal.cursor().expect("terminal grid");
        let (rect, _) = rt
            .presented("terminal-pane")
            .expect("terminal pane is drawn");
        let expected = [
            rect[0] + cursor.col as f32 * terminal::renderer::CELL_PT.0,
            rect[1] + cursor.line as f32 * terminal::renderer::CELL_PT.1,
            terminal::renderer::CELL_PT.0,
            terminal::renderer::CELL_PT.1,
        ];
        assert_eq!(rt.ime_area, Some(expected));
    }

    fn commands(rx: &flume::Receiver<connection::Command>) -> Vec<connection::Command> {
        let mut out = Vec::new();
        while let Ok(command) = rx.try_recv() {
            out.push(command);
        }
        out
    }

    fn editor_runtime() -> (
        Runtime<AppState>,
        flume::Receiver<connection::Command>,
        String,
    ) {
        let (mut rt, _) = runtime_with_shared();
        let (handle, rx) = connection::ConnectionHandle::test_handle(Arc::new(Metrics::default()));
        rt.update(|s, _| {
            s.connection = Some(handle);
            s.editor_surface = client::EditorSurface::Dom;
            let id = s.rows[0].id;
            let id_text = s.rows[0].id_text.clone();
            let first = &mut s.rows_mut()[0];
            first.kind = domain::SessionKind::Editor;
            first.terminal_id = None;
            s.active = Some(id);
            s.active_text = Some(id_text);
            s.editor.set_session(Some(id));
            s.editor.scroll.set([0.0, 400.0]);
            let lines = [
                (0, "fn main() {"),
                (1, "    body();"),
                (2, "}"),
                (3, "tail"),
            ];
            let _ = s
                .editor
                .apply_frame(id, editor::tests::frame(0, &lines, 4, (0, 0)));
        });
        rt.frame(0.1);
        let id_text = rt.state.rows[0].id_text.clone();
        (rt, rx, id_text)
    }

    /// Selects a session the way a person does now that there is no tab strip:
    /// its row in the sidebar.
    fn click_session(rt: &mut Runtime<AppState>, id_text: &str) {
        let index = rt
            .state
            .rows
            .iter()
            .position(|row| row.id_text == id_text)
            .expect("the session has a row");
        let (x, y) = rt
            .find(&format!("session-row-{index}"))
            .expect("its row is laid out");
        rt.pointer_down(x, y);
        rt.pointer_up(x, y);
        rt.frame(1.0 / 60.0);
    }

    #[test]
    fn paste_reaches_the_terminal() {
        let mut rt = runtime();
        rt.paste("x");
        rt.frame(1.0 / 60.0);
        let (_, summary) = rt.state.last_input.as_ref().expect("bytes were sent");
        assert!(
            summary.contains('x') && summary.contains("paste"),
            "terminal got {summary}"
        );
    }

    #[test]
    fn a_pasted_image_reaches_the_terminal_as_a_path() {
        let mut rt = runtime();
        let png = [
            &b"\x89PNG\r\n\x1a\n\0\0\0\rIHDR"[..],
            &[0, 0, 0, 2, 0, 0, 0, 3],
        ]
        .concat();
        let image = rune::ClipboardImage::from_png(png).expect("a PNG header");
        rt.paste_content(rune::ClipboardContent::new("").with_image(image));
        rt.frame(1.0 / 60.0);
        let (_, summary) = rt.state.last_input.as_ref().expect("bytes were sent");
        assert!(summary.contains("paste"), "terminal got {summary}");
    }

    #[test]
    fn paste_reaches_the_editor() {
        use domain::EditorInputEvent;
        let (mut rt, rx, _) = editor_runtime();
        let _ = commands(&rx);
        rt.paste("x");
        rt.frame(1.0 / 60.0);
        assert!(
            commands(&rx).iter().any(|command| matches!(
                command,
                connection::Command::EditorInput { events, .. }
                    if events == &[EditorInputEvent::Text("x".into())]
            )),
            "the editor received the paste"
        );
    }

    #[test]
    fn a_tab_click_then_shift_left_reaches_the_editor() {
        use domain::{EditorInputEvent, EditorKey, editor_modifiers};
        let (mut rt, rx, id_text) = editor_runtime();
        click_session(&mut rt, &id_text);
        let _ = commands(&rx);
        rt.key(rune::KeyEvent {
            key: rune::Key::Left,
            mods: rune::Mods {
                shift: true,
                ..rune::Mods::default()
            },
        });
        rt.frame(1.0 / 60.0);
        assert!(
            commands(&rx).iter().any(|command| matches!(
                command,
                connection::Command::EditorInput { events, .. }
                    if events == &[EditorInputEvent::Key {
                        key: EditorKey::Left,
                        modifiers: editor_modifiers::SHIFT,
                    }]
            )),
            "the editor got the shifted key"
        );
        assert_eq!(
            rt.state.terminal.attach.held_len(),
            0,
            "nothing was held for a terminal"
        );
    }

    #[test]
    fn shift_command_g_finds_previous_in_the_editor_instead_of_opening_git() {
        use domain::{EditorInputEvent, EditorKey, editor_modifiers};
        let (mut rt, rx, id_text) = editor_runtime();
        click_session(&mut rt, &id_text);
        let _ = commands(&rx);
        let panel = rt.state.workbench.panel;
        rt.key(rune::KeyEvent {
            key: rune::Key::Command('g'),
            mods: rune::Mods {
                cmd: true,
                shift: true,
                ..rune::Mods::default()
            },
        });
        rt.frame(1.0 / 60.0);
        assert!(
            commands(&rx).iter().any(|command| matches!(
                command,
                connection::Command::EditorInput { events, .. }
                    if events == &[EditorInputEvent::Key {
                        key: EditorKey::Char('b'),
                        modifiers: editor_modifiers::CONTROL,
                    }]
            )),
            "find-previous reached the editor"
        );
        assert_eq!(rt.state.workbench.panel, panel, "the Git panel stayed shut");
    }

    #[test]
    fn dragging_past_a_row_reaches_the_next_line() {
        use domain::EditorInputEvent;
        let (mut rt, rx, _) = editor_runtime();
        let (rect, _) = rt.presented("row:0").expect("row 0 is drawn");
        let x = rect[0] + 120.0;
        let y = rect[1] + 2.0;
        rt.pointer_down(x, y);
        rt.frame(1.0 / 60.0);
        rt.pointer_move(x, y + 20.0);
        rt.frame(1.0 / 60.0);
        rt.pointer_up(x, y + 20.0);
        rt.frame(1.0 / 60.0);
        let last = commands(&rx)
            .into_iter()
            .filter_map(|command| match command {
                connection::Command::EditorInput { events, .. } => {
                    events.into_iter().find_map(|event| match event {
                        EditorInputEvent::Pointer { line, .. } => Some(line),
                        _ => None,
                    })
                }
                _ => None,
            })
            .next_back();
        assert_eq!(last, Some(1), "the drag ended on the second row");
    }

    #[test]
    fn a_shift_click_carries_the_shift_modifier() {
        use domain::{EditorInputEvent, editor_modifiers};
        let (mut rt, rx, _) = editor_runtime();
        let (rect, _) = rt.presented("row:1").expect("row 1 is drawn");
        rt.physical_input(rune::PhysicalInput::Modifiers(rune::Mods {
            shift: true,
            ..rune::Mods::default()
        }));
        rt.pointer_down(rect[0] + 120.0, rect[1] + 2.0);
        rt.frame(1.0 / 60.0);
        rt.frame(1.0 / 60.0);
        rt.physical_input(rune::PhysicalInput::Modifiers(rune::Mods::default()));
        let shift = commands(&rx).into_iter().find_map(|command| match command {
            connection::Command::EditorInput { events, .. } => {
                events.into_iter().find_map(|event| match event {
                    EditorInputEvent::Pointer { modifiers, .. } => Some(modifiers),
                    _ => None,
                })
            }
            _ => None,
        });
        assert_eq!(shift, Some(editor_modifiers::SHIFT));
    }

    #[test]
    fn the_palette_scrolls_inside_its_dialog() {
        let mut rt = runtime_sized(1000.0, 600.0);
        rt.update(|s, _| {
            s.palette.show(crate::palette::Scope::Places);
            s.palette.set_places(places(40));
        });
        rt.frame(1.0 / 60.0);
        rt.frame(1.0 / 60.0);
        let (dialog, _) = rt.presented("palette-dialog").expect("the palette dialog");
        let (scroll, _) = rt
            .presented("palette-scroll")
            .expect("the palette viewport");
        assert!(
            scroll[0] >= dialog[0] - 0.5
                && scroll[1] >= dialog[1] - 0.5
                && scroll[0] + scroll[2] <= dialog[0] + dialog[2] + 0.5
                && scroll[1] + scroll[3] <= dialog[1] + dialog[3] + 0.5,
            "scroll {scroll:?} leaves dialog {dialog:?}"
        );
    }

    #[test]
    fn a_palette_hint_stays_inside_its_row() {
        let mut rt = runtime_sized(1000.0, 600.0);
        rt.update(|s, _| s.palette.show(crate::palette::Scope::Commands));
        rt.frame(1.0 / 60.0);
        rt.frame(1.0 / 60.0);
        let (row, _) = rt.presented("palette-0").expect("the first row");
        let (hint, _) = rt.presented("palette-hint-0").expect("its chord");
        assert!(
            hint[0] >= row[0] - 0.5 && hint[0] + hint[2] <= row[0] + row[2] + 0.5,
            "hint {hint:?} leaves row {row:?}"
        );
    }

    #[test]
    fn the_palette_reveals_a_selected_row_past_the_first_screen() {
        let mut rt = runtime_sized(1000.0, 600.0);
        rt.update(|s, _| {
            s.palette.show(crate::palette::Scope::Places);
            s.palette.set_places(places(40));
        });
        rt.frame(1.0 / 60.0);
        for _ in 0..35 {
            rt.key(rune::KeyEvent {
                key: rune::Key::Down,
                mods: rune::Mods::default(),
            });
        }
        rt.frame(1.0 / 60.0);
        rt.frame(0.5);
        rt.frame(0.5);
        let (scroll, _) = rt
            .presented("palette-scroll")
            .expect("the palette viewport");
        let (row, _) = rt
            .presented("palette-35")
            .expect("the selected row is built");
        assert!(
            row[1] >= scroll[1] - 0.5 && row[1] + row[3] <= scroll[1] + scroll[3] + 0.5,
            "row {row:?} is outside viewport {scroll:?}"
        );
    }

    #[test]
    fn the_shared_files_footer_stays_inside_its_dialog() {
        let mut rt = runtime_sized(1000.0, 600.0);
        let project = rt
            .state
            .store
            .as_ref()
            .and_then(|store| store.projects.first())
            .map(|project| project.id)
            .expect("a fixture project");
        rt.update(|s, _| {
            s.projects.share_form.target = Some((project, None));
        });
        rt.frame(1.0 / 60.0);
        rt.frame(1.0 / 60.0);
        let (dialog, _) = rt
            .presented("shared-files-dialog")
            .expect("the shared files dialog");
        let (done, _) = rt.presented("shared-files-done").expect("the Done button");
        assert!(
            done[1] >= dialog[1] - 0.5 && done[1] + done[3] <= dialog[1] + dialog[3] + 0.5,
            "Done {done:?} leaves dialog {dialog:?}"
        );
    }

    fn layered_view(s: &AppState, cx: &mut rune::Cx<AppState>) -> rune::El {
        ui::view(s, cx).opacity(0.9)
    }

    fn render_frames(
        rt: &mut Runtime<AppState>,
        compositor: &mut terminal::renderer::Terminal,
        gpu: &rune::render::Gpu,
        target: &rune::render::Offscreen,
        renderer: &mut rune::render::Renderer,
        frames: usize,
    ) {
        for _ in 0..frames {
            bench::render(gpu, target, renderer, rt, compositor, 1.0 / 60.0)
                .expect("the frame draws");
        }
    }

    fn compositor(shared: &Arc<SharedTerminal>) -> terminal::renderer::Terminal {
        terminal::renderer::Terminal::new(
            Arc::clone(shared),
            Arc::new(std::sync::Mutex::new(terminal::stats::Stats::default())),
            None,
        )
    }

    #[test]
    fn settings_round_trip_keeps_the_terminal_pipeline_valid() {
        let Some((gpu, target, mut renderer)) = test_gpu() else {
            return;
        };
        let (mut rt, shared) = runtime_with_shared();
        let mut terminal = compositor(&shared);
        let scope = gpu.device.push_error_scope(wgpu::ErrorFilter::Validation);
        render_frames(&mut rt, &mut terminal, &gpu, &target, &mut renderer, 2);
        rt.update(|s, _| s.open_settings(None));
        render_frames(&mut rt, &mut terminal, &gpu, &target, &mut renderer, 3);
        rt.update(|s, _| s.close_settings());
        render_frames(&mut rt, &mut terminal, &gpu, &target, &mut renderer, 3);
        let error = pollster::block_on(scope.pop());
        assert!(error.is_none(), "terminal pipeline mismatch: {error:?}");
    }

    #[test]
    fn the_terminal_draws_inside_an_isolated_layer() {
        let Some((gpu, target, mut renderer)) = test_gpu() else {
            return;
        };
        let store = fixture_store(&bench::fixture_path()).expect("fixture snapshot decodes");
        let metrics = Arc::new(Metrics::default());
        metrics.note_ui_thread();
        let shared = SharedTerminal::new();
        let mut state = AppState::for_bench(store, metrics, Arc::clone(&shared));
        let row = state.active_row().cloned().expect("active row");
        state
            .terminal
            .activate_fixture(1, row.id, row.terminal_id.expect("terminal"), (80, 24));
        state.terminal.focus_terminal = true;
        shared.set_measured(80, 24);
        let mut rt = Runtime::new(
            state,
            layered_view,
            (1180.0, 760.0),
            Env {
                system_dark: true,
                scale: 2.0,
            },
        );
        rt.poll = Some(app::poll);
        let mut terminal = compositor(&shared);
        let scope = gpu.device.push_error_scope(wgpu::ErrorFilter::Validation);
        render_frames(&mut rt, &mut terminal, &gpu, &target, &mut renderer, 3);
        let error = pollster::block_on(scope.pop());
        assert!(error.is_none(), "terminal inside a layer: {error:?}");
    }
}

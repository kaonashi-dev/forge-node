mod app;
mod appearance;
mod bench;
mod chrome;
mod commit;
mod connection;
mod editor;
mod file_icons;
mod fuzzy;
mod glyphs;
mod history;
mod keymap;
mod launch;
mod palette;
mod pr_review;
mod probe;
mod processes;
mod projects;
mod session_label;
mod settings;
mod storage;
mod sync;
mod terminal;
#[cfg(test)]
mod test_support;
mod theme;
mod ui;
mod usage;
mod workbench;

use std::path::PathBuf;
use std::sync::{Arc, OnceLock};
use std::time::Duration;

use client::Store;
use rune::{App, AppSpec, Chrome, Key};

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
    if fixture && state.active_row().and_then(|row| row.terminal_id).is_some() {
        state.generation = 1;
        state.status = app::LinkStatus::Connected;
        let row = state.active_row().cloned().expect("active row");
        state
            .terminal
            .activate_fixture(1, row.id, row.terminal_id.expect("terminal"), (80, 24));
        state.terminal.focus_terminal = true;
        state.terminal.interacted = true;
    }
    state
}

fn app_key(s: &mut AppState, ev: &rune::KeyEvent, tx: &mut rune::Tx) -> bool {
    if s.global_key(ev) {
        return true;
    }
    if ev.mods.cmd {
        match &ev.key {
            Key::Command('n') => {
                s.request_create();
                return true;
            }
            Key::Command('r') if ev.mods.shift => {
                s.request_reconnect();
                return true;
            }
            Key::Command('r') => {
                s.request_refresh();
                return true;
            }
            _ => {}
        }
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
        eprintln!("[ime] {ev:?}");
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
    let app = App::new(spec);
    let handle = app.handle();
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
    let connection = connection::spawn(Arc::clone(&boot), sink, wake);
    handle.update(move |state, _| state.connection = Some(connection));
    let compositor = terminal::renderer::Terminal::new(Arc::clone(&boot.terminal), stats);
    let app = app
        .init(init)
        .on_ime(ime)
        .on_focus(window_focus)
        .compositor(compositor)
        .chrome(Chrome {
            full_size_content: true,
            traffic_lights: Some((16.0, 25.0)),
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
forge-ui — the Rune client, built beside the Tauri app

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

    fn runtime() -> Runtime<AppState> {
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
        state.terminal.interacted = true;
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
        rt
    }

    #[test]
    fn a_line_editor_draws_its_rows_and_the_composition_at_the_caret() {
        let mut rt = runtime();
        rt.update(|s, _| {
            s.editor_surface = protocol::EditorSurface::Dom;
            let id = s.rows[0].id;
            s.rows[0].kind = domain::SessionKind::Editor;
            s.rows[0].terminal_id = None;
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
    fn a_tab_click_then_typing_still_reaches_the_terminal() {
        let mut rt = runtime();
        let id = rt.state.rows[2].id_text.clone();
        let (x, y) = rt.find(&format!("t-{id}")).expect("tab is laid out");
        rt.pointer_down(x, y);
        rt.pointer_up(x, y);
        rt.frame(1.0 / 60.0);
        rt.key(rune::KeyEvent {
            key: rune::Key::Text("q".to_string()),
            mods: rune::Mods::default(),
        });
        rt.frame(1.0 / 60.0);
        let (_, summary) = rt.state.last_input.as_ref().expect("bytes were sent");
        assert!(
            summary.contains("\"q\""),
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
}

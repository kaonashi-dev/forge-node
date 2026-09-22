//! `forge-editor`: the standalone terminal editor.
//!
//! The binary is the adapter: arguments, a bounded read, raw mode, and the
//! event loop. Editing decisions live in `editor-core`, key bindings in `app`,
//! and disk access in `disk` — running this must never require the daemon, the
//! GUI, a network or Node.
//!
//! With `--control <socket>`, the daemon owns checkout I/O and supplies the
//! initial text; the editor owns the live document and requests saves over
//! that socket. Adding `--headless` swaps the sink: the
//! same `App` publishes a window of lines instead of painting cells, so the
//! GUI's DOM surface and the terminal are one editor with two outputs.

mod app;
mod cli;
mod control;
mod disk;
mod frame;
mod input;
mod render;
mod screen;
mod view;

use std::process::ExitCode;
use std::thread;

use crossterm::event::{self, Event, KeyEventKind};
use editor_control::{
    DaemonMessage, EditorMessage, EditorStateWire, MAX_INPUT_EVENTS, MAX_VIEW_ROWS,
};
use editor_core::Document;

use crate::app::App;
use crate::control::{ControlChannel, Incoming};

/// How many terminal events may queue while a control request is applied.
/// Bounded so a stuck main loop is backpressure, not unbounded growth.
const EVENT_QUEUE: usize = 64;

/// Floor between two paints while input is still arriving.
///
/// The same 8 ms `daemon::terminal::FRAME` holds an attached terminal to: a
/// paste or a key repeat that lands ten events in one millisecond is one frame
/// on the wire, not ten, and the delta rung is priced per frame either way.
const FRAME: std::time::Duration = std::time::Duration::from_millis(8);

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    match cli::parse(&args) {
        Ok(cli::Command::Help) => {
            print!("{}", cli::USAGE);
            ExitCode::SUCCESS
        }
        Ok(cli::Command::Version) => {
            println!("forge-editor {}", env!("CARGO_PKG_VERSION"));
            ExitCode::SUCCESS
        }
        Ok(cli::Command::Edit(options)) => match run_mode(options) {
            Ok(()) => ExitCode::SUCCESS,
            Err(error) => {
                eprintln!("forge-editor: {error:#}");
                ExitCode::FAILURE
            }
        },
        Err(message) => {
            eprintln!("forge-editor: {message}\n\n{}", cli::USAGE);
            ExitCode::from(2)
        }
    }
}

/// The two inputs the loop waits on: a crossterm event, or the daemon.
enum Input {
    /// `None`: the terminal event source is gone (the reader thread failed).
    Terminal(Option<Event>),
    Control(Incoming),
    /// The autosave pause elapsed with nothing else to do.
    Tick,
}

/// Paint, or publish. The two modes share `open` and every handler below.
fn run_mode(options: cli::Options) -> anyhow::Result<()> {
    if options.headless {
        run_headless(options)
    } else {
        run(options)
    }
}

/// The headless host: no raw mode, no tty, no `render::draw`.
///
/// The one wait is the control channel and the autosave pause, so a session
/// nobody is typing in costs nothing. A frame goes out after the incoming
/// burst is applied, under the same [`FRAME`] floor the painting loop uses —
/// the emit rung is per frame either way, and the GUI's scroll container is
/// what moves between them.
fn run_headless(options: cli::Options) -> anyhow::Result<()> {
    let (mut app, control, buffer_id) = open(options)?;
    let mut control = control.expect("--headless implies --control");
    let mut last_state: Option<EditorStateWire> = None;
    let mut last_frame: Option<editor_control::ViewFrame> = None;
    publish_state(&app, &mut control, &mut last_state)?;
    publish_frame(&mut app, &mut control, buffer_id, &mut last_frame)?;
    let mut published = std::time::Instant::now();
    loop {
        let incoming = control.incoming().clone();
        let message = match app.autosave_deadline() {
            Some(at) => match incoming.recv_deadline(at) {
                Ok(message) => Some(message),
                Err(flume::RecvTimeoutError::Timeout) => None,
                Err(flume::RecvTimeoutError::Disconnected) => break,
            },
            None => match incoming.recv() {
                Ok(message) => Some(message),
                Err(_) => break,
            },
        };
        match message {
            Some(Incoming::Message(message)) => handle_control(&mut app, &mut control, message)?,
            Some(Incoming::Closed { reason }) => {
                anyhow::bail!("daemon control channel closed: {reason}")
            }
            None => app.autosave_if_due(),
        }
        // Drain what is already queued before publishing: a held arrow key is
        // one frame on the wire, not one per repeat.
        while let Ok(queued) = incoming.try_recv() {
            match queued {
                Incoming::Message(message) => handle_control(&mut app, &mut control, message)?,
                Incoming::Closed { reason } => {
                    anyhow::bail!("daemon control channel closed: {reason}")
                }
            }
        }
        if app.should_quit() {
            let _ = control.send(&EditorMessage::Closed {
                reason: "quit".to_string(),
            });
            break;
        }
        flush_save(&mut app, &mut control)?;
        flush_lookups(&mut app, &mut control)?;
        publish_state(&app, &mut control, &mut last_state)?;
        if incoming.is_empty() || published.elapsed() >= FRAME {
            publish_frame(&mut app, &mut control, buffer_id, &mut last_frame)?;
            published = std::time::Instant::now();
        }
    }
    Ok(())
}

fn run(options: cli::Options) -> anyhow::Result<()> {
    let (mut app, mut control, _) = open(options)?;
    let screen = screen::Screen::enter()?;
    let (width, height) = screen.size()?;
    app.resize(width, height);

    // One thread owns crossterm's process-global event source; the main loop
    // blocks on both it and the control channel, never on a poll loop.
    let (events_tx, events_rx) = flume::bounded(EVENT_QUEUE);
    // A read error is the tty going away, which ends the thread the same way a
    // closed channel does: there is nobody left to send to either way.
    thread::spawn(move || {
        while let Ok(event) = event::read() {
            if events_tx.send(event).is_err() {
                break;
            }
        }
    });

    let mut last_state: Option<EditorStateWire> = None;
    if let Some(channel) = control.as_mut() {
        publish_state(&app, channel, &mut last_state)?;
    }

    let out = std::io::stdout();
    // First frame before we block: the alternate screen is empty until we paint.
    render::draw(&mut app, &mut out.lock())?;
    let mut painted = std::time::Instant::now();
    let result = 'run: loop {
        // Prefer work already in the queue so a wheel burst is drained before
        // the next paint. Painting *before* the wait left a race: one scroll
        // event, empty queue for a moment, full-viewport frame, repeat — the
        // flicker that survived DEC 2026 and the row-padding fix.
        let incoming = control.as_ref().map(|channel| channel.incoming().clone());
        let input = match events_rx.try_recv() {
            Ok(event) => Input::Terminal(Some(event)),
            Err(flume::TryRecvError::Disconnected) => break Ok(()),
            Err(flume::TryRecvError::Empty) => match &incoming {
                Some(incoming) => {
                    let selector = flume::Selector::new()
                        .recv(&events_rx, |result| Input::Terminal(result.ok()))
                        .recv(incoming, |result| {
                            Input::Control(result.unwrap_or(Incoming::Closed {
                                reason: "disconnected".to_string(),
                            }))
                        });
                    // Waiting *on* the deadline rather than polling for it: the
                    // loop still blocks until something happens, and the pause
                    // is what wakes it. A bare sleep-and-check here would be
                    // the defect `docs/performance.md` names.
                    match app.autosave_deadline() {
                        Some(at) => match selector.wait_deadline(at) {
                            Ok(input) => input,
                            Err(_) => Input::Tick,
                        },
                        None => selector.wait(),
                    }
                }
                None => match events_rx.recv() {
                    Ok(event) => Input::Terminal(Some(event)),
                    Err(_) => break Ok(()),
                },
            },
        };
        if !apply_input(&mut app, control.as_mut(), input)? {
            break Ok(());
        }
        while let Ok(event) = events_rx.try_recv() {
            if !apply_input(&mut app, control.as_mut(), Input::Terminal(Some(event)))? {
                break 'run Ok(());
            }
        }
        if app.should_quit() {
            if let Some(channel) = control.as_mut() {
                let _ = channel.send(&EditorMessage::Closed {
                    reason: "quit".to_string(),
                });
            }
            break Ok(());
        }
        if let Some(channel) = control.as_mut() {
            // The save goes out before the state, so the daemon's write and the
            // `dirty` the GUI paints cannot arrive in the wrong order.
            flush_save(&mut app, channel)?;
            flush_lookups(&mut app, channel)?;
            publish_state(&app, channel, &mut last_state)?;
        }
        // Paint after the burst is applied. Damage accumulates across the
        // drained events; the floor still caps a sustained stream to ≤125/s.
        if events_rx.is_empty() || painted.elapsed() >= FRAME {
            if let Err(error) = render::draw(&mut app, &mut out.lock()) {
                break Err(error.into());
            }
            painted = std::time::Instant::now();
        }
    };
    // Restore the caller's screen before any error is printed on it.
    drop(screen);
    result
}

/// Apply one wake. `false` means the terminal event source is gone.
fn apply_input(
    app: &mut App,
    control: Option<&mut ControlChannel>,
    input: Input,
) -> anyhow::Result<bool> {
    match input {
        Input::Terminal(Some(Event::Key(key))) if key.kind != KeyEventKind::Release => {
            app.handle_key(key);
        }
        Input::Terminal(Some(Event::Resize(width, height))) => app.resize(width, height),
        Input::Terminal(Some(Event::Paste(text))) => app.paste(&text),
        Input::Terminal(Some(Event::Mouse(mouse))) => app.handle_mouse(mouse),
        Input::Terminal(Some(_)) => {}
        Input::Terminal(None) => return Ok(false),
        Input::Control(Incoming::Message(message)) => {
            if let Some(channel) = control {
                handle_control(app, channel, message)?;
            }
        }
        Input::Tick => app.autosave_if_due(),
        Input::Control(Incoming::Closed { reason }) => {
            return Err(anyhow::anyhow!("daemon control channel closed: {reason}"));
        }
    }
    Ok(true)
}

/// Build the app for the mode the arguments name.
///
/// Standalone reads the file through `disk`; integrated hands the buffer to the
/// daemon and never resolves the path on disk.
fn open(options: cli::Options) -> anyhow::Result<(App, Option<ControlChannel>, u64)> {
    if let Some(socket) = &options.control {
        let (mut channel, opened) = ControlChannel::connect(socket)?;
        let document = Document::from_bytes(opened.text.as_bytes(), opened.read_only)
            .map_err(|error| anyhow::anyhow!("{}: {error}", opened.path))?;
        let mut app = App::new(
            document,
            opened.path.clone().into(),
            opened.revision.clone(),
        );
        // The daemon owns the checkout: this editor never writes it, and a
        // save travels as a request the daemon answers with a revision.
        if options.headless {
            app.set_headless();
        } else {
            app.set_integrated();
        }
        app.set_autosave(opened.autosave);
        let line = opened.line.map(|value| value as usize).or(options.line);
        if let Some(line) = line {
            app.reveal(line, None);
        }
        channel.send(&EditorMessage::Opened {
            request_id: opened.request_id,
            document_version: app.document().version().0,
        })?;
        return Ok((app, Some(channel), opened.buffer_id));
    }

    let loaded = disk::load(&options.path)
        .map_err(|error| anyhow::anyhow!("could not read {}: {error}", options.path.display()))?;
    let document = Document::from_bytes(&loaded.bytes, options.read_only)
        .map_err(|error| anyhow::anyhow!("{}: {error}", options.path.display()))?;

    let mut app = App::new(document, options.path, Some(loaded.revision));
    if let Some(line) = options.line {
        app.goto_line(line);
    }
    Ok((app, None, 0))
}

/// Answer the daemon's request on the serialized event thread.
fn handle_control(
    app: &mut App,
    control: &mut ControlChannel,
    message: DaemonMessage,
) -> anyhow::Result<()> {
    match message {
        DaemonMessage::Retarget { path } => app.retarget(path.into()),
        DaemonMessage::Reveal {
            request_id,
            line,
            column,
        } => {
            app.reveal(line as usize, column.map(|value| value as usize));
            control.send(&EditorMessage::Revealed { request_id })?;
        }
        DaemonMessage::GetState { request_id } => {
            control.send(&EditorMessage::State {
                request_id: Some(request_id),
                state: app.wire_state(),
            })?;
        }
        DaemonMessage::Saved {
            request_id,
            revision,
        } => app.save_confirmed(request_id, revision),
        DaemonMessage::Reload {
            request_id,
            text,
            revision,
        } => match app.reload(&text, revision) {
            Ok(document_version) => control.send(&EditorMessage::Applied {
                request_id,
                document_version,
            })?,
            Err(reason) => control.send(&EditorMessage::Refused { request_id, reason })?,
        },
        DaemonMessage::GitMarks { marks, .. } => app.set_marks(&marks),
        DaemonMessage::Definitions { symbol, places, .. } => {
            app.definitions_arrived(symbol, places);
        }
        DaemonMessage::Diagnostics { command, items, .. } => {
            app.diagnostics_arrived(command, items);
        }
        DaemonMessage::ChangeDetails {
            line,
            before,
            after,
            truncated,
            ..
        } => app.details_arrived(line, before, after, truncated),
        DaemonMessage::SetAutosave { autosave, .. } => app.set_autosave(autosave),
        // Clamped before anything is done with it: `events` arrives from the
        // wire, and a sender that stopped draining must not decide how long
        // this loop runs without publishing.
        DaemonMessage::Input { events, .. } => {
            for event in events.into_iter().take(MAX_INPUT_EVENTS) {
                input::apply(app, event);
            }
        }
        DaemonMessage::SetView { view, .. } => {
            app.set_view(
                view.first_line as usize,
                (view.line_count as usize).min(MAX_VIEW_ROWS),
            );
        }
        DaemonMessage::Save { request_id } => {
            // The answer is the `SaveRequest` the main loop flushes next, and
            // then the daemon's `Saved`/`SaveRefused` for it.
            let _ = request_id;
            app.request_save();
        }
        DaemonMessage::SaveRefused { request_id, reason } => {
            app.save_refused(request_id, &reason);
        }
        DaemonMessage::ApplyPreviewEdit {
            request_id,
            expected_document_version,
            edits,
        } => match app.apply_preview_edit(&edits, expected_document_version) {
            Ok(document_version) => control.send(&EditorMessage::Applied {
                request_id,
                document_version,
            })?,
            Err(reason) => control.send(&EditorMessage::Refused { request_id, reason })?,
        },
        DaemonMessage::Open { request_id, .. } => {
            // A second open would replace the buffer behind the daemon's back.
            control.send(&EditorMessage::Refused {
                request_id,
                reason: "a buffer is already open".to_string(),
            })?;
        }
        // The handshake is over; a second Welcome is a protocol error rather
        // than something to answer.
        DaemonMessage::Welcome { .. } => {
            anyhow::bail!("unexpected Welcome after the control handshake");
        }
        // Forward compatibility: a daemon from the future may send a request
        // this build does not know; ignoring it is the conservative answer.
        _ => {}
    }
    Ok(())
}

/// Send the save the last key press asked for, if it asked for one.
fn flush_save(app: &mut App, control: &mut ControlChannel) -> anyhow::Result<()> {
    let Some((request_id, text, document_version)) = app.take_save_request() else {
        return Ok(());
    };
    control.send(&EditorMessage::SaveRequest {
        request_id,
        text,
        document_version,
    })?;
    Ok(())
}

/// Send the definition lookup and the open request, if the last key made one.
fn flush_lookups(app: &mut App, control: &mut ControlChannel) -> anyhow::Result<()> {
    if let Some((request_id, symbol)) = app.take_definition_request() {
        control.send(&EditorMessage::FindDefinition { request_id, symbol })?;
    }
    if let Some(request_id) = app.take_diagnostics_request() {
        control.send(&EditorMessage::RunDiagnostics { request_id })?;
    }
    if let Some((request_id, line)) = app.take_details_request() {
        control.send(&EditorMessage::ChangeDetails { request_id, line })?;
    }
    if let Some((request_id, path, line)) = app.take_open_request() {
        control.send(&EditorMessage::OpenPath {
            request_id,
            path,
            line,
        })?;
    }
    Ok(())
}

/// Publish the window when it changed.
///
/// The comparison is what keeps a keystroke that moved nothing — a `Ctrl` on
/// its own, an arrow at the end of the buffer — off the socket. Building the
/// frame is per input and not per tick, so the cost is the one the person
/// just asked for.
fn publish_frame(
    app: &mut App,
    control: &mut ControlChannel,
    buffer_id: u64,
    last: &mut Option<editor_control::ViewFrame>,
) -> anyhow::Result<()> {
    // The decoration cache is an input to the window, and the TUI happens to
    // fill it inside `take_frame`. Both sinks need it; only one of them paints.
    app.prepare_view();
    let built = frame::build(app, buffer_id);
    if last.as_ref() == Some(&built) {
        return Ok(());
    }
    control.send(&EditorMessage::ViewFrame {
        frame: built.clone(),
    })?;
    *last = Some(built);
    Ok(())
}

/// Report the buffer state when it changed.
fn publish_state(
    app: &App,
    control: &mut ControlChannel,
    last: &mut Option<EditorStateWire>,
) -> anyhow::Result<()> {
    let state = app.wire_state();
    if last.as_ref() == Some(&state) {
        return Ok(());
    }
    control.send(&EditorMessage::State {
        request_id: None,
        state: state.clone(),
    })?;
    *last = Some(state);
    Ok(())
}

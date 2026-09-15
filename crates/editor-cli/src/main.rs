//! `forge-editor`: the standalone terminal editor.
//!
//! The binary is the adapter: arguments, a bounded read, raw mode, and the
//! event loop. Editing decisions live in `editor-core`, key bindings in `app`,
//! and disk access in `disk` — running this must never require the daemon, the
//! GUI, a network or Node.
//!
//! `--control <socket>` is the one integrated route: the daemon owns the
//! document, hands the buffer over that socket, and the local disk adapter
//! stays off. See `control.rs`.

mod app;
mod cli;
mod control;
mod disk;
mod render;
mod screen;
mod view;

use std::process::ExitCode;
use std::thread;

use crossterm::event::{self, Event, KeyEventKind};
use editor_control::{DaemonMessage, EditorMessage, EditorStateWire};
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
        Ok(cli::Command::Edit(options)) => match run(options) {
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

fn run(options: cli::Options) -> anyhow::Result<()> {
    let (mut app, mut control) = open(options)?;
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
    let mut painted = std::time::Instant::now() - FRAME;
    let result = loop {
        // Paint only when the input has caught up, or the floor has passed.
        // Damage accumulates either way, so a skipped frame costs nothing but
        // the write it did not make.
        if events_rx.is_empty() || painted.elapsed() >= FRAME {
            if let Err(error) = render::draw(&mut app, &mut out.lock()) {
                break Err(error.into());
            }
            painted = std::time::Instant::now();
        }
        let incoming = control.as_ref().map(|channel| channel.incoming().clone());
        let input = match &incoming {
            Some(incoming) => {
                let selector = flume::Selector::new()
                    .recv(&events_rx, |result| Input::Terminal(result.ok()))
                    .recv(incoming, |result| {
                        Input::Control(result.unwrap_or(Incoming::Closed {
                            reason: "disconnected".to_string(),
                        }))
                    });
                // Waiting *on* the deadline rather than polling for it: the
                // loop still blocks until something happens, and the pause is
                // what wakes it. A bare sleep-and-check here would be the
                // defect `docs/performance.md` names.
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
        };
        match input {
            Input::Terminal(Some(Event::Key(key))) if key.kind != KeyEventKind::Release => {
                app.handle_key(key);
            }
            Input::Terminal(Some(Event::Resize(width, height))) => app.resize(width, height),
            Input::Terminal(Some(Event::Paste(text))) => app.paste(&text),
            Input::Terminal(Some(Event::Mouse(mouse))) => app.handle_mouse(mouse),
            Input::Terminal(Some(_)) => {}
            Input::Terminal(None) => break Ok(()),
            Input::Control(Incoming::Message(message)) => {
                if let Some(channel) = control.as_mut() {
                    handle_control(&mut app, channel, message)?;
                }
            }
            Input::Tick => app.autosave_if_due(),
            Input::Control(Incoming::Closed { reason }) => {
                break Err(anyhow::anyhow!("daemon control channel closed: {reason}"));
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
    };
    // Restore the caller's screen before any error is printed on it.
    drop(screen);
    result
}

/// Build the app for the mode the arguments name.
///
/// Standalone reads the file through `disk`; integrated hands the buffer to the
/// daemon and never resolves the path on disk.
fn open(options: cli::Options) -> anyhow::Result<(App, Option<ControlChannel>)> {
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
        app.set_integrated();
        app.set_autosave(opened.autosave);
        let line = opened.line.map(|value| value as usize).or(options.line);
        if let Some(line) = line {
            app.reveal(line, None);
        }
        channel.send(&EditorMessage::Opened {
            request_id: opened.request_id,
            document_version: app.document().version().0,
        })?;
        return Ok((app, Some(channel)));
    }

    let loaded = disk::load(&options.path)
        .map_err(|error| anyhow::anyhow!("could not read {}: {error}", options.path.display()))?;
    let document = Document::from_bytes(&loaded.bytes, options.read_only)
        .map_err(|error| anyhow::anyhow!("{}: {error}", options.path.display()))?;

    let mut app = App::new(document, options.path, Some(loaded.revision));
    if let Some(line) = options.line {
        app.goto_line(line);
    }
    Ok((app, None))
}

/// Answer the daemon's request on the serialized event thread.
fn handle_control(
    app: &mut App,
    control: &mut ControlChannel,
    message: DaemonMessage,
) -> anyhow::Result<()> {
    match message {
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
        DaemonMessage::ChangeDetails {
            line,
            before,
            after,
            truncated,
            ..
        } => app.details_arrived(line, before, after, truncated),
        DaemonMessage::SetAutosave { autosave, .. } => app.set_autosave(autosave),
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

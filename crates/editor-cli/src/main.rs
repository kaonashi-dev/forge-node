//! `forge-editor`: the standalone terminal editor.
//!
//! The binary is the adapter: arguments, a bounded read, raw mode, and the
//! event loop. Editing decisions live in `editor-core`, key bindings in `app`,
//! and disk access in `disk` — running this must never require the daemon, the
//! GUI, a network or Node.

mod app;
mod cli;
mod disk;
mod render;
mod screen;
mod view;

use std::process::ExitCode;

use crossterm::event::{self, Event, KeyEventKind};
use editor_core::Document;

use crate::app::App;

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

fn run(options: cli::Options) -> anyhow::Result<()> {
    let loaded = disk::load(&options.path)
        .map_err(|error| anyhow::anyhow!("could not read {}: {error}", options.path.display()))?;
    let document = Document::from_bytes(&loaded.bytes, options.read_only)
        .map_err(|error| anyhow::anyhow!("{}: {error}", options.path.display()))?;

    let mut app = App::new(document, options.path, Some(loaded.revision));
    if let Some(line) = options.line {
        app.goto_line(line);
    }
    let screen = screen::Screen::enter()?;
    let (width, height) = screen.size()?;
    app.resize(width, height);

    let out = std::io::stdout();
    let result = loop {
        if let Err(error) = render::draw(&mut app, &mut out.lock()) {
            break Err(error.into());
        }
        match event::read() {
            Ok(Event::Key(key)) if key.kind != KeyEventKind::Release => app.handle_key(key),
            Ok(Event::Resize(width, height)) => app.resize(width, height),
            Ok(Event::Paste(text)) => app.paste(&text),
            Ok(_) => {}
            Err(error) => break Err(error.into()),
        }
        if app.should_quit() {
            break Ok(());
        }
    };
    // Restore the caller's screen before any error is printed on it.
    drop(screen);
    result
}

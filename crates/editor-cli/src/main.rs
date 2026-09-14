//! `forge-editor`: the standalone terminal editor spike (F1).
//!
//! Not the product: no syntax highlighting, no undo, no daemon channel. What it
//! proves is the route the plan needs before F2 builds an engine — a real PTY,
//! raw input (bracketed paste included), a rendered viewport, and an atomic
//! save — so F1 can measure it and decide go/no-go.
//!
//! The crate depends on no Forge crate on purpose: running it must never
//! require the daemon, the GUI, a network or Node.

mod app;
mod cli;
mod document;
mod render;
mod screen;
mod view;

use std::process::ExitCode;

use crossterm::event::{self, Event, KeyEventKind};

use crate::app::App;
use crate::document::Document;

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
    let bytes = std::fs::read(&options.path)
        .map_err(|error| anyhow::anyhow!("could not read {}: {error}", options.path.display()))?;
    let document = Document::from_bytes(&bytes, options.read_only)
        .map_err(|error| anyhow::anyhow!("{}: {error}", options.path.display()))?;

    let mut app = App::new(document, options.path);
    let screen = screen::Screen::enter()?;
    let (width, height) = screen.size()?;
    app.resize(width, height);

    let out = std::io::stdout();
    loop {
        render::draw(&app, &mut out.lock())?;
        match event::read()? {
            Event::Key(key) if key.kind != KeyEventKind::Release => {
                app.handle_key(key);
            }
            Event::Resize(width, height) => app.resize(width, height),
            Event::Paste(text) => app.paste(&text),
            _ => {}
        }
        if app.should_quit() {
            break;
        }
    }
    // Restore the caller's screen before any error is printed on it.
    drop(screen);
    Ok(())
}

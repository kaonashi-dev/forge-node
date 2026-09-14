//! Raw mode and the alternate screen, restored by a guard.
//!
//! `Drop` and not just the happy path: an error or a panic mid-loop must leave
//! the caller's shell usable, so the tests in this crate can drive the binary
//! and the person running it can still type afterwards.

use std::io;

use crossterm::event::{DisableBracketedPaste, EnableBracketedPaste};
use crossterm::terminal::{
    disable_raw_mode, enable_raw_mode, size, DisableLineWrap, EnableLineWrap, EnterAlternateScreen,
    LeaveAlternateScreen,
};
use crossterm::{cursor, execute};

pub struct Screen;

impl Screen {
    pub fn enter() -> io::Result<Self> {
        enable_raw_mode()?;
        let mut out = io::stdout();
        if let Err(error) = execute!(
            out,
            EnterAlternateScreen,
            DisableLineWrap,
            EnableBracketedPaste
        ) {
            let _ = disable_raw_mode();
            return Err(error);
        }
        Ok(Self)
    }

    /// Current terminal size, after any resize the terminal has reported.
    pub fn size(&self) -> io::Result<(u16, u16)> {
        size()
    }
}

impl Drop for Screen {
    fn drop(&mut self) {
        let mut out = io::stdout();
        let _ = execute!(
            out,
            cursor::Show,
            LeaveAlternateScreen,
            DisableBracketedPaste,
            EnableLineWrap
        );
        let _ = disable_raw_mode();
    }
}

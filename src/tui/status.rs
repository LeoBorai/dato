//! Pins the status bar to the last terminal row.
//!
//! The shell scrolls inside a region that leaves out the last row, so the bar
//! stays put while output grows above it.

use std::io::{self, IsTerminal, Write};

use crossterm::{cursor, queue, terminal};

use crate::ui::StatusInfo;

/// Keeps the scroll region set while alive and restores the terminal on drop.
pub struct StatusBar {
    rows: u16,
}

/// Rows below which there is no room for a bar next to the shell.
const MIN_ROWS: u16 = 4;

impl StatusBar {
    /// `None` when stdout is not a terminal or the terminal is too small.
    pub fn enable() -> Option<Self> {
        let mut out = io::stdout();
        if !out.is_terminal() {
            return None;
        }
        let (_, rows) = terminal::size().ok()?;
        if rows < MIN_ROWS {
            return None;
        }
        // Make sure the cursor is above the bar row before the region is set.
        out.write_all(b"\n").ok()?;
        queue!(out, cursor::MoveUp(1)).ok()?;
        let bar = Self { rows };
        bar.set_region(&mut out).ok()?;
        out.flush().ok()?;
        Some(bar)
    }

    /// Rows left for the shell.
    pub fn shell_rows(&self) -> u16 {
        self.rows - 1
    }

    fn set_region(&self, out: &mut impl Write) -> io::Result<()> {
        // Setting the region homes the cursor; put it back.
        queue!(out, cursor::SavePosition)?;
        write!(out, "\x1b[1;{}r", self.rows - 1)?;
        queue!(out, cursor::RestorePosition)
    }

    /// Draws the bar, following a changed terminal size.
    pub fn draw(&mut self, info: &StatusInfo) {
        let mut out = io::stdout();
        let Ok((columns, rows)) = terminal::size() else {
            return;
        };
        if rows < MIN_ROWS {
            return;
        }
        if rows != self.rows {
            self.rows = rows;
            let _ = self.set_region(&mut out);
        }
        let _ = queue!(out, cursor::SavePosition, cursor::MoveTo(0, rows - 1));
        let _ = write!(out, "{}", info.render(columns as usize).ansi());
        let _ = queue!(out, cursor::RestorePosition);
        let _ = out.flush();
    }
}

impl Drop for StatusBar {
    fn drop(&mut self) {
        let mut out = io::stdout();
        // Free the whole screen and leave the cursor on the bar's row.
        let _ = write!(out, "\x1b[r");
        let _ = queue!(
            out,
            cursor::MoveTo(0, self.rows - 1),
            terminal::Clear(terminal::ClearType::CurrentLine)
        );
        let _ = out.flush();
    }
}

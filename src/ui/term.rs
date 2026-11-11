//! Drives a [`Widget`] on a terminal: raw mode, key events and inline redraws.

use std::collections::VecDeque;
use std::io::{self, Write};

use crossterm::event::{self, Event, KeyEvent};
use crossterm::{cursor, queue, terminal};

use super::{Step, Widget, is_press};

/// Switches the terminal's raw mode on and off.
pub trait RawMode {
    fn enable(&mut self) -> io::Result<()>;
    fn disable(&mut self) -> io::Result<()>;
}

/// The process terminal.
pub struct Terminal;

impl RawMode for Terminal {
    fn enable(&mut self) -> io::Result<()> {
        terminal::enable_raw_mode()
    }

    fn disable(&mut self) -> io::Result<()> {
        terminal::disable_raw_mode()
    }
}

/// Keeps raw mode on while alive and restores cooked mode on drop, including
/// on early return or panic.
pub struct RawModeGuard<'a, R: RawMode> {
    mode: &'a mut R,
}

impl<'a, R: RawMode> RawModeGuard<'a, R> {
    pub fn new(mode: &'a mut R) -> io::Result<Self> {
        mode.enable()?;
        Ok(Self { mode })
    }
}

impl<R: RawMode> Drop for RawModeGuard<'_, R> {
    fn drop(&mut self) {
        let _ = self.mode.disable();
    }
}

/// Where key events come from.
pub trait KeySource {
    /// The next key press; `None` when the source is exhausted.
    fn next_key(&mut self) -> io::Result<Option<KeyEvent>>;
}

/// Key presses from the terminal (requires raw mode).
pub struct TerminalKeys;

impl KeySource for TerminalKeys {
    fn next_key(&mut self) -> io::Result<Option<KeyEvent>> {
        loop {
            if let Event::Key(k) = event::read()?
                && is_press(&k)
            {
                return Ok(Some(k));
            }
        }
    }
}

/// A fixed list of keys, for tests.
pub struct VecKeys(pub VecDeque<KeyEvent>);

impl VecKeys {
    pub fn new(keys: impl IntoIterator<Item = KeyEvent>) -> Self {
        Self(keys.into_iter().collect())
    }
}

impl KeySource for VecKeys {
    fn next_key(&mut self) -> io::Result<Option<KeyEvent>> {
        Ok(self.0.pop_front())
    }
}

/// Redraws a block of lines in place below the cursor.
struct Frame {
    drawn: u16,
}

impl Frame {
    fn draw<O: Write>(&mut self, out: &mut O, lines: &[super::Line]) -> io::Result<()> {
        if self.drawn > 0 {
            queue!(out, cursor::MoveUp(self.drawn))?;
        }
        queue!(
            out,
            cursor::MoveToColumn(0),
            terminal::Clear(terminal::ClearType::FromCursorDown)
        )?;
        for line in lines {
            write!(out, "{}\r\n", line.ansi())?;
        }
        self.drawn = lines.len() as u16;
        out.flush()
    }
}

/// Runs `widget` with keys from `keys`, drawing to `out`, until it finishes or
/// is cancelled. Returns `None` on cancel or when keys run out. The final frame
/// stays on screen.
pub fn drive<W: Widget, O: Write>(
    widget: &mut W,
    keys: &mut dyn KeySource,
    out: &mut O,
) -> io::Result<Option<W::Output>> {
    let mut frame = Frame { drawn: 0 };
    queue!(out, cursor::Hide)?;
    frame.draw(out, &widget.render())?;
    let result = loop {
        let Some(key) = keys.next_key()? else {
            break None;
        };
        let step = widget.on_key(key);
        frame.draw(out, &widget.render())?;
        match step {
            Step::Continue => {}
            Step::Done(v) => break Some(v),
            Step::Cancel => break None,
        }
    };
    queue!(out, cursor::Show)?;
    out.flush()?;
    Ok(result)
}

/// Runs `widget` on the real terminal in raw mode, drawing to stdout.
pub fn run<W: Widget>(widget: &mut W) -> io::Result<Option<W::Output>> {
    let mut mode = Terminal;
    let _raw = RawModeGuard::new(&mut mode)?;
    drive(widget, &mut TerminalKeys, &mut io::stdout())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ui::Line;
    use crate::ui::test_keys::{enter, key, typed};
    use crossterm::event::KeyCode;

    #[derive(Default)]
    struct FakeMode {
        raw: bool,
        calls: Vec<&'static str>,
    }

    impl RawMode for FakeMode {
        fn enable(&mut self) -> io::Result<()> {
            self.raw = true;
            self.calls.push("enable");
            Ok(())
        }
        fn disable(&mut self) -> io::Result<()> {
            self.raw = false;
            self.calls.push("disable");
            Ok(())
        }
    }

    #[test]
    fn guard_restores_cooked_mode_on_drop() {
        let mut mode = FakeMode::default();
        {
            let g = RawModeGuard::new(&mut mode).unwrap();
            assert!(g.mode.raw);
        }
        assert!(!mode.raw);
        assert_eq!(mode.calls, ["enable", "disable"]);
    }

    #[test]
    fn guard_restores_on_panic() {
        let mut mode = FakeMode::default();
        let r = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            let _g = RawModeGuard::new(&mut mode).unwrap();
            panic!("boom");
        }));
        assert!(r.is_err());
        assert!(!mode.raw);
    }

    /// Collects typed characters until Enter.
    struct Echo(String);

    impl Widget for Echo {
        type Output = String;
        fn on_key(&mut self, k: KeyEvent) -> Step<String> {
            match k.code {
                KeyCode::Enter => Step::Done(self.0.clone()),
                KeyCode::Esc => Step::Cancel,
                KeyCode::Char(c) => {
                    self.0.push(c);
                    Step::Continue
                }
                _ => Step::Continue,
            }
        }
        fn render(&self) -> Vec<Line> {
            vec![Line::plain(format!("> {}", self.0))]
        }
    }

    #[test]
    fn drive_feeds_keys_and_redraws() {
        let mut out = Vec::new();
        let mut keys = VecKeys::new(typed("ab").into_iter().chain([enter()]));
        let got = drive(&mut Echo(String::new()), &mut keys, &mut out).unwrap();
        assert_eq!(got.as_deref(), Some("ab"));
        let screen = String::from_utf8_lossy(&out);
        assert!(screen.contains("> ab\r\n"), "{screen:?}");

        let mut keys = VecKeys::new([key(KeyCode::Esc)]);
        assert_eq!(
            drive(&mut Echo(String::new()), &mut keys, &mut Vec::<u8>::new()).unwrap(),
            None
        );
    }
}

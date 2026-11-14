//! Terminal UI components in atomic design, built on `crossterm`.
//!
//! - `atoms`: smallest interactive widgets (select list, text and password input).
//! - `molecules`: atoms combined with a label and an error line.
//! - `organisms`: complete flows built from molecules (the connect wizard).
//!
//! Every component is a [`Widget`]: a state machine that takes key events and
//! renders lines. Components never touch the terminal themselves; [`term::run`]
//! drives a widget against a real terminal, and tests feed key events directly.

pub mod atoms;
pub mod form;
pub mod molecules;
pub mod organisms;
pub mod status_bar;
pub mod term;

use crossterm::event::{KeyCode, KeyEvent, KeyEventKind, KeyModifiers};
use nu_ansi_term::{Color, Style};

use crate::server::Server;

pub use form::{ConnectForm, Field, FieldValues};
pub use status_bar::StatusInfo;

/// The color that marks a server: the dot of its saved connections.
pub fn server_color(server: Server) -> Color {
    match server {
        Server::PostgreSql => Color::Cyan,
        Server::MySql => Color::Yellow,
        Server::Sqlite => Color::Purple,
    }
}

/// Text color that reads well on a background of `bg`.
pub fn contrast_color(bg: Color) -> Color {
    match bg {
        Color::Cyan | Color::Yellow | Color::White | Color::LightGray => Color::Black,
        _ => Color::White,
    }
}

/// How a server is named to the user.
pub fn server_title(server: Server) -> &'static str {
    match server {
        Server::PostgreSql => "PostgreSQL",
        Server::MySql => "MySQL",
        Server::Sqlite => "SQLite",
    }
}

/// What a widget wants after handling a key.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Step<T> {
    /// Keep reading keys.
    Continue,
    /// Finished with a value.
    Done(T),
    /// The user gave up (Esc or Ctrl-C).
    Cancel,
}

/// One rendered line: styled spans, in order.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Line(pub Vec<(Style, String)>);

impl Line {
    pub fn plain(text: impl Into<String>) -> Self {
        Line(vec![(Style::new(), text.into())])
    }

    pub fn styled(style: Style, text: impl Into<String>) -> Self {
        Line(vec![(style, text.into())])
    }

    pub fn push(&mut self, style: Style, text: impl Into<String>) {
        self.0.push((style, text.into()));
    }

    pub fn extend(&mut self, other: Line) {
        self.0.extend(other.0);
    }

    /// The text without styling.
    pub fn text(&self) -> String {
        self.0.iter().map(|(_, t)| t.as_str()).collect()
    }

    /// The text with ANSI styling.
    pub fn ansi(&self) -> String {
        self.0
            .iter()
            .map(|(s, t)| s.paint(t.as_str()).to_string())
            .collect()
    }
}

/// An interactive component.
pub trait Widget {
    type Output;

    fn on_key(&mut self, key: KeyEvent) -> Step<Self::Output>;

    fn render(&self) -> Vec<Line>;
}

/// Esc, or Ctrl-C (which arrives as a key while the terminal is in raw mode).
pub fn is_cancel(key: &KeyEvent) -> bool {
    key.code == KeyCode::Esc
        || (key.code == KeyCode::Char('c') && key.modifiers.contains(KeyModifiers::CONTROL))
}

/// Whether the event is a press (or repeat), as opposed to a release.
pub fn is_press(key: &KeyEvent) -> bool {
    key.kind != KeyEventKind::Release
}

#[cfg(test)]
pub(crate) mod test_keys {
    //! Key-event helpers for component tests.

    use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

    pub fn key(code: KeyCode) -> KeyEvent {
        KeyEvent::new(code, KeyModifiers::NONE)
    }

    pub fn ctrl(c: char) -> KeyEvent {
        KeyEvent::new(KeyCode::Char(c), KeyModifiers::CONTROL)
    }

    /// One `Char` event per character of `s`.
    pub fn typed(s: &str) -> Vec<KeyEvent> {
        s.chars().map(|c| key(KeyCode::Char(c))).collect()
    }

    pub fn enter() -> KeyEvent {
        key(KeyCode::Enter)
    }

    /// Feeds `keys` to `w`, returning the first non-`Continue` step.
    pub fn feed<W: super::Widget>(
        w: &mut W,
        keys: impl IntoIterator<Item = KeyEvent>,
    ) -> super::Step<W::Output> {
        for k in keys {
            match w.on_key(k) {
                super::Step::Continue => {}
                other => return other,
            }
        }
        super::Step::Continue
    }
}

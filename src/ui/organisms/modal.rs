//! Boxed dialogs: confirm, password and notice.

use crossterm::event::{KeyCode, KeyEvent};
use nu_ansi_term::{Color, Style};

use crate::ui::atoms::PasswordInput;
use crate::ui::{Line, Step, Widget, is_cancel};

/// Columns of text inside the box.
const WIDTH: usize = 56;

/// Splits `text` into lines of at most [`WIDTH`] columns, at spaces.
fn wrap(text: &str) -> Vec<String> {
    let mut lines: Vec<String> = Vec::new();
    for word in text.split_whitespace() {
        match lines.last_mut() {
            Some(l) if l.len() + 1 + word.len() <= WIDTH => {
                l.push(' ');
                l.push_str(word);
            }
            _ => lines.push(word.to_string()),
        }
    }
    lines
}

fn boxed(title: &str, message: &str, extra: Option<Line>, hint: &str) -> Vec<Line> {
    let border = Style::new().fg(Color::Yellow);
    let row = |content: Line| {
        let pad = WIDTH.saturating_sub(content.text().chars().count());
        let mut line = Line::styled(border, "│ ");
        line.extend(content);
        line.push(border, format!("{} │", " ".repeat(pad)));
        line
    };
    let rule = |left: &str, right: &str| {
        Line::styled(border, format!("{left}{}{right}", "─".repeat(WIDTH + 2)))
    };
    let mut out = vec![
        rule("┌", "┐"),
        row(Line::styled(border.bold(), title)),
        row(Line::default()),
    ];
    out.extend(wrap(message).into_iter().map(|l| row(Line::plain(l))));
    if let Some(extra) = extra {
        out.push(row(extra));
    }
    out.push(row(Line::default()));
    out.push(row(Line::styled(Style::new().fg(Color::DarkGray), hint)));
    out.push(rule("└", "┘"));
    out
}

/// `y` accepts; `n`, Enter and Esc decline.
pub struct ConfirmModal {
    message: &'static str,
}

impl ConfirmModal {
    pub fn new(message: &'static str) -> Self {
        Self { message }
    }
}

impl Widget for ConfirmModal {
    type Output = ();

    fn on_key(&mut self, key: KeyEvent) -> Step<()> {
        match key.code {
            _ if is_cancel(&key) => Step::Cancel,
            KeyCode::Char('y' | 'Y') => Step::Done(()),
            KeyCode::Char('n' | 'N') | KeyCode::Enter => Step::Cancel,
            _ => Step::Continue,
        }
    }

    fn render(&self) -> Vec<Line> {
        boxed(
            "Safeguard",
            self.message,
            None,
            "y to send, n or Esc to abort",
        )
    }
}

/// Asks for a password without showing it.
#[derive(Default)]
pub struct PasswordModal {
    input: PasswordInput,
}

impl Widget for PasswordModal {
    type Output = String;

    fn on_key(&mut self, key: KeyEvent) -> Step<String> {
        self.input.on_key(key)
    }

    fn render(&self) -> Vec<Line> {
        // A long password would break out of the box: show at most its width.
        let stars = self.input.value().chars().count().min(WIDTH - 2);
        boxed(
            "Safeguard",
            "Enter the connection password to send this query.",
            Some(Line::plain(format!("> {}", "*".repeat(stars)))),
            "Enter to send, Esc to abort",
        )
    }
}

/// A message dismissed with Enter or Esc.
pub struct NoticeModal {
    message: String,
}

impl NoticeModal {
    pub fn new(message: &str) -> Self {
        Self {
            message: message.into(),
        }
    }
}

impl Widget for NoticeModal {
    type Output = ();

    fn on_key(&mut self, key: KeyEvent) -> Step<()> {
        if is_cancel(&key) || key.code == KeyCode::Enter {
            Step::Done(())
        } else {
            Step::Continue
        }
    }

    fn render(&self) -> Vec<Line> {
        boxed("Query aborted", &self.message, None, "Enter to close")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ui::test_keys::{enter, feed, key, typed};

    #[test]
    fn confirm_only_accepts_y() {
        let step = |k| feed(&mut ConfirmModal::new("m"), [k]);
        assert_eq!(step(key(KeyCode::Char('y'))), Step::Done(()));
        assert_eq!(step(enter()), Step::Cancel);
        assert_eq!(step(key(KeyCode::Esc)), Step::Cancel);
    }

    #[test]
    fn password_is_masked_and_returned() {
        let mut m = PasswordModal::default();
        feed(&mut m, typed("secret"));
        assert!(!m.render().iter().any(|l| l.text().contains("secret")));
        assert_eq!(feed(&mut m, [enter()]), Step::Done("secret".into()));
    }

    #[test]
    fn box_lines_have_one_width() {
        let lines = NoticeModal::new(crate::safeguard::BLOCKED_MESSAGE).render();
        assert!(lines.iter().all(|l| l.text().chars().count() == WIDTH + 4));
    }
}

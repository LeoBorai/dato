//! A single-line text field with a cursor.

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use nu_ansi_term::{Color, Style};

use crate::ui::{Line, Step, Widget, is_cancel};

#[derive(Debug, Clone)]
pub struct TextInput {
    chars: Vec<char>,
    /// Cursor position, in characters.
    cursor: usize,
    placeholder: String,
    /// Character shown instead of each typed character (password fields).
    mask: Option<char>,
}

impl TextInput {
    /// A field pre-filled with `default`, cursor at the end.
    pub fn new(default: &str) -> Self {
        let chars: Vec<char> = default.chars().collect();
        Self {
            cursor: chars.len(),
            chars,
            placeholder: String::new(),
            mask: None,
        }
    }

    /// Text shown dimmed while the field is empty.
    pub fn with_placeholder(mut self, placeholder: &str) -> Self {
        self.placeholder = placeholder.to_string();
        self
    }

    /// Shows `mask` once per character instead of the text.
    pub fn with_mask(mut self, mask: char) -> Self {
        self.mask = Some(mask);
        self
    }

    pub fn value(&self) -> String {
        self.chars.iter().collect()
    }

    /// The characters before the cursor.
    pub fn before_cursor(&self) -> &[char] {
        &self.chars[..self.cursor]
    }

    /// Replaces the characters in `range` with `with`, leaving the cursor
    /// after the new text.
    pub fn replace(&mut self, range: std::ops::Range<usize>, with: &str) {
        let new: Vec<char> = with.chars().collect();
        self.cursor = range.start + new.len();
        self.chars.splice(range, new);
    }

    fn shown(&self) -> Vec<char> {
        match self.mask {
            Some(m) => vec![m; self.chars.len()],
            None => self.chars.clone(),
        }
    }

    /// The field without the cursor highlight (for completed fields).
    pub fn render_value(&self) -> Line {
        if self.chars.is_empty() {
            return Line::styled(Style::new().fg(Color::DarkGray), self.placeholder.clone());
        }
        Line::plain(self.shown().into_iter().collect::<String>())
    }
}

impl Widget for TextInput {
    type Output = String;

    fn on_key(&mut self, key: KeyEvent) -> Step<String> {
        if is_cancel(&key) {
            return Step::Cancel;
        }
        let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
        match key.code {
            KeyCode::Enter => return Step::Done(self.value()),
            KeyCode::Char('u') if ctrl => {
                self.chars.drain(..self.cursor);
                self.cursor = 0;
            }
            KeyCode::Char('a') if ctrl => self.cursor = 0,
            KeyCode::Char('e') if ctrl => self.cursor = self.chars.len(),
            KeyCode::Char(_) if ctrl => {}
            KeyCode::Char(c) => {
                self.chars.insert(self.cursor, c);
                self.cursor += 1;
            }
            KeyCode::Backspace if self.cursor > 0 => {
                self.cursor -= 1;
                self.chars.remove(self.cursor);
            }
            KeyCode::Delete if self.cursor < self.chars.len() => {
                self.chars.remove(self.cursor);
            }
            KeyCode::Left => self.cursor = self.cursor.saturating_sub(1),
            KeyCode::Right => self.cursor = (self.cursor + 1).min(self.chars.len()),
            KeyCode::Home => self.cursor = 0,
            KeyCode::End => self.cursor = self.chars.len(),
            _ => {}
        }
        Step::Continue
    }

    /// The text with the character under the cursor shown in reverse video.
    fn render(&self) -> Vec<Line> {
        let cursor = Style::new().reverse();
        let mut line = Line::default();
        if self.chars.is_empty() {
            line.push(cursor, " ");
            if !self.placeholder.is_empty() {
                line.push(Style::new().fg(Color::DarkGray), self.placeholder.clone());
            }
            return vec![line];
        }
        let shown = self.shown();
        let before: String = shown[..self.cursor].iter().collect();
        line.push(Style::new(), before);
        match shown.get(self.cursor) {
            Some(c) => {
                line.push(cursor, c.to_string());
                line.push(
                    Style::new(),
                    shown[self.cursor + 1..].iter().collect::<String>(),
                );
            }
            None => line.push(cursor, " "),
        }
        vec![line]
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ui::test_keys::{ctrl, enter, feed, key, typed};

    fn text(w: &TextInput) -> String {
        w.render()[0].text()
    }

    #[test]
    fn typing_and_enter() {
        let mut w = TextInput::new("");
        assert_eq!(
            feed(&mut w, typed("app.db").into_iter().chain([enter()])),
            Step::Done("app.db".into())
        );
    }

    #[test]
    fn default_value_and_editing_keys() {
        let mut w = TextInput::new("localhost");
        assert_eq!(text(&w), "localhost ");
        feed(&mut w, [key(KeyCode::Backspace); 4]);
        assert_eq!(w.value(), "local");
        feed(&mut w, [key(KeyCode::Home), key(KeyCode::Delete)]);
        assert_eq!(w.value(), "ocal");
        feed(&mut w, [key(KeyCode::Right), key(KeyCode::Char('X'))]);
        assert_eq!(w.value(), "oXcal");
        feed(
            &mut w,
            [key(KeyCode::Left), key(KeyCode::Left), key(KeyCode::End)],
        );
        feed(&mut w, typed("!"));
        assert_eq!(w.value(), "oXcal!");
        feed(&mut w, [ctrl('u')]);
        assert_eq!(w.value(), "");
    }

    #[test]
    fn placeholder_while_empty() {
        let w = TextInput::new("").with_placeholder(":memory:");
        assert_eq!(text(&w), " :memory:");
        assert_eq!(w.render_value().text(), ":memory:");
    }

    #[test]
    fn esc_cancels() {
        assert_eq!(
            feed(&mut TextInput::new("x"), [key(KeyCode::Esc)]),
            Step::Cancel
        );
    }
}

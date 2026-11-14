//! A label, a text or password input, and an optional error line.

use crossterm::event::{KeyCode, KeyEvent};
use nu_ansi_term::{Color, Style};

use crate::ui::atoms::{PasswordInput, PathInput, TextInput};
use crate::ui::{Line, Step, Widget, is_cancel};

/// One of a few fixed values, picked with the arrow keys.
#[derive(Debug, Clone)]
pub struct Choice {
    options: Vec<String>,
    selected: usize,
}

impl Choice {
    /// `selected` falls back to the first option when it is not one of them.
    /// `options` must not be empty.
    pub fn new(options: &[&str], selected: &str) -> Self {
        assert!(!options.is_empty(), "Choice needs at least one option");
        Self {
            options: options.iter().map(|o| o.to_string()).collect(),
            selected: options.iter().position(|o| *o == selected).unwrap_or(0),
        }
    }

    pub fn value(&self) -> String {
        self.options[self.selected].clone()
    }

    fn on_key(&mut self, key: KeyEvent) -> Step<String> {
        if is_cancel(&key) {
            return Step::Cancel;
        }
        let n = self.options.len();
        match key.code {
            KeyCode::Left | KeyCode::Up | KeyCode::BackTab => {
                self.selected = (self.selected + n - 1) % n
            }
            KeyCode::Right | KeyCode::Down | KeyCode::Tab => {
                self.selected = (self.selected + 1) % n
            }
            KeyCode::Enter => return Step::Done(self.value()),
            _ => {}
        }
        Step::Continue
    }

    /// `(*) a  ( ) b  ( ) c`, or just the picked value when `done`.
    fn render_value(&self, done: bool) -> Line {
        if done {
            return Line::plain(self.value());
        }
        let text = self
            .options
            .iter()
            .enumerate()
            .map(|(i, o)| format!("({}) {o}", if i == self.selected { '*' } else { ' ' }))
            .collect::<Vec<_>>()
            .join("  ");
        Line::styled(Style::new().fg(Color::Cyan), text)
    }
}

#[derive(Debug, Clone)]
pub enum FieldInput {
    Text(TextInput),
    Password(PasswordInput),
    Path(PathInput),
    Choice(Choice),
}

#[derive(Debug, Clone)]
pub struct LabeledField {
    pub label: String,
    input: FieldInput,
    error: Option<String>,
}

impl LabeledField {
    pub fn new(label: &str, input: FieldInput) -> Self {
        Self {
            label: label.to_string(),
            input,
            error: None,
        }
    }

    pub fn value(&self) -> String {
        match &self.input {
            FieldInput::Text(t) => t.value(),
            FieldInput::Password(p) => p.value(),
            FieldInput::Path(p) => p.value(),
            FieldInput::Choice(ch) => ch.value(),
        }
    }

    pub fn set_error(&mut self, error: Option<String>) {
        self.error = error;
    }

    fn label_span(&self) -> Line {
        Line::styled(Style::new().bold(), format!("{}: ", self.label))
    }

    /// The finished field: label and value, without the cursor.
    pub fn render_done(&self) -> Line {
        let mut line = self.label_span();
        line.extend(match &self.input {
            FieldInput::Text(t) => t.render_value(),
            FieldInput::Password(p) => p.render_value(),
            FieldInput::Path(p) => p.render_value(),
            FieldInput::Choice(ch) => ch.render_value(true),
        });
        line
    }
}

impl Widget for LabeledField {
    type Output = String;

    fn on_key(&mut self, key: KeyEvent) -> Step<String> {
        self.error = None;
        match &mut self.input {
            FieldInput::Text(t) => t.on_key(key),
            FieldInput::Password(p) => p.on_key(key),
            FieldInput::Path(p) => p.on_key(key),
            FieldInput::Choice(ch) => ch.on_key(key),
        }
    }

    fn render(&self) -> Vec<Line> {
        let mut line = self.label_span();
        let input = match &self.input {
            FieldInput::Text(t) => t.render(),
            FieldInput::Password(p) => p.render(),
            FieldInput::Path(p) => p.render(),
            FieldInput::Choice(ch) => vec![ch.render_value(false)],
        };
        let mut input = input.into_iter();
        if let Some(first) = input.next() {
            line.extend(first);
        }
        let mut out = vec![line];
        // Menu lines of a path field.
        out.extend(input);
        if let Some(e) = &self.error {
            out.push(Line::styled(Style::new().fg(Color::Red), format!("  {e}")));
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ui::test_keys::{enter, feed, typed};

    #[test]
    fn label_value_and_error() {
        let mut f = LabeledField::new("Port", FieldInput::Text(TextInput::new("3306")));
        assert_eq!(f.render()[0].text(), "Port: 3306 ");
        f.set_error(Some("not a port number".into()));
        assert_eq!(f.render()[1].text(), "  not a port number");
        // Typing clears the error.
        feed(&mut f, typed("7"));
        assert_eq!(f.render().len(), 1);
        assert_eq!(feed(&mut f, [enter()]), Step::Done("33067".into()));
        assert_eq!(f.render_done().text(), "Port: 33067");
    }

    #[test]
    fn password_field_is_masked() {
        let mut f = LabeledField::new("Password", FieldInput::Password(PasswordInput::new()));
        feed(&mut f, typed("secret"));
        assert_eq!(f.render_done().text(), "Password: ******");
        assert_eq!(f.value(), "secret");
    }

    #[test]
    fn choice_field_cycles_and_picks() {
        use crossterm::event::KeyCode;
        let mut f = LabeledField::new(
            "TLS mode",
            FieldInput::Choice(Choice::new(&["disable", "prefer", "require"], "disable")),
        );
        assert_eq!(
            f.render()[0].text(),
            "TLS mode: (*) disable  ( ) prefer  ( ) require"
        );
        let key = |code| crate::ui::test_keys::key(code);
        feed(&mut f, [key(KeyCode::Right), key(KeyCode::Right)]);
        assert_eq!(f.value(), "require");
        feed(&mut f, [key(KeyCode::Right)]);
        assert_eq!(f.value(), "disable");
        feed(&mut f, [key(KeyCode::Left)]);
        assert_eq!(f.value(), "require");
        assert_eq!(feed(&mut f, [enter()]), Step::Done("require".into()));
        assert_eq!(f.render_done().text(), "TLS mode: require");
        assert_eq!(feed(&mut f, [key(KeyCode::Esc)]), Step::Cancel);
    }
}

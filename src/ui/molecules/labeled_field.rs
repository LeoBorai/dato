//! A label, a text or password input, and an optional error line.

use crossterm::event::KeyEvent;
use nu_ansi_term::{Color, Style};

use crate::ui::atoms::{PasswordInput, TextInput};
use crate::ui::{Line, Step, Widget};

#[derive(Debug, Clone)]
pub enum FieldInput {
    Text(TextInput),
    Password(PasswordInput),
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
        }
    }

    fn render(&self) -> Vec<Line> {
        let mut line = self.label_span();
        let input = match &self.input {
            FieldInput::Text(t) => t.render(),
            FieldInput::Password(p) => p.render(),
        };
        if let Some(first) = input.into_iter().next() {
            line.extend(first);
        }
        let mut out = vec![line];
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
}

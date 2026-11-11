//! A text field that never shows what is typed: one `*` per character.

use crossterm::event::KeyEvent;

use super::TextInput;
use crate::ui::{Line, Step, Widget};

pub const MASK: char = '*';

#[derive(Debug, Clone)]
pub struct PasswordInput(TextInput);

impl PasswordInput {
    pub fn new() -> Self {
        Self(TextInput::new("").with_mask(MASK))
    }

    pub fn value(&self) -> String {
        self.0.value()
    }

    pub fn render_value(&self) -> Line {
        self.0.render_value()
    }
}

impl Default for PasswordInput {
    fn default() -> Self {
        Self::new()
    }
}

impl Widget for PasswordInput {
    type Output = String;

    fn on_key(&mut self, key: KeyEvent) -> Step<String> {
        self.0.on_key(key)
    }

    fn render(&self) -> Vec<Line> {
        self.0.render()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ui::test_keys::{enter, feed, typed};

    #[test]
    fn typed_text_is_masked() {
        let mut w = PasswordInput::new();
        feed(&mut w, typed("secret"));
        let shown = w.render()[0].text();
        assert_eq!(shown, "****** ");
        assert!(!shown.contains("secret"));
        assert!(!w.render()[0].ansi().contains("secret"));
        assert_eq!(w.render_value().text(), "******");
        assert_eq!(feed(&mut w, [enter()]), Step::Done("secret".into()));
    }
}

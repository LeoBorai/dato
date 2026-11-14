//! Pick one item with the arrow keys and Enter.

use std::fmt::Display;

use crossterm::event::{KeyCode, KeyEvent};
use nu_ansi_term::{Color, Style};

use crate::ui::{Line, Step, Widget, is_cancel};

pub struct SelectList<T> {
    items: Vec<T>,
    selected: usize,
}

impl<T: Clone + Display> SelectList<T> {
    /// `items` must not be empty.
    pub fn new(items: impl IntoIterator<Item = T>) -> Self {
        let items: Vec<T> = items.into_iter().collect();
        assert!(!items.is_empty(), "SelectList needs at least one item");
        Self { items, selected: 0 }
    }

    pub fn items(&self) -> &[T] {
        &self.items
    }

    pub fn selected_index(&self) -> usize {
        self.selected
    }

    pub fn selected(&self) -> &T {
        &self.items[self.selected]
    }
}

impl<T: Clone + Display> Widget for SelectList<T> {
    type Output = T;

    fn on_key(&mut self, key: KeyEvent) -> Step<T> {
        if is_cancel(&key) {
            return Step::Cancel;
        }
        let n = self.items.len();
        match key.code {
            KeyCode::Up | KeyCode::BackTab => self.selected = (self.selected + n - 1) % n,
            KeyCode::Down | KeyCode::Tab => self.selected = (self.selected + 1) % n,
            KeyCode::Home => self.selected = 0,
            KeyCode::End => self.selected = n - 1,
            KeyCode::Enter => return Step::Done(self.selected().clone()),
            _ => {}
        }
        Step::Continue
    }

    fn render(&self) -> Vec<Line> {
        self.items
            .iter()
            .enumerate()
            .map(|(i, item)| {
                if i == self.selected {
                    Line::styled(Style::new().bold().fg(Color::Cyan), format!("> {item}"))
                } else {
                    Line::plain(format!("  {item}"))
                }
            })
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ui::test_keys::{ctrl, enter, feed, key};

    fn texts(w: &SelectList<&str>) -> Vec<String> {
        w.render().iter().map(Line::text).collect()
    }

    #[test]
    fn arrows_and_enter_select() {
        let mut w = SelectList::new(["sqlite", "mysql"]);
        assert_eq!(texts(&w), ["> sqlite", "  mysql"]);
        assert_eq!(
            feed(&mut w, [key(KeyCode::Down), enter()]),
            Step::Done("mysql")
        );
        assert_eq!(texts(&w), ["  sqlite", "> mysql"]);
    }

    #[test]
    fn selection_wraps() {
        let mut w = SelectList::new(["a", "b", "c"]);
        assert_eq!(feed(&mut w, [key(KeyCode::Up), enter()]), Step::Done("c"));
        assert_eq!(feed(&mut w, [key(KeyCode::Down), enter()]), Step::Done("a"));
    }

    #[test]
    fn esc_and_ctrl_c_cancel() {
        assert_eq!(
            feed(&mut SelectList::new(["a"]), [key(KeyCode::Esc)]),
            Step::Cancel
        );
        assert_eq!(feed(&mut SelectList::new(["a"]), [ctrl('c')]), Step::Cancel);
    }
}

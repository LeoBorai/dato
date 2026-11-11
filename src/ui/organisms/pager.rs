//! Shows output wider (or taller) than the terminal in a scrollable viewport
//! with a status line, instead of letting the terminal wrap it.

use crossterm::event::KeyEvent;
use nu_ansi_term::{Color, Style};
use unicode_width::UnicodeWidthStr;

use crate::ui::atoms::Viewport;
use crate::ui::{Line, Step, Widget};

/// Whether `text` has a line wider than `columns` terminal cells.
pub fn too_wide(text: &str, columns: usize) -> bool {
    text.lines().any(|l| l.width() > columns)
}

pub struct Pager {
    view: Viewport,
}

impl Pager {
    /// A pager for `text` on a terminal of `columns` x `rows`. One row is kept
    /// for the status line and one so the prompt stays on screen.
    pub fn new(text: &str, columns: usize, rows: usize) -> Self {
        // One cell less than the terminal so a full line never auto-wraps.
        let width = columns.saturating_sub(1).max(1);
        let lines = text.lines().count();
        let height = lines.min(rows.saturating_sub(2)).max(1);
        Self {
            view: Viewport::new(text, width, height),
        }
    }

    fn status(&self) -> Line {
        let (c1, c2, ct) = self.view.columns();
        let mut parts = vec![format!("cols {c1}-{c2} of {ct}")];
        if self.view.can_scroll_vertically() {
            let (r1, r2, rt) = self.view.rows();
            parts.push(format!("lines {r1}-{r2} of {rt}"));
        }
        let mut keys = Vec::new();
        if self.view.can_scroll_horizontally() {
            keys.push("Left/Right scroll");
        }
        if self.view.can_scroll_vertically() {
            keys.push("Up/Down");
        }
        keys.push("q to close");
        Line::styled(
            Style::new().fg(Color::DarkGray),
            format!("-- {} -- {}", parts.join(", "), keys.join(", ")),
        )
    }
}

impl Widget for Pager {
    type Output = ();

    fn on_key(&mut self, key: KeyEvent) -> Step<()> {
        self.view.on_key(key)
    }

    fn render(&self) -> Vec<Line> {
        let mut out = self.view.render();
        out.push(self.status());
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ui::test_keys::{feed, key};
    use crossterm::event::KeyCode;

    #[test]
    fn width_check() {
        assert!(too_wide("short\n0123456789", 9));
        assert!(!too_wide("short\n0123456789", 10));
        // Display width, not bytes.
        assert!(!too_wide("日本", 4));
        assert!(too_wide("日本", 3));
    }

    #[test]
    fn clips_to_terminal_and_scrolls_right() {
        let table = "+------+------+------+\n| aaaa | bbbb | cccc |\n+------+------+------+";
        let mut p = Pager::new(table, 12, 24);
        let lines: Vec<_> = p.render().iter().map(Line::text).collect();
        assert_eq!(lines[1], "| aaaa | bb");
        assert!(lines[3].contains("cols 1-11 of 22") && lines[3].contains("Left/Right"));
        assert!(!lines[3].contains("Up/Down"));
        assert_eq!(lines.len(), 4);

        feed(&mut p, [key(KeyCode::Right)]);
        assert_eq!(p.render()[1].text(), " bbbb | ccc");
        assert_eq!(feed(&mut p, [key(KeyCode::Char('q'))]), Step::Done(()));
    }

    #[test]
    fn tall_output_fits_the_terminal() {
        let text = (0..100)
            .map(|i| format!("{i:>30}"))
            .collect::<Vec<_>>()
            .join("\n");
        let p = Pager::new(&text, 20, 10);
        let lines = p.render();
        // 8 content lines + status, leaving a row for the prompt.
        assert_eq!(lines.len(), 9);
        assert!(lines[8].text().contains("lines 1-8 of 100"));
    }
}

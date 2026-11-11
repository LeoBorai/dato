//! A fixed-size window over lines of text, scrolled with the arrow keys.
//!
//! Lines are clipped by display width (wide characters count as two cells), so
//! rows wider than the window stay aligned instead of wrapping.

use crossterm::event::{KeyCode, KeyEvent};
use unicode_width::{UnicodeWidthChar, UnicodeWidthStr};

use crate::ui::{Line, Step, Widget, is_cancel};

/// Cells moved per Left/Right press.
pub const H_STEP: usize = 8;

#[derive(Debug, Clone)]
pub struct Viewport {
    lines: Vec<String>,
    width: usize,
    height: usize,
    /// First visible display column.
    left: usize,
    /// First visible line.
    top: usize,
    content_width: usize,
}

/// The cells `[left, left + width)` of `s`. A wide character cut by either edge
/// is replaced by spaces so columns stay aligned.
pub fn clip(s: &str, left: usize, width: usize) -> String {
    let mut out = String::new();
    let mut col = 0;
    let end = left + width;
    for c in s.chars() {
        let w = c.width().unwrap_or(0);
        let (start, stop) = (col, col + w);
        col = stop;
        if stop <= left {
            continue;
        }
        if start >= end {
            break;
        }
        if start < left || stop > end {
            // Partly visible wide character.
            let visible = stop.min(end) - start.max(left);
            out.extend(std::iter::repeat_n(' ', visible));
        } else {
            out.push(c);
        }
    }
    out
}

impl Viewport {
    /// A `width` x `height` window over `text`, at the top-left corner.
    pub fn new(text: &str, width: usize, height: usize) -> Self {
        let lines: Vec<String> = text.lines().map(str::to_string).collect();
        let content_width = lines.iter().map(|l| l.width()).max().unwrap_or(0);
        Self {
            lines,
            width: width.max(1),
            height: height.max(1),
            left: 0,
            top: 0,
            content_width,
        }
    }

    fn max_left(&self) -> usize {
        self.content_width.saturating_sub(self.width)
    }

    fn max_top(&self) -> usize {
        self.lines.len().saturating_sub(self.height)
    }

    /// `(first column, last column, total columns)`, 1-based, for a status line.
    pub fn columns(&self) -> (usize, usize, usize) {
        let last = (self.left + self.width).min(self.content_width);
        (self.left + 1, last, self.content_width)
    }

    /// `(first line, last line, total lines)`, 1-based.
    pub fn rows(&self) -> (usize, usize, usize) {
        let last = (self.top + self.height).min(self.lines.len());
        (self.top + 1, last, self.lines.len())
    }

    pub fn can_scroll_horizontally(&self) -> bool {
        self.max_left() > 0
    }

    pub fn can_scroll_vertically(&self) -> bool {
        self.max_top() > 0
    }
}

impl Widget for Viewport {
    type Output = ();

    /// Arrows scroll; Home/End jump to the left/right edge; PageUp/PageDown and
    /// `g`/`G` move vertically; `q`, Enter, Esc or Ctrl-C close.
    fn on_key(&mut self, key: KeyEvent) -> Step<()> {
        if is_cancel(&key) {
            return Step::Done(());
        }
        match key.code {
            KeyCode::Char('q') | KeyCode::Enter => return Step::Done(()),
            KeyCode::Right | KeyCode::Char('l') => {
                self.left = (self.left + H_STEP).min(self.max_left())
            }
            KeyCode::Left | KeyCode::Char('h') => self.left = self.left.saturating_sub(H_STEP),
            KeyCode::Home => self.left = 0,
            KeyCode::End => self.left = self.max_left(),
            KeyCode::Down | KeyCode::Char('j') => self.top = (self.top + 1).min(self.max_top()),
            KeyCode::Up | KeyCode::Char('k') => self.top = self.top.saturating_sub(1),
            KeyCode::PageDown | KeyCode::Char(' ') => {
                self.top = (self.top + self.height).min(self.max_top())
            }
            KeyCode::PageUp => self.top = self.top.saturating_sub(self.height),
            KeyCode::Char('g') => self.top = 0,
            KeyCode::Char('G') => self.top = self.max_top(),
            _ => {}
        }
        Step::Continue
    }

    fn render(&self) -> Vec<Line> {
        self.lines
            .iter()
            .skip(self.top)
            .take(self.height)
            .map(|l| Line::plain(clip(l, self.left, self.width)))
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ui::test_keys::{enter, feed, key};

    fn shown(v: &Viewport) -> Vec<String> {
        v.render().iter().map(Line::text).collect()
    }

    #[test]
    fn clip_by_display_width() {
        assert_eq!(clip("abcdef", 2, 3), "cde");
        assert_eq!(clip("abc", 2, 5), "c");
        assert_eq!(clip("abc", 5, 5), "");
        // `日` is two cells wide: cut by an edge it becomes a space.
        assert_eq!(clip("a日b", 0, 2), "a ");
        assert_eq!(clip("a日b", 2, 2), " b");
        assert_eq!(clip("a日b", 1, 2), "日");
    }

    #[test]
    fn right_arrow_scrolls_horizontally_within_bounds() {
        let text = "0123456789abcdefghij\nshort";
        let mut v = Viewport::new(text, 10, 5);
        assert!(v.can_scroll_horizontally());
        assert_eq!(shown(&v), ["0123456789", "short"]);
        feed(&mut v, [key(KeyCode::Right)]);
        assert_eq!(shown(&v), ["89abcdefgh", ""]);
        // Stops at the right edge: the last column stays visible.
        feed(&mut v, [key(KeyCode::Right); 5]);
        assert_eq!(shown(&v)[0], "abcdefghij");
        assert_eq!(v.columns(), (11, 20, 20));
        feed(&mut v, [key(KeyCode::Left)]);
        assert_eq!(shown(&v)[0], "23456789ab");
        feed(&mut v, [key(KeyCode::Home)]);
        assert_eq!(shown(&v)[0], "0123456789");
        feed(&mut v, [key(KeyCode::End)]);
        assert_eq!(shown(&v)[0], "abcdefghij");
    }

    #[test]
    fn vertical_scrolling() {
        let text = (1..=10)
            .map(|i| i.to_string())
            .collect::<Vec<_>>()
            .join("\n");
        let mut v = Viewport::new(&text, 5, 3);
        assert_eq!(shown(&v), ["1", "2", "3"]);
        feed(&mut v, [key(KeyCode::Down)]);
        assert_eq!(shown(&v), ["2", "3", "4"]);
        feed(
            &mut v,
            [
                key(KeyCode::PageDown),
                key(KeyCode::PageDown),
                key(KeyCode::PageDown),
            ],
        );
        assert_eq!(shown(&v), ["8", "9", "10"]);
        assert_eq!(v.rows(), (8, 10, 10));
        feed(&mut v, [key(KeyCode::Char('g'))]);
        assert_eq!(v.rows(), (1, 3, 10));
    }

    #[test]
    fn quit_keys() {
        for k in [key(KeyCode::Char('q')), key(KeyCode::Esc), enter()] {
            assert_eq!(feed(&mut Viewport::new("x", 1, 1), [k]), Step::Done(()));
        }
    }
}

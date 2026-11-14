//! A text field for file paths: typing `@` opens a menu of files and folders
//! to pick from.

use std::path::PathBuf;

use crossterm::event::{KeyCode, KeyEvent};
use nu_ansi_term::{Color, Style};

use crate::ui::atoms::TextInput;
use crate::ui::{Line, Step, Widget};

/// Entries shown at once; the menu scrolls past that.
const VISIBLE: usize = 8;

#[derive(Debug, Clone, PartialEq, Eq)]
struct Entry {
    name: String,
    dir: bool,
}

#[derive(Debug, Clone)]
pub struct PathInput {
    input: TextInput,
    selected: usize,
    /// Esc closed the menu; it opens again on the next edit.
    dismissed: bool,
}

/// `~` and `~/..` mean the home directory; an empty path is the current one.
fn expand(dir: &str) -> PathBuf {
    if dir.is_empty() {
        PathBuf::from(".")
    } else {
        crate::paths::expand_tilde(dir)
    }
}

/// Splits `query` into its folder part (with the trailing `/`) and the name
/// typed so far.
fn split_query(query: &str) -> (&str, &str) {
    match query.rfind('/') {
        Some(i) => query.split_at(i + 1),
        None => ("", query),
    }
}

/// Folders first, then files, each sorted by name. Hidden entries only show
/// once the name typed so far starts with a dot.
fn list(query: &str) -> Vec<Entry> {
    let (dir, prefix) = split_query(query);
    let Ok(read) = std::fs::read_dir(expand(dir)) else {
        return Vec::new();
    };
    let mut entries: Vec<Entry> =
        read.flatten()
            .filter_map(|e| {
                let name = e.file_name().to_string_lossy().into_owned();
                (name.starts_with(prefix) && (prefix.starts_with('.') || !name.starts_with('.')))
                    .then(|| Entry {
                        dir: e.path().is_dir(),
                        name,
                    })
            })
            .collect();
    entries.sort_by(|a, b| b.dir.cmp(&a.dir).then_with(|| a.name.cmp(&b.name)));
    entries
}

impl PathInput {
    pub fn new(default: &str) -> Self {
        Self {
            input: TextInput::new(default),
            selected: 0,
            dismissed: false,
        }
    }

    pub fn with_placeholder(mut self, placeholder: &str) -> Self {
        self.input = self.input.with_placeholder(placeholder);
        self
    }

    pub fn value(&self) -> String {
        self.input.value()
    }

    /// The finished field, without the cursor.
    pub fn render_value(&self) -> Line {
        self.input.render_value()
    }

    /// Where the `@` that opened the menu is, and what follows it up to the
    /// cursor. The `@` must start a word.
    fn token(&self) -> Option<(usize, String)> {
        let before = self.input.before_cursor();
        let at = before.iter().rposition(|c| *c == '@')?;
        let word_start = at == 0 || before[at - 1].is_whitespace();
        let query: String = before[at + 1..].iter().collect();
        (word_start && !query.contains(char::is_whitespace)).then_some((at, query))
    }

    /// The open menu: the token and its matches. `None` when closed.
    fn menu(&self) -> Option<(usize, String, Vec<Entry>)> {
        if self.dismissed {
            return None;
        }
        let (at, query) = self.token()?;
        let entries = list(&query);
        Some((at, query, entries))
    }

    /// Puts the picked entry in place of the `@` word. A folder keeps the `@`
    /// so the menu goes on into it; a file ends the word.
    fn accept(&mut self, at: usize, query: &str, entry: &Entry) {
        let (dir, _) = split_query(query);
        let end = at + 1 + query.chars().count();
        let text = if entry.dir {
            format!("@{dir}{}/", entry.name)
        } else {
            format!("{dir}{}", entry.name)
        };
        self.input.replace(at..end, &text);
    }
}

impl Widget for PathInput {
    type Output = String;

    fn on_key(&mut self, key: KeyEvent) -> Step<String> {
        if let Some((at, query, entries)) = self.menu() {
            let n = entries.len();
            match key.code {
                KeyCode::Esc => {
                    self.dismissed = true;
                    return Step::Continue;
                }
                KeyCode::Up | KeyCode::BackTab if n > 0 => {
                    self.selected = (self.selected + n - 1) % n;
                    return Step::Continue;
                }
                KeyCode::Down | KeyCode::Tab if n > 0 => {
                    self.selected = (self.selected + 1) % n;
                    return Step::Continue;
                }
                KeyCode::Enter | KeyCode::Right if n > 0 => {
                    let entry = &entries[self.selected.min(n - 1)];
                    self.accept(at, &query, entry);
                    self.selected = 0;
                    return Step::Continue;
                }
                _ => {}
            }
        }
        self.selected = 0;
        self.dismissed = false;
        match self.input.on_key(key) {
            Step::Done(_) => {
                // Nothing picked: the `@` was only a trigger.
                if let Some((at, _)) = self.token() {
                    self.input.replace(at..at + 1, "");
                }
                Step::Done(self.value())
            }
            other => other,
        }
    }

    fn render(&self) -> Vec<Line> {
        let mut out = self.input.render();
        let Some((_, _, entries)) = self.menu() else {
            return out;
        };
        let dim = Style::new().fg(Color::DarkGray);
        if entries.is_empty() {
            out.push(Line::styled(dim, "  no matching files"));
            return out;
        }
        let start = (self.selected + 1).saturating_sub(VISIBLE);
        let end = (start + VISIBLE).min(entries.len());
        for (i, e) in entries.iter().enumerate().take(end).skip(start) {
            let on = i == self.selected;
            let color = if e.dir { Color::Cyan } else { Color::Default };
            let style = if on {
                Style::new().bold().fg(color)
            } else {
                Style::new().fg(color)
            };
            let slash = if e.dir { "/" } else { "" };
            out.push(Line::styled(
                style,
                format!("  {} {}{slash}", if on { ">" } else { " " }, e.name),
            ));
        }
        if end < entries.len() {
            out.push(Line::styled(
                dim,
                format!("    ... {} more", entries.len() - end),
            ));
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ui::test_keys::{enter, feed, key, typed};

    /// `dir/` with `sub/`, `a.db`, `b.txt` and a hidden `.secret`.
    fn tree() -> (tempfile::TempDir, String) {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir(dir.path().join("sub")).unwrap();
        std::fs::write(dir.path().join("sub/inner.db"), "").unwrap();
        for f in ["a.db", "b.txt", ".secret"] {
            std::fs::write(dir.path().join(f), "").unwrap();
        }
        let base = format!("{}/", dir.path().display());
        (dir, base)
    }

    fn screen(w: &PathInput) -> Vec<String> {
        w.render().iter().map(Line::text).collect()
    }

    #[test]
    fn no_menu_without_an_at_sign() {
        let (_d, base) = tree();
        let mut w = PathInput::new("");
        feed(&mut w, typed(&base));
        assert_eq!(screen(&w).len(), 1);
    }

    #[test]
    fn at_opens_the_menu_folders_first_and_hides_dotfiles() {
        let (_d, base) = tree();
        let mut w = PathInput::new("");
        feed(&mut w, typed(&format!("@{base}")));
        let s = screen(&w);
        assert_eq!(&s[1..], ["  > sub/", "    a.db", "    b.txt"], "{s:?}");
        feed(&mut w, typed("."));
        assert!(screen(&w).iter().any(|l| l.contains(".secret")));
    }

    #[test]
    fn typing_filters_and_enter_picks_a_file() {
        let (_d, base) = tree();
        let mut w = PathInput::new("");
        feed(&mut w, typed(&format!("@{base}b")));
        assert_eq!(screen(&w)[1..], ["  > b.txt"]);
        assert_eq!(feed(&mut w, [enter()]), Step::Continue);
        // The `@` is gone and the field holds the plain path.
        assert_eq!(w.value(), format!("{base}b.txt"));
        assert_eq!(screen(&w).len(), 1);
        assert_eq!(feed(&mut w, [enter()]), Step::Done(format!("{base}b.txt")));
    }

    #[test]
    fn arrows_move_and_a_folder_goes_deeper() {
        let (_d, base) = tree();
        let mut w = PathInput::new("");
        feed(&mut w, typed(&format!("@{base}")));
        feed(
            &mut w,
            [key(KeyCode::Down), key(KeyCode::Up), key(KeyCode::Tab)],
        );
        feed(&mut w, [key(KeyCode::Up)]);
        assert!(screen(&w)[1].starts_with("  > sub/"), "{:?}", screen(&w));
        feed(&mut w, [enter()]);
        assert_eq!(w.value(), format!("@{base}sub/"));
        assert_eq!(screen(&w)[1..], ["  > inner.db"]);
        feed(&mut w, [key(KeyCode::Tab), enter()]);
        assert_eq!(w.value(), format!("{base}sub/inner.db"));
    }

    #[test]
    fn esc_closes_the_menu_then_cancels() {
        let (_d, base) = tree();
        let mut w = PathInput::new("");
        feed(&mut w, typed(&format!("@{base}")));
        assert_eq!(feed(&mut w, [key(KeyCode::Esc)]), Step::Continue);
        assert_eq!(screen(&w).len(), 1);
        assert_eq!(feed(&mut w, [key(KeyCode::Esc)]), Step::Cancel);
    }

    #[test]
    fn no_match_says_so_and_enter_drops_the_trigger() {
        let (_d, base) = tree();
        let mut w = PathInput::new("");
        feed(&mut w, typed(&format!("@{base}new.db")));
        assert_eq!(screen(&w)[1], "  no matching files");
        assert_eq!(feed(&mut w, [enter()]), Step::Done(format!("{base}new.db")));
    }

    #[test]
    fn at_inside_a_word_is_not_a_trigger() {
        let mut w = PathInput::new("");
        feed(&mut w, typed("me@host"));
        assert_eq!(screen(&w).len(), 1);
        assert_eq!(feed(&mut w, [enter()]), Step::Done("me@host".into()));
    }
}

//! SQL syntax highlighting.

use nu_ansi_term::{Color, Style};
use reedline::{Highlighter, StyledText};

use super::keywords::is_keyword;
use crate::sql::{TokenKind, tokenize};

fn style_for(kind: TokenKind, text: &str, at_line_start: bool) -> Style {
    match kind {
        TokenKind::Word if is_keyword(text) => Style::new().bold().fg(Color::Blue),
        TokenKind::String => Style::new().fg(Color::Green),
        TokenKind::QuotedIdent => Style::new().fg(Color::Yellow),
        TokenKind::Number => Style::new().fg(Color::Purple),
        TokenKind::LineComment | TokenKind::BlockComment => Style::new().fg(Color::DarkGray),
        TokenKind::Punct | TokenKind::Word if at_line_start && text.starts_with(['.', '\\']) => {
            Style::new().fg(Color::Cyan)
        }
        _ => Style::new(),
    }
}

/// Styled spans covering `line` exactly, in order.
pub fn spans(line: &str) -> Vec<(Style, String)> {
    let mut out: Vec<(Style, String)> = Vec::new();
    for tok in tokenize(line) {
        let text = tok.text(line);
        let at_start = line[..tok.start].trim().is_empty();
        out.push((style_for(tok.kind, text, at_start), text.to_string()));
    }
    out
}

pub struct SqlHighlighter;

impl Highlighter for SqlHighlighter {
    fn highlight(&self, line: &str, _cursor: usize) -> StyledText {
        let mut st = StyledText::new();
        st.buffer = spans(line);
        st
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn keyword_and_string_get_distinct_styles() {
        let s = spans("select 'x'");
        let kw = s.iter().find(|(_, t)| t == "select").unwrap().0;
        let st = s.iter().find(|(_, t)| t == "'x'").unwrap().0;
        assert_ne!(kw, st);
        assert_ne!(kw, Style::new());
    }

    #[test]
    fn spans_cover_input() {
        let line = "select 1 -- c\n, \"a\"";
        let joined: String = spans(line).into_iter().map(|(_, t)| t).collect();
        assert_eq!(joined, line);
    }

    #[test]
    fn numbers_and_comments_are_styled() {
        let s = spans("1 -- c");
        assert_ne!(s[0].0, Style::new());
        assert_ne!(s.last().unwrap().0, Style::new());
        assert_ne!(s[0].0, s.last().unwrap().0);
    }
}

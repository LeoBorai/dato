//! SQL syntax highlighting.

use std::sync::{Arc, RwLock};

use nu_ansi_term::{Color, Style};
use reedline::{Highlighter, StyledText};

use crate::dialect::Dialect;
use crate::server::Server;
use crate::sql::{TokenKind, tokenize_with};

fn style_for(server: Server, kind: TokenKind, text: &str, at_line_start: bool) -> Style {
    match kind {
        TokenKind::Word if server.is_keyword(text) => Style::new().bold().fg(Color::Blue),
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

/// Styled spans covering `line` exactly, in order, using SQLite rules.
pub fn spans(line: &str) -> Vec<(Style, String)> {
    spans_with(line, Server::Sqlite)
}

/// Styled spans covering `line` exactly, in order, for `server`'s dialect.
pub fn spans_with(line: &str, server: Server) -> Vec<(Style, String)> {
    let mut out: Vec<(Style, String)> = Vec::new();
    for tok in tokenize_with(line, &server.lex_rules()) {
        let text = tok.text(line);
        let at_start = line[..tok.start].trim().is_empty();
        out.push((
            style_for(server, tok.kind, text, at_start),
            text.to_string(),
        ));
    }
    out
}

/// Highlights with the dialect of the session's current server.
pub struct SqlHighlighter {
    dialect: Arc<RwLock<Server>>,
}

impl SqlHighlighter {
    pub fn new(dialect: Arc<RwLock<Server>>) -> Self {
        Self { dialect }
    }
}

impl Highlighter for SqlHighlighter {
    fn highlight(&self, line: &str, _cursor: usize) -> StyledText {
        let mut st = StyledText::new();
        st.buffer = spans_with(line, *self.dialect.read().unwrap());
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
    fn double_quotes_follow_the_dialect() {
        let style = |server, text: &str| {
            spans_with("select \"x\" # c", server)
                .into_iter()
                .find(|(_, t)| t == text)
                .map(|(s, _)| s)
        };
        let string = spans("'s'")[0].0;
        let ident = spans("\"i\"")[0].0;
        let comment = spans("-- c")[0].0;
        assert_eq!(style(Server::Sqlite, "\"x\""), Some(ident));
        assert_eq!(style(Server::MySql, "\"x\""), Some(string));
        assert_eq!(style(Server::MySql, "# c"), Some(comment));
        assert_eq!(style(Server::Sqlite, "# c"), None);
        // Keywords come from the server's list.
        let show = |server| spans_with("show", server)[0].0;
        assert_ne!(show(Server::MySql), Style::new());
        assert_eq!(show(Server::Sqlite), Style::new());
    }

    #[test]
    fn dollar_quoted_body_is_a_string_on_postgres() {
        let string = spans("'s'")[0].0;
        let comment = spans("-- c")[0].0;
        let s = spans_with("do $$ x; y $$", Server::PostgreSql);
        assert!(s.iter().any(|(st, t)| t == "$$ x; y $$" && *st == string));
        let c = spans_with("select 1 /* a /* b */ c */", Server::PostgreSql);
        assert_eq!(c.last().unwrap().0, comment);
        assert_eq!(c.last().unwrap().1, "/* a /* b */ c */");
        let ident = spans("\"i\"")[0].0;
        let q = spans_with("select \"x\"", Server::PostgreSql);
        assert_eq!(q.last().unwrap().0, ident);
    }

    #[test]
    fn numbers_and_comments_are_styled() {
        let s = spans("1 -- c");
        assert_ne!(s[0].0, Style::new());
        assert_ne!(s.last().unwrap().0, Style::new());
        assert_ne!(s[0].0, s.last().unwrap().0);
    }
}

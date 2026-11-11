//! Shared SQL lexer: statement splitting, completeness checks and token spans.
//!
//! The `*_with` functions take the [`LexRules`] of the session's server; the
//! plain functions use the SQLite rules.

use crate::dialect::{LexRules, sqlite};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TokenKind {
    Whitespace,
    Word,
    Number,
    String,
    QuotedIdent,
    LineComment,
    BlockComment,
    Punct,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Token {
    pub kind: TokenKind,
    pub start: usize,
    pub end: usize,
    /// `false` for a string, identifier or block comment cut off by end of input.
    pub terminated: bool,
}

impl Token {
    pub fn text<'a>(&self, src: &'a str) -> &'a str {
        &src[self.start..self.end]
    }
}

fn is_word_byte(b: u8) -> bool {
    b.is_ascii_alphanumeric() || b == b'_' || b == b'$' || b >= 0x80
}

/// Scans a quoted run starting at `i` (the opening quote) with `close` as terminator,
/// a doubled close as an escape (`''`) when `doubling`, and `\x` as an escape when
/// `backslash`. Returns (end, terminated).
fn scan_quoted(b: &[u8], i: usize, close: u8, doubling: bool, backslash: bool) -> (usize, bool) {
    let mut j = i + 1;
    while j < b.len() {
        if backslash && b[j] == b'\\' {
            j += 2;
            continue;
        }
        if b[j] == close {
            if doubling && b.get(j + 1) == Some(&close) {
                j += 2;
                continue;
            }
            return (j + 1, true);
        }
        j += 1;
    }
    (b.len(), false)
}

/// Scans a dollar-quoted string starting at `i` (a `$`). Returns `None` when `$`
/// does not open one (`$1`, a lone `$`), else (end, terminated).
fn scan_dollar_quoted(b: &[u8], i: usize) -> Option<(usize, bool)> {
    let mut j = i + 1;
    // The tag is empty or starts with a letter or underscore, so `$1` is not one.
    if b.get(j).is_some_and(|c| c.is_ascii_digit()) {
        return None;
    }
    while j < b.len() && (b[j].is_ascii_alphanumeric() || b[j] == b'_' || b[j] >= 0x80) {
        j += 1;
    }
    if b.get(j) != Some(&b'$') {
        return None;
    }
    let tag = &b[i..=j];
    let body = j + 1;
    match b[body..].windows(tag.len()).position(|w| w == tag) {
        Some(p) => Some((body + p + tag.len(), true)),
        None => Some((b.len(), false)),
    }
}

/// Scans a block comment starting at `i` (`/*`). Returns (end, terminated).
fn scan_block_comment(b: &[u8], i: usize, nested: bool) -> (usize, bool) {
    let mut j = i + 2;
    let mut depth = 1usize;
    while j < b.len() {
        if b[j] == b'*' && b.get(j + 1) == Some(&b'/') {
            j += 2;
            depth -= 1;
            if depth == 0 {
                return (j, true);
            }
        } else if nested && b[j] == b'/' && b.get(j + 1) == Some(&b'*') {
            j += 2;
            depth += 1;
        } else {
            j += 1;
        }
    }
    (b.len(), false)
}

/// Splits `src` into contiguous tokens covering every byte, using SQLite rules.
pub fn tokenize(src: &str) -> Vec<Token> {
    tokenize_with(src, &sqlite::RULES)
}

/// Splits `src` into contiguous tokens covering every byte.
pub fn tokenize_with(src: &str, rules: &LexRules) -> Vec<Token> {
    let b = src.as_bytes();
    let mut out = Vec::new();
    let mut i = 0;
    while i < b.len() {
        let c = b[i];
        let (kind, end, terminated) = if c.is_ascii_whitespace() {
            let mut j = i;
            while j < b.len() && b[j].is_ascii_whitespace() {
                j += 1;
            }
            (TokenKind::Whitespace, j, true)
        } else if rules.escape_strings && matches!(c, b'e' | b'E') && b.get(i + 1) == Some(&b'\'') {
            let (e, t) = scan_quoted(b, i + 1, b'\'', true, true);
            (TokenKind::String, e, t)
        } else if c == b'$'
            && rules.dollar_quotes
            && let Some((e, t)) = scan_dollar_quoted(b, i)
        {
            (TokenKind::String, e, t)
        } else if c == b'\'' || (c == b'"' && rules.double_quote_is_string) {
            let (e, t) = scan_quoted(b, i, c, true, rules.backslash_escapes);
            (TokenKind::String, e, t)
        } else if c == b'"' || c == b'`' {
            let (e, t) = scan_quoted(b, i, c, true, false);
            (TokenKind::QuotedIdent, e, t)
        } else if c == b'[' && rules.bracket_idents {
            let (e, t) = scan_quoted(b, i, b']', false, false);
            (TokenKind::QuotedIdent, e, t)
        } else if (c == b'-'
            && b.get(i + 1) == Some(&b'-')
            && (!rules.dash_comment_needs_space
                || b.get(i + 2).is_none_or(|n| n.is_ascii_whitespace())))
            || (c == b'#' && rules.hash_comments)
        {
            let mut j = i;
            while j < b.len() && b[j] != b'\n' {
                j += 1;
            }
            (TokenKind::LineComment, j, true)
        } else if c == b'/' && b.get(i + 1) == Some(&b'*') {
            let (e, t) = scan_block_comment(b, i, rules.nested_block_comments);
            (TokenKind::BlockComment, e, t)
        } else if c.is_ascii_digit() || (c == b'.' && b.get(i + 1).is_some_and(u8::is_ascii_digit))
        {
            let mut j = i + 1;
            while j < b.len() && (b[j].is_ascii_alphanumeric() || b[j] == b'.' || b[j] == b'_') {
                j += 1;
            }
            (TokenKind::Number, j, true)
        } else if is_word_byte(c) {
            let mut j = i;
            while j < b.len() && is_word_byte(b[j]) {
                j += 1;
            }
            (TokenKind::Word, j, true)
        } else {
            (TokenKind::Punct, i + 1, true)
        };
        out.push(Token {
            kind,
            start: i,
            end,
            terminated,
        });
        i = end;
    }
    out
}

struct Scan {
    /// Byte ranges of statements, without their terminating `;`.
    statements: Vec<(usize, usize)>,
    /// A statement has started but no terminating `;` was seen.
    open: bool,
    unterminated: bool,
}

fn scan(src: &str, rules: &LexRules) -> Scan {
    let mut statements = Vec::new();
    let mut unterminated = false;
    let mut start = 0;
    let mut significant = 0usize;
    let mut first_words: Vec<String> = Vec::new();
    let mut in_trigger = false;
    let mut body_started = false;
    let mut case_depth = 0usize;
    let mut end_seen = false;

    for tok in tokenize_with(src, rules) {
        if !tok.terminated {
            unterminated = true;
        }
        match tok.kind {
            TokenKind::Whitespace | TokenKind::LineComment | TokenKind::BlockComment => continue,
            TokenKind::Punct if tok.text(src) == ";" => {
                if in_trigger && !(body_started && end_seen) {
                    significant += 1;
                    end_seen = false;
                    continue;
                }
                if significant > 0 {
                    statements.push((start, tok.start));
                }
                start = tok.end;
                significant = 0;
                first_words.clear();
                in_trigger = false;
                body_started = false;
                case_depth = 0;
                end_seen = false;
                continue;
            }
            _ => {}
        }
        significant += 1;
        end_seen = false;
        if tok.kind == TokenKind::Word {
            let w = tok.text(src).to_ascii_lowercase();
            if first_words.len() < 3 {
                first_words.push(w.clone());
                if rules.trigger_bodies
                    && first_words[0] == "create"
                    && first_words[1..].iter().any(|x| x == "trigger")
                {
                    in_trigger = true;
                }
            }
            if in_trigger {
                match w.as_str() {
                    "begin" => body_started = true,
                    "case" => case_depth += 1,
                    "end" => {
                        if case_depth > 0 {
                            case_depth -= 1;
                        } else if body_started {
                            end_seen = true;
                        }
                    }
                    _ => {}
                }
            }
        }
    }
    let open = significant > 0;
    if open {
        statements.push((start, src.len()));
    }
    Scan {
        statements,
        open,
        unterminated,
    }
}

/// Splits input into statements (trimmed, without the `;`), keeping trigger bodies whole.
/// Uses SQLite rules.
pub fn split_statements(src: &str) -> Vec<&str> {
    split_statements_with(src, &sqlite::RULES)
}

/// Splits input into statements (trimmed, without the `;`).
pub fn split_statements_with<'a>(src: &'a str, rules: &LexRules) -> Vec<&'a str> {
    scan(src, rules)
        .statements
        .into_iter()
        .map(|(s, e)| src[s..e].trim())
        .filter(|s| !s.is_empty())
        .collect()
}

/// Detects a trailing `\G`. Returns the input without it and whether it was present.
pub fn split_vertical(input: &str) -> (&str, bool) {
    let t = input.trim_end();
    match t.strip_suffix("\\G") {
        Some(rest) => (rest, true),
        None => (input, false),
    }
}

/// Whether Enter should submit `input` (vs. insert a newline), using SQLite rules.
pub fn is_complete(input: &str) -> bool {
    is_complete_with(input, &sqlite::RULES)
}

/// Whether Enter should submit `input` (vs. insert a newline).
pub fn is_complete_with(input: &str, rules: &LexRules) -> bool {
    let t = input.trim();
    if t.is_empty() || crate::special::is_special(t) {
        return true;
    }
    if t.ends_with("\\G") || t.ends_with("\\e") {
        return true;
    }
    let s = scan(input, rules);
    !s.open && !s.unterminated
}

#[cfg(test)]
mod tests {
    use super::*;

    fn kinds(src: &str) -> Vec<(TokenKind, &str)> {
        tokenize(src)
            .into_iter()
            .filter(|t| t.kind != TokenKind::Whitespace)
            .map(|t| (t.kind, t.text(src)))
            .collect()
    }

    #[test]
    fn token_kinds() {
        use TokenKind::*;
        assert_eq!(
            kinds("select 'a''b', \"x y\", [z], 1.5e3 -- c\n/* d */ ;"),
            vec![
                (Word, "select"),
                (String, "'a''b'"),
                (Punct, ","),
                (QuotedIdent, "\"x y\""),
                (Punct, ","),
                (QuotedIdent, "[z]"),
                (Punct, ","),
                (Number, "1.5e3"),
                (LineComment, "-- c"),
                (BlockComment, "/* d */"),
                (Punct, ";"),
            ]
        );
    }

    #[test]
    fn tokens_cover_input() {
        let src = "select 'é' -- x\n, `a` /* y";
        let joined: String = tokenize(src).iter().map(|t| t.text(src)).collect();
        assert_eq!(joined, src);
    }

    #[test]
    fn unterminated_flags() {
        assert!(!tokenize("'abc")[0].terminated);
        assert!(!tokenize("/* abc")[0].terminated);
    }

    #[test]
    fn split_basic_and_trailing() {
        assert_eq!(
            split_statements("select 1; select 2"),
            vec!["select 1", "select 2"]
        );
        assert_eq!(split_statements(";; select 1;;"), vec!["select 1"]);
        assert!(split_statements("-- only comment").is_empty());
    }

    #[test]
    fn split_ignores_semicolons_in_strings_and_comments() {
        assert_eq!(
            split_statements("select 'a;b'; select 2 -- x;y\n; select /* ; */ 3;"),
            vec!["select 'a;b'", "select 2 -- x;y", "select /* ; */ 3"]
        );
    }

    #[test]
    fn split_keeps_trigger_whole() {
        let sql = "create trigger tr after insert on t begin \
                   update t set a = case when 1 then 2 end; insert into log values (1); end; select 1;";
        let parts = split_statements(sql);
        assert_eq!(parts.len(), 2);
        assert!(parts[0].starts_with("create trigger") && parts[0].ends_with("end"));
        assert_eq!(parts[1], "select 1");
    }

    #[test]
    fn vertical_suffix() {
        assert_eq!(split_vertical("select 1\\G"), ("select 1", true));
        assert_eq!(split_vertical("select 1;"), ("select 1;", false));
    }

    const MYSQL: LexRules = crate::dialect::mysql::RULES;

    #[test]
    fn mysql_strings_and_comments() {
        use TokenKind::*;
        let src = "select 'it\\'s; fine', \"a;b\", `c d`, [e] # note; x\n-- y\n--z";
        let k: Vec<_> = tokenize_with(src, &MYSQL)
            .into_iter()
            .filter(|t| t.kind != Whitespace)
            .map(|t| (t.kind, t.text(src)))
            .collect();
        assert_eq!(
            k,
            vec![
                (Word, "select"),
                (String, "'it\\'s; fine'"),
                (Punct, ","),
                (String, "\"a;b\""),
                (Punct, ","),
                (QuotedIdent, "`c d`"),
                (Punct, ","),
                (Punct, "["),
                (Word, "e"),
                (Punct, "]"),
                (LineComment, "# note; x"),
                (LineComment, "-- y"),
                (Punct, "-"),
                (Punct, "-"),
                (Word, "z"),
            ]
        );
    }

    #[test]
    fn mysql_splitting() {
        assert_eq!(
            split_statements_with("select 'it\\'s; fine'; select 2", &MYSQL),
            vec!["select 'it\\'s; fine'", "select 2"]
        );
        assert_eq!(
            split_statements_with("select 1; # note; more", &MYSQL),
            vec!["select 1"]
        );
        assert!(is_complete_with("select 1; # note; more", &MYSQL));
        assert!(!is_complete_with("select \"a;b\"", &MYSQL));
        assert!(!is_complete("select \"a;b\""));
        // No trigger-body handling on MySQL: the inner `;` ends the statement.
        assert_eq!(
            split_statements_with(
                "create trigger t before insert on a for each row begin set @x = 1; end",
                &MYSQL
            )
            .len(),
            2
        );
    }

    #[test]
    fn completeness() {
        assert!(is_complete(""));
        assert!(is_complete("   "));
        assert!(is_complete("select 1;"));
        assert!(is_complete("select 1; -- trailing"));
        assert!(!is_complete("select 1"));
        assert!(!is_complete("select 'a;b'"));
        assert!(!is_complete("select 1; select 2"));
        assert!(!is_complete("select 'open"));
        assert!(is_complete(".tables"));
        assert!(is_complete("\\d users"));
        assert!(is_complete("select 1\\G"));
        assert!(is_complete("select 1 \\e"));
        assert!(!is_complete(
            "create trigger t after insert on a begin select 1;"
        ));
        assert!(is_complete(
            "create trigger t after insert on a begin select 1; end;"
        ));
    }
}

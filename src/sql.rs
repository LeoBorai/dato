//! Shared SQL lexer: statement splitting, completeness checks and token spans.

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

/// Scans a quoted run starting at `i` (the opening quote) with `close` as terminator
/// and a doubled close as an escape (`''`). Returns (end, terminated).
fn scan_quoted(b: &[u8], i: usize, close: u8, doubling: bool) -> (usize, bool) {
    let mut j = i + 1;
    while j < b.len() {
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

/// Splits `src` into contiguous tokens covering every byte.
pub fn tokenize(src: &str) -> Vec<Token> {
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
        } else if c == b'\'' {
            let (e, t) = scan_quoted(b, i, b'\'', true);
            (TokenKind::String, e, t)
        } else if c == b'"' || c == b'`' {
            let (e, t) = scan_quoted(b, i, c, true);
            (TokenKind::QuotedIdent, e, t)
        } else if c == b'[' {
            let (e, t) = scan_quoted(b, i, b']', false);
            (TokenKind::QuotedIdent, e, t)
        } else if c == b'-' && b.get(i + 1) == Some(&b'-') {
            let mut j = i;
            while j < b.len() && b[j] != b'\n' {
                j += 1;
            }
            (TokenKind::LineComment, j, true)
        } else if c == b'/' && b.get(i + 1) == Some(&b'*') {
            let mut j = i + 2;
            let mut term = false;
            while j < b.len() {
                if b[j] == b'*' && b.get(j + 1) == Some(&b'/') {
                    j += 2;
                    term = true;
                    break;
                }
                j += 1;
            }
            (TokenKind::BlockComment, j.min(b.len()), term)
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

fn scan(src: &str) -> Scan {
    let mut statements = Vec::new();
    let mut unterminated = false;
    let mut start = 0;
    let mut significant = 0usize;
    let mut first_words: Vec<String> = Vec::new();
    let mut in_trigger = false;
    let mut body_started = false;
    let mut case_depth = 0usize;
    let mut end_seen = false;

    for tok in tokenize(src) {
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
                if first_words[0] == "create" && first_words[1..].iter().any(|x| x == "trigger") {
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
pub fn split_statements(src: &str) -> Vec<&str> {
    scan(src)
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

/// Whether Enter should submit `input` (vs. insert a newline).
pub fn is_complete(input: &str) -> bool {
    let t = input.trim();
    if t.is_empty() {
        return true;
    }
    if t.ends_with("\\G") || t.ends_with("\\e") {
        return true;
    }
    let s = scan(input);
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

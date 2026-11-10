//! Works out what the cursor expects, from the statement being typed.
//!
//! A token scan, not a parser: it must cope with the unfinished SQL that is
//! normal while typing.

use super::keywords::KEYWORDS;
use crate::sql::{Token, TokenKind, tokenize};

/// A kind of name the cursor can take.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Expect {
    Relations,
    Columns,
    Keywords,
    Commands,
}

/// A table or view named in the statement, with its alias when it has one.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Reference {
    pub name: String,
    pub alias: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Context {
    /// Byte offset where the typed fragment starts (it ends at the cursor).
    pub start: usize,
    /// The typed fragment, without any `name.` qualifier.
    pub prefix: String,
    /// What to offer, in order. Empty when nothing fits (inside a string or comment).
    pub expect: Vec<Expect>,
    /// `name` of a `name.fragment` being typed.
    pub qualifier: Option<String>,
    /// Relations named anywhere in the current statement, before or after the cursor.
    pub references: Vec<Reference>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Clause {
    None,
    Columns,
    Relations,
    Into,
    Other,
}

fn is_word_char(c: char) -> bool {
    c.is_alphanumeric() || c == '_' || c == '$'
}

fn fragment_start(line: &str, pos: usize) -> usize {
    line[..pos]
        .char_indices()
        .rev()
        .take_while(|(_, c)| is_word_char(*c))
        .last()
        .map_or(pos, |(i, _)| i)
}

fn is_semicolon(line: &str, t: &Token) -> bool {
    t.kind == TokenKind::Punct && t.text(line) == ";"
}

/// Whether the cursor sits inside a string, quoted identifier or comment.
fn in_literal(tokens: &[Token], pos: usize) -> bool {
    tokens.iter().any(|t| {
        if t.start >= pos {
            return false;
        }
        match t.kind {
            TokenKind::String | TokenKind::QuotedIdent | TokenKind::BlockComment => {
                pos < t.end || !t.terminated
            }
            TokenKind::LineComment => pos <= t.end,
            _ => false,
        }
    })
}

fn clause_for(word: &str) -> Option<Clause> {
    Some(match word {
        "SELECT" | "WHERE" | "ON" | "BY" | "HAVING" | "SET" | "AND" | "OR" | "USING"
        | "RETURNING" => Clause::Columns,
        "FROM" | "JOIN" | "UPDATE" | "TABLE" => Clause::Relations,
        "INTO" => Clause::Into,
        "ORDER" | "GROUP" | "LIMIT" | "OFFSET" | "VALUES" | "UNION" | "INTERSECT" | "EXCEPT"
        | "WITH" | "CREATE" | "ALTER" | "DROP" | "INSERT" | "DELETE" => Clause::Other,
        _ => return None,
    })
}

fn is_keyword(word: &str) -> bool {
    KEYWORDS.iter().any(|k| k.eq_ignore_ascii_case(word))
}

fn unquote(text: &str) -> String {
    let mut chars = text.chars();
    let (open, close) = match chars.next() {
        Some('"') => ('"', '"'),
        Some('`') => ('`', '`'),
        Some('[') => ('[', ']'),
        _ => return text.to_string(),
    };
    let inner = &text[1..];
    let inner = inner.strip_suffix(close).unwrap_or(inner);
    if open == '[' {
        inner.to_string()
    } else {
        inner.replace(&format!("{close}{close}"), &close.to_string())
    }
}

/// Reads an identifier token: a bare word that is not a keyword, or a quoted one.
fn ident(line: &str, t: &Token) -> Option<String> {
    match t.kind {
        TokenKind::Word if !is_keyword(t.text(line)) => Some(t.text(line).to_string()),
        TokenKind::QuotedIdent => Some(unquote(t.text(line))),
        _ => None,
    }
}

/// Relations named after `FROM`, `JOIN`, `UPDATE`, `INTO` and `TABLE`.
fn references(line: &str, toks: &[&Token]) -> Vec<Reference> {
    let mut out = Vec::new();
    let mut i = 0;
    while i < toks.len() {
        let t = toks[i];
        i += 1;
        let introduces = t.kind == TokenKind::Word
            && matches!(
                t.text(line).to_ascii_uppercase().as_str(),
                "FROM" | "JOIN" | "UPDATE" | "INTO" | "TABLE"
            );
        if !introduces {
            continue;
        }
        while let Some(mut name) = toks.get(i).and_then(|t| ident(line, t)) {
            i += 1;
            // `schema.table` names the table.
            while toks.get(i).is_some_and(|t| t.text(line) == ".")
                && let Some(part) = toks.get(i + 1).and_then(|t| ident(line, t))
            {
                name = part;
                i += 2;
            }
            let mut alias = None;
            if toks
                .get(i)
                .is_some_and(|t| t.text(line).eq_ignore_ascii_case("AS"))
            {
                i += 1;
                if let Some(a) = toks.get(i).and_then(|t| ident(line, t)) {
                    alias = Some(a);
                    i += 1;
                }
            } else if let Some(a) = toks.get(i).and_then(|t| ident(line, t)) {
                alias = Some(a);
                i += 1;
            }
            out.push(Reference { name, alias });
            if toks.get(i).is_some_and(|t| t.text(line) == ",") {
                i += 1;
            } else {
                break;
            }
        }
    }
    out
}

/// Analyzes `line` with the cursor at byte offset `pos`.
pub fn analyze(line: &str, pos: usize) -> Context {
    let mut start = fragment_start(line, pos);
    let mut ctx = Context {
        start,
        prefix: line[start..pos].to_string(),
        expect: Vec::new(),
        qualifier: None,
        references: Vec::new(),
    };

    // `.cmd` or `\cmd` at the start of input or after whitespace.
    let head = &line[..start];
    if let Some(sigil) = head.chars().next_back().filter(|c| matches!(c, '.' | '\\'))
        && head[..head.len() - 1]
            .chars()
            .next_back()
            .is_none_or(char::is_whitespace)
    {
        start -= 1;
        ctx.start = start;
        ctx.prefix = line[start..pos].to_string();
        debug_assert!(ctx.prefix.starts_with(sigil));
        ctx.expect = vec![Expect::Commands];
        return ctx;
    }

    let tokens = tokenize(line);
    if in_literal(&tokens, pos) {
        return ctx;
    }

    // `name.fragment`
    if let Some(before) = head.strip_suffix('.') {
        let q = fragment_start(before, before.len());
        if q < before.len() {
            ctx.qualifier = Some(before[q..].to_string());
        }
    }

    // Tokens of the statement under the cursor, without the fragment itself.
    let stmt_start = tokens
        .iter()
        .filter(|t| is_semicolon(line, t) && t.end <= pos)
        .map(|t| t.end)
        .next_back()
        .unwrap_or(0);
    let stmt_end = tokens
        .iter()
        .find(|t| is_semicolon(line, t) && t.start >= pos)
        .map_or(line.len(), |t| t.start);
    let sig: Vec<&Token> = tokens
        .iter()
        .filter(|t| {
            t.start >= stmt_start
                && t.end <= stmt_end
                && !matches!(
                    t.kind,
                    TokenKind::Whitespace | TokenKind::LineComment | TokenKind::BlockComment
                )
                && !(t.start == start && !ctx.prefix.is_empty())
        })
        .collect();
    ctx.references = references(line, &sig);

    if ctx.qualifier.is_some() {
        ctx.expect = vec![Expect::Columns];
        return ctx;
    }

    let mut clause = Clause::None;
    let mut prev = String::new();
    let mut in_column_list = false;
    for t in sig.iter().filter(|t| t.end <= start) {
        let text = t.text(line);
        if t.kind == TokenKind::Word
            && let Some(c) = clause_for(&text.to_ascii_uppercase())
        {
            clause = c;
            in_column_list = false;
        }
        match text {
            "(" if clause == Clause::Into => in_column_list = true,
            ")" => in_column_list = false,
            _ => {}
        }
        prev = text.to_ascii_uppercase();
    }

    let after_introducer = matches!(prev.as_str(), "FROM" | "JOIN" | "UPDATE" | "TABLE" | ",");
    ctx.expect = match clause {
        Clause::Columns => vec![Expect::Columns, Expect::Keywords],
        Clause::Relations if after_introducer => vec![Expect::Relations],
        Clause::Into if prev == "INTO" => vec![Expect::Relations],
        Clause::Into if in_column_list && matches!(prev.as_str(), "(" | ",") => {
            vec![Expect::Columns]
        }
        Clause::None | Clause::Relations | Clause::Into | Clause::Other => vec![Expect::Keywords],
    };
    ctx
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Splits `text` at the `|` cursor marker.
    fn at(text: &str) -> (String, usize) {
        let pos = text.find('|').expect("cursor marker");
        (text.replacen('|', "", 1), pos)
    }

    fn ctx(text: &str) -> Context {
        let (line, pos) = at(text);
        analyze(&line, pos)
    }

    #[test]
    fn tables_after_from_and_join() {
        assert_eq!(ctx("select * from c|").expect, vec![Expect::Relations]);
        assert_eq!(
            ctx("select * from a join c|").expect,
            vec![Expect::Relations]
        );
        assert_eq!(ctx("select * from a, c|").expect, vec![Expect::Relations]);
        assert_eq!(ctx("update c|").expect, vec![Expect::Relations]);
        assert_eq!(ctx("insert into c|").expect, vec![Expect::Relations]);
    }

    #[test]
    fn columns_in_column_clauses() {
        let both = vec![Expect::Columns, Expect::Keywords];
        for sql in [
            "select t|",
            "select * from a where t|",
            "select * from a join b on t|",
            "select * from a order by t|",
            "select * from a group by t|",
            "update a set t|",
            "select * from a where x = 1 and t|",
        ] {
            assert_eq!(ctx(sql).expect, both, "{sql}");
        }
    }

    #[test]
    fn keywords_otherwise() {
        assert_eq!(ctx("sel|").expect, vec![Expect::Keywords]);
        assert_eq!(
            ctx("select * from orders wh|").expect,
            vec![Expect::Keywords]
        );
        assert_eq!(ctx("select 1; sel|").expect, vec![Expect::Keywords]);
        assert_eq!(
            ctx("insert into t values (1) wh|").expect,
            vec![Expect::Keywords]
        );
    }

    #[test]
    fn column_list_after_insert_into() {
        assert_eq!(ctx("insert into t (a, b|").expect, vec![Expect::Columns]);
        assert_eq!(ctx("insert into t (|").expect, vec![Expect::Columns]);
        assert_eq!(ctx("insert into t (a) va|").expect, vec![Expect::Keywords]);
    }

    #[test]
    fn statement_under_cursor_only() {
        // The first statement's FROM must not leak into the second.
        let c = ctx("select * from a; select t| from b");
        assert_eq!(c.references.len(), 1);
        assert_eq!(c.references[0].name, "b");
    }

    #[test]
    fn alias_forms() {
        let c = ctx("select | from orders o join customers as c on 1 join items");
        let got: Vec<_> = c
            .references
            .iter()
            .map(|r| (r.name.as_str(), r.alias.as_deref()))
            .collect();
        assert_eq!(
            got,
            vec![
                ("orders", Some("o")),
                ("customers", Some("c")),
                ("items", None)
            ]
        );
    }

    #[test]
    fn keyword_after_table_is_not_an_alias() {
        let c = ctx("select | from orders where 1");
        assert_eq!(c.references[0].alias, None);
        let c = ctx("select | from orders left join t on 1");
        assert_eq!(c.references[0].alias, None);
    }

    #[test]
    fn comma_list_and_schema_prefix() {
        let c = ctx("select | from a x, main.b y, c");
        let got: Vec<_> = c
            .references
            .iter()
            .map(|r| (r.name.as_str(), r.alias.as_deref()))
            .collect();
        assert_eq!(got, vec![("a", Some("x")), ("b", Some("y")), ("c", None)]);
    }

    #[test]
    fn quoted_names() {
        let c = ctx("select | from \"my table\" as [t x]");
        assert_eq!(c.references[0].name, "my table");
        assert_eq!(c.references[0].alias.as_deref(), Some("t x"));
    }

    #[test]
    fn relations_after_cursor_are_seen() {
        let c = ctx("select | from orders");
        assert_eq!(c.references[0].name, "orders");
        assert_eq!(c.expect, vec![Expect::Columns, Expect::Keywords]);
    }

    #[test]
    fn partial_word_is_not_a_reference() {
        let c = ctx("select * from ord|");
        assert!(c.references.is_empty());
    }

    #[test]
    fn qualifier_and_span() {
        let c = ctx("select o.| from orders o");
        assert_eq!(c.qualifier.as_deref(), Some("o"));
        assert_eq!(c.prefix, "");
        assert_eq!(c.expect, vec![Expect::Columns]);

        let (line, pos) = at("select o.to| from orders o");
        let c = analyze(&line, pos);
        assert_eq!(c.qualifier.as_deref(), Some("o"));
        assert_eq!(c.prefix, "to");
        assert_eq!(c.start, 9);
    }

    #[test]
    fn dot_without_qualifier_is_not_one() {
        assert_eq!(ctx("select 1 + ).|").qualifier, None);
    }

    #[test]
    fn special_commands() {
        let c = ctx(".tab|");
        assert_eq!((c.start, c.prefix.as_str()), (0, ".tab"));
        assert_eq!(c.expect, vec![Expect::Commands]);
        assert_eq!(ctx("\\d|").expect, vec![Expect::Commands]);
        assert_eq!(ctx("  .t|").start, 2);
    }

    #[test]
    fn nothing_inside_strings_and_comments() {
        for sql in [
            "select 'abc|",
            "select 'a|b'",
            "select 1 -- com|",
            "select /* com| */ 1",
            "select /* com|",
            "select \"ab|",
        ] {
            assert!(ctx(sql).expect.is_empty(), "{sql}");
        }
    }

    #[test]
    fn closed_string_is_not_inside() {
        assert!(!ctx("select 'abc'| ").expect.is_empty());
    }

    #[test]
    fn empty_input_expects_keywords() {
        assert_eq!(ctx("|").expect, vec![Expect::Keywords]);
    }
}

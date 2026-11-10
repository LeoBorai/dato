//! Keyword, special-command and schema completion.

use std::sync::{Arc, RwLock};

use reedline::{Completer, CompletionResult, Span, Suggestion};

use super::keywords::KEYWORDS;
use crate::special::symbolic_names;
use crate::tui::session::SchemaCache;

/// Start of the word ending at `pos`. A leading `.` or `\` belongs to the word
/// when it begins the input or follows whitespace.
fn word_start(line: &str, pos: usize) -> usize {
    let head = &line[..pos];
    let mut start = head
        .char_indices()
        .rev()
        .take_while(|(_, c)| c.is_alphanumeric() || *c == '_')
        .last()
        .map_or(pos, |(i, _)| i);
    if let Some(prev) = head[..start].chars().next_back()
        && (prev == '.' || prev == '\\')
        && head[..start - 1]
            .chars()
            .next_back()
            .is_none_or(char::is_whitespace)
    {
        start -= 1;
    }
    start
}

/// Candidate strings for the word before `pos`, with the word's start offset.
pub fn candidates(cache: &SchemaCache, line: &str, pos: usize) -> (usize, Vec<String>) {
    let start = word_start(line, pos);
    let prefix = &line[start..pos];
    if prefix.is_empty() {
        return (start, Vec::new());
    }
    let lower = prefix.to_lowercase();
    let mut out: Vec<String> = if prefix.starts_with(['.', '\\']) {
        let mut v: Vec<String> = symbolic_names()
            .filter(|n| n.starts_with(prefix))
            .map(String::from)
            .collect();
        v.sort();
        v
    } else {
        // Schema names first (they are what users usually mean), then keywords.
        let mut names: Vec<String> = cache
            .tables
            .iter()
            .chain(&cache.columns)
            .filter(|n| n.to_lowercase().starts_with(&lower))
            .cloned()
            .collect();
        names.sort();
        names.dedup();
        names.extend(
            KEYWORDS
                .iter()
                .filter(|k| k.to_lowercase().starts_with(&lower))
                .map(|k| k.to_string()),
        );
        names
    };
    out.dedup();
    // Completing a word to itself adds nothing.
    out.retain(|c| c.to_lowercase() != lower);
    (start, out)
}

pub struct SqlCompleter {
    schema: Arc<RwLock<SchemaCache>>,
}

impl SqlCompleter {
    pub fn new(schema: Arc<RwLock<SchemaCache>>) -> Self {
        Self { schema }
    }
}

impl Completer for SqlCompleter {
    fn complete(&mut self, line: &str, pos: usize) -> CompletionResult {
        let cache = self.schema.read().unwrap();
        let (start, names) = candidates(&cache, line, pos);
        let suggestions: Vec<Suggestion> = names
            .into_iter()
            .map(|value| Suggestion {
                value,
                span: Span::new(start, pos),
                ..Default::default()
            })
            .collect();
        CompletionResult::fresh(suggestions)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cache() -> SchemaCache {
        SchemaCache {
            tables: vec!["customers".into(), "orders".into()],
            columns: vec!["id".into(), "total".into()],
        }
    }

    #[test]
    fn completes_keyword_ignoring_case() {
        let (start, c) = candidates(&cache(), "sel", 3);
        assert_eq!(start, 0);
        assert_eq!(c, vec!["SELECT"]);
    }

    #[test]
    fn completes_table_name() {
        let line = "select * from cus";
        let (start, c) = candidates(&cache(), line, line.len());
        assert_eq!(start, 14);
        assert_eq!(c, vec!["customers"]);
    }

    #[test]
    fn completes_columns() {
        let (_, c) = candidates(&cache(), "select to", 9);
        assert_eq!(c, vec!["total"]);
    }

    #[test]
    fn completes_special_commands() {
        let (start, c) = candidates(&cache(), ".tab", 4);
        assert_eq!(start, 0);
        assert_eq!(c, vec![".tables"]);
        let (_, c) = candidates(&cache(), "\\d", 2);
        assert!(c.contains(&"\\dt".to_string()) && c.contains(&"\\di".to_string()));
    }

    #[test]
    fn nothing_for_empty_prefix() {
        assert!(candidates(&cache(), "select ", 7).1.is_empty());
    }

    #[test]
    fn dot_inside_identifier_is_not_a_command() {
        let (start, c) = candidates(&cache(), "select o.tot", 12);
        assert_eq!(start, 9);
        assert_eq!(c, vec!["total"]);
    }

    #[test]
    fn suggestion_span_covers_prefix() {
        let schema = Arc::new(RwLock::new(cache()));
        let mut comp = SqlCompleter::new(schema);
        let r = comp.complete("select * from ord", 17);
        let s = r.suggestions();
        assert_eq!(s[0].value, "orders");
        assert_eq!((s[0].span.start, s[0].span.end), (14, 17));
    }
}

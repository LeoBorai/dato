//! SQL dialects: how SQL text is read for each [`Server`].
//!
//! The shared layer (this module) defines [`LexRules`] and the [`Dialect`]
//! trait; each server has its own submodule with its rules and keyword list.

pub mod mysql;
pub mod sqlite;

use crate::server::Server;

/// Lexical differences between servers, consumed by `sql::tokenize_with`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LexRules {
    /// `"..."` is a string literal (MySQL) rather than a quoted identifier.
    pub double_quote_is_string: bool,
    /// `[...]` is a quoted identifier (SQLite).
    pub bracket_idents: bool,
    /// A backslash escapes the next character inside strings (MySQL).
    pub backslash_escapes: bool,
    /// `#` starts a comment to end of line (MySQL).
    pub hash_comments: bool,
    /// `--` starts a comment only when followed by whitespace or end of input (MySQL).
    pub dash_comment_needs_space: bool,
    /// `CREATE TRIGGER ... BEGIN ... END` bodies are not split at inner `;` (SQLite).
    pub trigger_bodies: bool,
}

/// The dialect layer of [`Server`].
pub trait Dialect {
    fn lex_rules(&self) -> LexRules;
    /// Upper-case keywords, sorted.
    fn keywords(&self) -> &'static [&'static str];
    /// Whether a statement can change the visible schema, so the completion
    /// cache must be refreshed after it.
    fn changes_schema(&self, stmt: &str) -> bool;
    /// Whether `name.` may refer to another database (MySQL `db.table`).
    fn qualifies_databases(&self) -> bool;

    fn is_keyword(&self, word: &str) -> bool {
        self.keywords()
            .binary_search(&word.to_ascii_uppercase().as_str())
            .is_ok()
    }
}

/// Lower-cased first word of a statement.
fn first_word(stmt: &str) -> String {
    stmt.split(|c: char| !c.is_alphanumeric())
        .find(|w| !w.is_empty())
        .unwrap_or("")
        .to_ascii_lowercase()
}

impl Dialect for Server {
    fn lex_rules(&self) -> LexRules {
        match self {
            Server::Sqlite => sqlite::RULES,
            Server::MySql => mysql::RULES,
        }
    }

    fn keywords(&self) -> &'static [&'static str] {
        match self {
            Server::Sqlite => sqlite::KEYWORDS,
            Server::MySql => mysql::KEYWORDS,
        }
    }

    fn changes_schema(&self, stmt: &str) -> bool {
        let words = match self {
            Server::Sqlite => sqlite::SCHEMA_CHANGES,
            Server::MySql => mysql::SCHEMA_CHANGES,
        };
        words.contains(&first_word(stmt).as_str())
    }

    fn qualifies_databases(&self) -> bool {
        match self {
            Server::Sqlite => false,
            Server::MySql => true,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn server_specific_keywords() {
        assert!(Server::MySql.is_keyword("show"));
        assert!(!Server::Sqlite.is_keyword("show"));
        assert!(Server::Sqlite.is_keyword("pragma"));
        assert!(!Server::MySql.is_keyword("PRAGMA"));
        for s in Server::ALL {
            assert!(s.is_keyword("select") && s.is_keyword("WHERE"));
            assert!(!s.is_keyword("users"));
        }
    }

    #[test]
    fn schema_changes_per_server() {
        for stmt in ["rename table a to b", "truncate t", "use crm"] {
            assert!(Server::MySql.changes_schema(stmt), "{stmt}");
            assert!(!Server::Sqlite.changes_schema(stmt), "{stmt}");
        }
        for stmt in ["pragma foreign_keys=on", "attach 'x.db' as x"] {
            assert!(Server::Sqlite.changes_schema(stmt), "{stmt}");
            assert!(!Server::MySql.changes_schema(stmt), "{stmt}");
        }
        for s in Server::ALL {
            assert!(s.changes_schema("  CREATE table t(a)"));
            assert!(!s.changes_schema("select 1"));
        }
    }
}

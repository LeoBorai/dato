//! SQL dialects: how SQL text is read for each [`Server`].
//!
//! The shared layer (this module) defines [`LexRules`] and the [`Dialect`]
//! trait; each server has its own submodule with its rules and keyword list.

pub mod mysql;
pub mod psql;
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
    /// `$$...$$` and `$tag$...$tag$` are string literals (PostgreSQL).
    pub dollar_quotes: bool,
    /// `/* */` comments nest (PostgreSQL).
    pub nested_block_comments: bool,
    /// `E'...'` is a string with backslash escapes (PostgreSQL).
    pub escape_strings: bool,
}

/// What `name.` in front of a table can refer to besides a table or alias.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Qualifier {
    /// Nothing: names are not qualified (SQLite).
    None,
    /// Another database on the server (MySQL `db.table`).
    Database,
    /// A schema of the current database (PostgreSQL `schema.table`).
    Schema,
}

/// The dialect layer of [`Server`].
pub trait Dialect {
    fn lex_rules(&self) -> LexRules;
    /// Upper-case keywords, sorted.
    fn keywords(&self) -> &'static [&'static str];
    /// Whether a statement can change the visible schema, so the completion
    /// cache must be refreshed after it.
    fn changes_schema(&self, stmt: &str) -> bool;
    /// What `name.` may refer to besides a table or alias.
    fn qualifier(&self) -> Qualifier;

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
            Server::PostgreSql => psql::RULES,
        }
    }

    fn keywords(&self) -> &'static [&'static str] {
        match self {
            Server::Sqlite => sqlite::KEYWORDS,
            Server::MySql => mysql::KEYWORDS,
            Server::PostgreSql => psql::KEYWORDS,
        }
    }

    fn changes_schema(&self, stmt: &str) -> bool {
        let words = match self {
            Server::Sqlite => sqlite::SCHEMA_CHANGES,
            Server::MySql => mysql::SCHEMA_CHANGES,
            Server::PostgreSql => psql::SCHEMA_CHANGES,
        };
        words.contains(&first_word(stmt).as_str())
    }

    fn qualifier(&self) -> Qualifier {
        match self {
            Server::Sqlite => Qualifier::None,
            Server::MySql => Qualifier::Database,
            Server::PostgreSql => Qualifier::Schema,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn postgres_keywords_are_its_own() {
        assert!(Server::PostgreSql.is_keyword("returning"));
        assert!(!Server::PostgreSql.is_keyword("pragma"));
        assert!(!Server::PostgreSql.is_keyword("show_foo"));
        assert!(!Server::MySql.is_keyword("returning"));
    }

    #[test]
    fn postgres_schema_changes() {
        for stmt in [
            "set search_path to sales",
            "alter table a rename to b",
            "comment on table a is 'x'",
        ] {
            assert!(Server::PostgreSql.changes_schema(stmt), "{stmt}");
        }
        assert!(!Server::PostgreSql.changes_schema("select 1"));
        assert!(!Server::PostgreSql.changes_schema("use crm"));
        assert!(!Server::MySql.changes_schema("set search_path to x"));
    }

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
    fn qualifier_per_server() {
        assert_eq!(Server::Sqlite.qualifier(), Qualifier::None);
        assert_eq!(Server::MySql.qualifier(), Qualifier::Database);
        assert_eq!(Server::PostgreSql.qualifier(), Qualifier::Schema);
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

//! Database clients.
//!
//! The shared layer (this module and `value`) defines the sync, object-safe
//! [`Client`] trait, the result types and [`ConnectOptions`]. Each server has its
//! own submodule (`sqlite`, `mysql`), and [`Connector`] maps a [`Server`] to the
//! submodule that opens it.

pub mod mysql;
pub mod sqlite;
pub mod value;

use std::sync::Arc;

use crate::server::Server;

pub use value::{ColumnInfo, Relation, RelationKind, ResultSet, StatementResult, Value};

#[derive(Debug, thiserror::Error)]
pub enum ClientError {
    #[error("{message}")]
    Sql {
        message: String,
        partial: Vec<StatementResult>,
    },
    #[error("interrupted")]
    Interrupted { partial: Vec<StatementResult> },
    #[error("{0}")]
    Unsupported(String),
    #[error("{0}")]
    Open(String),
}

impl ClientError {
    /// Results of the statements that succeeded before the failure.
    pub fn partial(&self) -> &[StatementResult] {
        match self {
            ClientError::Sql { partial, .. } | ClientError::Interrupted { partial } => partial,
            _ => &[],
        }
    }
}

/// Cancels the statement a client is currently running. Callable from any thread.
pub trait Interrupt: Send + Sync {
    fn interrupt(&self);
}

pub trait Client: Send {
    /// The server this client talks to.
    fn server(&self) -> Server;
    /// Runs every statement in `sql` in order, stopping at the first error.
    fn execute(&mut self, sql: &str) -> Result<Vec<StatementResult>, ClientError>;
    fn tables(&mut self, prefix: Option<&str>) -> Result<Vec<String>, ClientError>;
    fn views(&mut self, prefix: Option<&str>) -> Result<Vec<String>, ClientError>;
    /// Columns: `name`, `sql`.
    fn indexes(&mut self, table: Option<&str>) -> Result<ResultSet, ClientError>;
    /// `CREATE` statements, optionally for one table only.
    fn schema(&mut self, table: Option<&str>) -> Result<Vec<String>, ClientError>;
    fn databases(&mut self) -> Result<ResultSet, ClientError>;
    fn columns(&mut self, table: &str) -> Result<Vec<ColumnInfo>, ClientError>;
    fn interrupt_handle(&self) -> Arc<dyn Interrupt>;
    /// Identifies the connection in the prompt. Read on every prompt, so it may
    /// change during the session (e.g. after MySQL `USE`).
    fn prompt_label(&self) -> String;

    /// Every table and view of the current database with its columns, sorted by
    /// name. Servers that can read this in one round trip override it.
    /// Databases on the server, for `db.` and `USE` completion. Empty for servers
    /// without named databases to switch between.
    fn database_names(&mut self) -> Result<Vec<String>, ClientError> {
        Ok(Vec::new())
    }

    /// `(database, table)` pairs across the server's user databases, for `db.`
    /// completion. Empty for servers without named databases.
    fn qualified_tables(&mut self) -> Result<Vec<(String, String)>, ClientError> {
        Ok(Vec::new())
    }

    fn relations(&mut self) -> Result<Vec<Relation>, ClientError> {
        // `tables()` lists views too; `views()` tells which are views.
        let names = self.tables(None)?;
        let views = self.views(None)?;
        Ok(names
            .into_iter()
            .map(|name| {
                let kind = if views.contains(&name) {
                    RelationKind::View
                } else {
                    RelationKind::Table
                };
                let columns = self.columns(&name).unwrap_or_default();
                Relation {
                    name,
                    kind,
                    columns,
                }
            })
            .collect())
    }
}

/// MySQL login parameters, from `dato mysql`, `\connect mysql` or the wizard.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MySqlOptions {
    pub host: String,
    pub port: u16,
    pub user: String,
    pub password: Option<String>,
    /// `-p` was given: ask for the password before connecting.
    pub password_required: bool,
    pub socket: Option<String>,
    pub database: Option<String>,
}

impl Default for MySqlOptions {
    fn default() -> Self {
        Self {
            host: "localhost".into(),
            port: 3306,
            user: default_user(),
            password: None,
            password_required: false,
            socket: None,
            database: None,
        }
    }
}

/// The current OS user, as the `mysql` client uses by default.
pub fn default_user() -> String {
    std::env::var("USER")
        .or_else(|_| std::env::var("USERNAME"))
        .unwrap_or_else(|_| "root".into())
}

/// Where and how to connect. One variant per [`Server`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ConnectOptions {
    /// `None` is an in-memory database.
    Sqlite {
        path: Option<String>,
    },
    MySql(MySqlOptions),
}

impl ConnectOptions {
    pub fn server(&self) -> Server {
        match self {
            ConnectOptions::Sqlite { .. } => Server::Sqlite,
            ConnectOptions::MySql(_) => Server::MySql,
        }
    }

    /// SQLite options for a CLI path argument (`:memory:` or empty is in-memory).
    pub fn sqlite(path: Option<&str>) -> Self {
        let path = match path {
            None | Some("") | Some(":memory:") => None,
            Some(p) => Some(p.to_string()),
        };
        ConnectOptions::Sqlite { path }
    }
}

/// The connection layer of [`Server`]: opens a client from options.
pub trait Connector {
    fn open(&self, opts: &ConnectOptions) -> Result<Box<dyn Client>, ClientError>;
}

impl Connector for Server {
    fn open(&self, opts: &ConnectOptions) -> Result<Box<dyn Client>, ClientError> {
        match (self, opts) {
            (Server::Sqlite, ConnectOptions::Sqlite { path }) => {
                Ok(Box::new(sqlite::SqliteClient::open(path.as_deref())?))
            }
            (Server::MySql, ConnectOptions::MySql(o)) => mysql::open(o),
            (s, o) => Err(ClientError::Open(format!(
                "{} options cannot open a {s} connection",
                o.server()
            ))),
        }
    }
}

/// Opens a client for `opts` with the server the options belong to.
pub fn open(opts: &ConnectOptions) -> Result<Box<dyn Client>, ClientError> {
    opts.server().open(opts)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sqlite_options_from_path() {
        assert_eq!(
            ConnectOptions::sqlite(None),
            ConnectOptions::Sqlite { path: None }
        );
        assert_eq!(
            ConnectOptions::sqlite(Some(":memory:")),
            ConnectOptions::Sqlite { path: None }
        );
        assert_eq!(
            ConnectOptions::sqlite(Some("app.db")),
            ConnectOptions::Sqlite {
                path: Some("app.db".into())
            }
        );
    }

    #[test]
    fn sqlite_opens_through_server() {
        let mut c = Server::Sqlite.open(&ConnectOptions::sqlite(None)).unwrap();
        assert_eq!(c.execute("select 1").unwrap().len(), 1);
    }

    #[test]
    fn mismatched_options_are_rejected() {
        let err = Server::MySql
            .open(&ConnectOptions::sqlite(None))
            .err()
            .unwrap();
        assert!(err.to_string().contains("cannot open a mysql connection"));
    }

    #[test]
    fn mysql_defaults() {
        let o = MySqlOptions::default();
        assert_eq!((o.host.as_str(), o.port), ("localhost", 3306));
    }
}

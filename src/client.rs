//! Database clients.
//!
//! The shared layer (this module and `value`) defines the sync, object-safe
//! [`Client`] trait, the result types and [`ConnectOptions`]. Each server has its
//! own submodule (`sqlite`, `mysql`), and [`Connector`] maps a [`Server`] to the
//! submodule that opens it.

pub mod mysql;
pub mod psql;
pub mod sqlite;
pub mod value;

use std::sync::Arc;

use crate::server::Server;

pub use mysql::MySqlOptions;
pub use psql::PsqlOptions;
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

    /// `(database or schema, table)` pairs across the server's user databases or
    /// schemas, for `qualifier.` completion. Empty for servers without qualifiers.
    fn qualified_tables(&mut self) -> Result<Vec<(String, String)>, ClientError> {
        Ok(Vec::new())
    }

    /// Relations of every user schema, each with `schema` set, for
    /// `schema.table.` completion. Empty for servers that do not qualify names
    /// by schema.
    fn schema_relations(&mut self) -> Result<Vec<Relation>, ClientError> {
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
                    schema: None,
                    name,
                    kind,
                    columns,
                }
            })
            .collect())
    }
}

/// Where and how to connect. One variant per [`Server`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ConnectOptions {
    /// `None` is an in-memory database.
    Sqlite {
        path: Option<String>,
    },
    MySql(MySqlOptions),
    Psql(PsqlOptions),
}

impl ConnectOptions {
    pub fn server(&self) -> Server {
        match self {
            ConnectOptions::Sqlite { .. } => Server::Sqlite,
            ConnectOptions::MySql(_) => Server::MySql,
            ConnectOptions::Psql(_) => Server::PostgreSql,
        }
    }

    /// The prompt to ask for a password with, when these options ask for one
    /// that is not set yet.
    pub fn needs_password(&self) -> Option<&'static str> {
        match self {
            ConnectOptions::Sqlite { .. } => None,
            ConnectOptions::MySql(o) => {
                (o.password_required && o.password.is_none()).then_some("Enter password: ")
            }
            ConnectOptions::Psql(o) => {
                (o.password_required && o.password.is_none()).then_some("Password: ")
            }
        }
    }

    /// Stores the password asked for after [`Self::needs_password`].
    pub fn set_password(&mut self, password: String) {
        match self {
            ConnectOptions::Sqlite { .. } => {}
            ConnectOptions::MySql(o) => o.password = Some(password),
            ConnectOptions::Psql(o) => o.password = Some(password),
        }
    }

    /// SQLite options for a CLI path argument (`:memory:` or empty is in-memory).
    pub fn sqlite(path: Option<&str>) -> Self {
        let path = match path {
            None | Some("") | Some(":memory:") => None,
            Some(p) => Some(crate::paths::expand_tilde(p).to_string_lossy().into_owned()),
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
            (Server::PostgreSql, ConnectOptions::Psql(o)) => psql::open(o),
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
    fn sqlite_path_expands_tilde() {
        let Some(home) = dirs::home_dir() else { return };
        assert_eq!(
            ConnectOptions::sqlite(Some("~/app.db")),
            ConnectOptions::Sqlite {
                path: Some(home.join("app.db").to_string_lossy().into_owned())
            }
        );
    }

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
    fn password_prompt_per_server() {
        let mut o = ConnectOptions::MySql(MySqlOptions {
            password_required: true,
            ..Default::default()
        });
        assert_eq!(o.needs_password(), Some("Enter password: "));
        o.set_password("x".into());
        assert_eq!(o.needs_password(), None);
        assert_eq!(ConnectOptions::sqlite(None).needs_password(), None);
    }

    #[test]
    fn mysql_defaults() {
        let o = MySqlOptions::default();
        assert_eq!((o.host.as_str(), o.port), ("localhost", 3306));
    }
}

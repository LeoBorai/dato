//! Database clients. Each backend implements the sync, object-safe [`Client`] trait.

mod mysql;
mod postgres;
pub mod sqlite;
pub mod value;

use std::sync::Arc;

pub use value::{ColumnInfo, ResultSet, StatementResult, Value};

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
    fn display_name(&self) -> String;
}

/// Where to connect, parsed from the CLI argument.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Target {
    /// `None` is an in-memory database.
    Sqlite(Option<String>),
    Postgres(String),
    Mysql(String),
}

impl Target {
    pub fn parse(arg: Option<&str>) -> Target {
        match arg {
            None | Some("") | Some(":memory:") => Target::Sqlite(None),
            Some(s) if s.starts_with("postgres://") || s.starts_with("postgresql://") => {
                Target::Postgres(s.to_string())
            }
            Some(s) if s.starts_with("mysql://") => Target::Mysql(s.to_string()),
            Some(s) => Target::Sqlite(Some(s.to_string())),
        }
    }
}

pub fn open(target: &Target) -> Result<Box<dyn Client>, ClientError> {
    match target {
        Target::Sqlite(path) => open_sqlite(path.as_deref()),
        Target::Postgres(url) => postgres::open(url),
        Target::Mysql(url) => mysql::open(url),
    }
}

fn open_sqlite(path: Option<&str>) -> Result<Box<dyn Client>, ClientError> {
    Ok(Box::new(sqlite::SqliteClient::open(path)?))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn target_parsing() {
        assert_eq!(Target::parse(None), Target::Sqlite(None));
        assert_eq!(Target::parse(Some(":memory:")), Target::Sqlite(None));
        assert_eq!(
            Target::parse(Some("app.db")),
            Target::Sqlite(Some("app.db".into()))
        );
        assert!(matches!(
            Target::parse(Some("postgres://h/db")),
            Target::Postgres(_)
        ));
        assert!(matches!(
            Target::parse(Some("postgresql://h/db")),
            Target::Postgres(_)
        ));
        assert!(matches!(
            Target::parse(Some("mysql://h/db")),
            Target::Mysql(_)
        ));
    }

    #[test]
    fn server_backends_are_unavailable() {
        let err = open(&Target::Postgres("postgres://h/db".into()))
            .err()
            .unwrap();
        assert!(
            err.to_string()
                .contains("Postgres backend is not available")
        );
        let err = open(&Target::Mysql("mysql://h/db".into())).err().unwrap();
        assert!(err.to_string().contains("MySQL backend is not available"));
    }
}

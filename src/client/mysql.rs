//! MySQL client built on the sync `mysql` crate.

mod options;
mod value;

pub use options::MySqlOptions;

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use mysql::prelude::Queryable;
use mysql::{Conn, Opts, OptsBuilder, Params, Row};

use super::{
    Client, ClientError, ColumnInfo, Interrupt, Relation, RelationKind, ResultSet, StatementResult,
    Value,
};
use crate::dialect;
use crate::server::Server;
use crate::sql::split_statements_with;

/// `ER_QUERY_INTERRUPTED`: the statement was stopped by `KILL QUERY`.
const QUERY_INTERRUPTED: u16 = 1317;

/// Schemas that hold server metadata, left out of `db.` completion.
const SYSTEM_SCHEMAS: &str = "'information_schema','mysql','performance_schema','sys'";

pub struct MySqlClient {
    conn: Conn,
    opts: Opts,
    user: String,
    host: String,
    database: Option<String>,
    /// Set by [`MySqlInterrupt`]. `KILL QUERY` does not always fail the statement
    /// (an interrupted `SLEEP()` just returns 1), so the client checks this too.
    interrupted: Arc<AtomicBool>,
}

/// Stops the running statement from a second connection.
struct MySqlInterrupt {
    opts: Opts,
    id: u32,
    interrupted: Arc<AtomicBool>,
}

impl Interrupt for MySqlInterrupt {
    fn interrupt(&self) {
        self.interrupted.store(true, Ordering::SeqCst);
        // A failed kill is ignored: the statement then runs to completion.
        if let Ok(mut c) = Conn::new(self.opts.clone()) {
            let _ = c.query_drop(format!("KILL QUERY {}", self.id));
        }
    }
}

fn convert_err(err: mysql::Error, partial: Vec<StatementResult>) -> ClientError {
    match err {
        mysql::Error::MySqlError(e) if e.code == QUERY_INTERRUPTED => {
            ClientError::Interrupted { partial }
        }
        e => ClientError::Sql {
            message: e.to_string(),
            partial,
        },
    }
}

fn err(e: mysql::Error) -> ClientError {
    convert_err(e, Vec::new())
}

/// Converts a row using its own column metadata.
fn row_values(row: Row) -> Vec<Value> {
    let kinds: Vec<_> = row
        .columns_ref()
        .iter()
        .map(|c| value::kind_of(c.column_type(), c.character_set()))
        .collect();
    row.unwrap()
        .into_iter()
        .zip(kinds)
        .map(|(v, k)| value::convert(k, v))
        .collect()
}

fn text(v: &Value) -> String {
    match v {
        Value::Text(t) | Value::Numeric(t) => t.clone(),
        Value::Integer(i) => i.to_string(),
        Value::Blob(b) => String::from_utf8_lossy(b).into_owned(),
        _ => String::new(),
    }
}

/// Escapes `%`, `_` and `\` for `LIKE` (MySQL's default escape is `\`).
fn like_prefix(prefix: &str) -> String {
    let mut out = String::with_capacity(prefix.len() + 1);
    for c in prefix.chars() {
        if matches!(c, '%' | '_' | '\\') {
            out.push('\\');
        }
        out.push(c);
    }
    out.push('%');
    out
}

fn quote_ident(name: &str) -> String {
    format!("`{}`", name.replace('`', "``"))
}

fn builder(o: &MySqlOptions) -> OptsBuilder {
    let b = OptsBuilder::new()
        .ip_or_hostname(Some(o.host.clone()))
        .tcp_port(o.port)
        .user(Some(o.user.clone()))
        .pass(o.password.clone())
        .db_name(o.database.clone())
        // Metadata queries use `DATABASE()`, which a cached prepared statement
        // keeps resolving to the database it was first prepared under, so they
        // would go stale after `USE`.
        .stmt_cache_size(0)
        .tcp_connect_timeout(Some(Duration::from_secs(10)));
    match &o.socket {
        Some(s) => b.socket(Some(s.clone())).prefer_socket(true),
        None => b,
    }
}

pub fn open(o: &MySqlOptions) -> Result<Box<dyn Client>, ClientError> {
    Ok(Box::new(MySqlClient::open(o)?))
}

impl MySqlClient {
    pub fn open(o: &MySqlOptions) -> Result<Self, ClientError> {
        let opts = Opts::from(builder(o));
        let conn = Conn::new(opts.clone()).map_err(|e| {
            let at = o
                .socket
                .clone()
                .unwrap_or_else(|| format!("{}:{}", o.host, o.port));
            ClientError::Open(format!("cannot connect to {}@{at}: {e}", o.user))
        })?;
        let mut c = Self {
            conn,
            opts,
            user: o.user.clone(),
            host: o.host.clone(),
            database: None,
            interrupted: Arc::default(),
        };
        c.sync_database();
        Ok(c)
    }

    /// Re-reads the current database (after connecting or `USE`).
    fn sync_database(&mut self) {
        self.database = self
            .conn
            .query_first::<Option<String>, _>("SELECT DATABASE()")
            .ok()
            .flatten()
            .flatten();
    }

    fn run_one(&mut self, sql: &str) -> Result<Vec<StatementResult>, mysql::Error> {
        let mut out = Vec::new();
        let mut result = self.conn.query_iter(sql)?;
        while let Some(set) = result.iter() {
            let columns: Vec<String> = set
                .columns()
                .as_ref()
                .iter()
                .map(|c| c.name_str().into_owned())
                .collect();
            if columns.is_empty() {
                out.push(StatementResult::Affected(set.affected_rows()));
                continue;
            }
            let mut rows = Vec::new();
            for row in set {
                rows.push(row_values(row?));
            }
            out.push(StatementResult::Rows(ResultSet { columns, rows }));
        }
        Ok(out)
    }

    fn query_set(&mut self, sql: &str, params: Params) -> Result<ResultSet, ClientError> {
        let mut result = self.conn.exec_iter(sql, params).map_err(err)?;
        let mut rs = ResultSet::default();
        if let Some(set) = result.iter() {
            rs.columns = set
                .columns()
                .as_ref()
                .iter()
                .map(|c| c.name_str().into_owned())
                .collect();
            for row in set {
                rs.rows.push(row_values(row.map_err(err)?));
            }
        }
        Ok(rs)
    }

    fn names(&mut self, sql: &str, params: Params) -> Result<Vec<String>, ClientError> {
        Ok(self
            .query_set(sql, params)?
            .rows
            .iter()
            .filter_map(|r| r.first().map(text))
            .collect())
    }
}

impl Client for MySqlClient {
    fn server(&self) -> Server {
        Server::MySql
    }

    fn execute(&mut self, sql: &str) -> Result<Vec<StatementResult>, ClientError> {
        let mut results = Vec::new();
        for stmt in split_statements_with(sql, &dialect::mysql::RULES) {
            self.interrupted.store(false, Ordering::SeqCst);
            let outcome = self.run_one(stmt);
            if stmt
                .split_whitespace()
                .next()
                .is_some_and(|w| w.eq_ignore_ascii_case("use"))
            {
                self.sync_database();
            }
            if self.interrupted.swap(false, Ordering::SeqCst) {
                return Err(ClientError::Interrupted { partial: results });
            }
            match outcome {
                Ok(r) => results.extend(r),
                Err(e) => return Err(convert_err(e, results)),
            }
        }
        Ok(results)
    }

    fn tables(&mut self, prefix: Option<&str>) -> Result<Vec<String>, ClientError> {
        self.names(
            "SELECT TABLE_NAME FROM information_schema.TABLES \
             WHERE TABLE_SCHEMA = DATABASE() AND TABLE_NAME LIKE ? ORDER BY 1",
            Params::Positional(vec![like_prefix(prefix.unwrap_or("")).into()]),
        )
    }

    fn views(&mut self, prefix: Option<&str>) -> Result<Vec<String>, ClientError> {
        self.names(
            "SELECT TABLE_NAME FROM information_schema.TABLES \
             WHERE TABLE_SCHEMA = DATABASE() AND TABLE_TYPE = 'VIEW' AND TABLE_NAME LIKE ? \
             ORDER BY 1",
            Params::Positional(vec![like_prefix(prefix.unwrap_or("")).into()]),
        )
    }

    fn indexes(&mut self, table: Option<&str>) -> Result<ResultSet, ClientError> {
        self.query_set(
            "SELECT INDEX_NAME AS name, \
               CONCAT(IF(NON_UNIQUE = 0, 'UNIQUE ', ''), 'INDEX ', INDEX_NAME, ' ON ', \
                      TABLE_NAME, ' (', \
                      GROUP_CONCAT(COLUMN_NAME ORDER BY SEQ_IN_INDEX SEPARATOR ', '), ')') AS `sql` \
             FROM information_schema.STATISTICS \
             WHERE TABLE_SCHEMA = DATABASE() AND (? IS NULL OR TABLE_NAME = ?) \
             GROUP BY TABLE_NAME, INDEX_NAME, NON_UNIQUE ORDER BY TABLE_NAME, INDEX_NAME",
            Params::Positional(vec![table.into(), table.into()]),
        )
    }

    fn schema(&mut self, table: Option<&str>) -> Result<Vec<String>, ClientError> {
        let names = match table {
            Some(t) => vec![t.to_string()],
            None => self.tables(None)?,
        };
        let mut out = Vec::new();
        for name in names {
            let rs = self.query_set(
                &format!("SHOW CREATE TABLE {}", quote_ident(&name)),
                Params::Empty,
            )?;
            if let Some(stmt) = rs.rows.first().and_then(|r| r.get(1)) {
                out.push(text(stmt));
            }
        }
        Ok(out)
    }

    fn databases(&mut self) -> Result<ResultSet, ClientError> {
        self.query_set(
            "SELECT SCHEMA_NAME AS name FROM information_schema.SCHEMATA ORDER BY 1",
            Params::Empty,
        )
    }

    fn columns(&mut self, table: &str) -> Result<Vec<ColumnInfo>, ClientError> {
        let rs = self.query_set(
            "SELECT COLUMN_NAME, COLUMN_TYPE, IS_NULLABLE, COLUMN_DEFAULT, COLUMN_KEY \
             FROM information_schema.COLUMNS \
             WHERE TABLE_SCHEMA = DATABASE() AND TABLE_NAME = ? ORDER BY ORDINAL_POSITION",
            Params::Positional(vec![table.into()]),
        )?;
        Ok(rs.rows.iter().map(|r| column_info(&r[0..5])).collect())
    }

    /// All tables and views of the current database with their columns, in one query.
    fn relations(&mut self) -> Result<Vec<Relation>, ClientError> {
        let rs = self.query_set(
            "SELECT t.TABLE_NAME, t.TABLE_TYPE, \
               c.COLUMN_NAME, c.COLUMN_TYPE, c.IS_NULLABLE, c.COLUMN_DEFAULT, c.COLUMN_KEY \
             FROM information_schema.TABLES t \
             LEFT JOIN information_schema.COLUMNS c \
               ON c.TABLE_SCHEMA = t.TABLE_SCHEMA AND c.TABLE_NAME = t.TABLE_NAME \
             WHERE t.TABLE_SCHEMA = DATABASE() \
             ORDER BY t.TABLE_NAME, c.ORDINAL_POSITION",
            Params::Empty,
        )?;
        let mut out: Vec<Relation> = Vec::new();
        for r in &rs.rows {
            let name = text(&r[0]);
            if out.last().is_none_or(|l| l.name != name) {
                let kind = if text(&r[1]) == "VIEW" {
                    RelationKind::View
                } else {
                    RelationKind::Table
                };
                out.push(Relation {
                    schema: None,
                    name,
                    kind,
                    columns: Vec::new(),
                });
            }
            if r[2] != Value::Null {
                out.last_mut().unwrap().columns.push(column_info(&r[2..7]));
            }
        }
        Ok(out)
    }

    fn database_names(&mut self) -> Result<Vec<String>, ClientError> {
        self.names(
            "SELECT SCHEMA_NAME FROM information_schema.SCHEMATA ORDER BY 1",
            Params::Empty,
        )
    }

    fn qualified_tables(&mut self) -> Result<Vec<(String, String)>, ClientError> {
        let rs = self.query_set(
            &format!(
                "SELECT TABLE_SCHEMA, TABLE_NAME FROM information_schema.TABLES \
                 WHERE TABLE_SCHEMA NOT IN ({SYSTEM_SCHEMAS}) ORDER BY 1, 2"
            ),
            Params::Empty,
        )?;
        Ok(rs.rows.iter().map(|r| (text(&r[0]), text(&r[1]))).collect())
    }

    fn interrupt_handle(&self) -> Arc<dyn Interrupt> {
        Arc::new(MySqlInterrupt {
            opts: self.opts.clone(),
            id: self.conn.connection_id(),
            interrupted: Arc::clone(&self.interrupted),
        })
    }

    fn database(&self) -> Option<String> {
        self.database.clone()
    }

    fn prompt_label(&self) -> String {
        match &self.database {
            Some(db) => format!("{}@{}/{db}", self.user, self.host),
            None => format!("{}@{}", self.user, self.host),
        }
    }
}

/// `COLUMN_NAME, COLUMN_TYPE, IS_NULLABLE, COLUMN_DEFAULT, COLUMN_KEY`.
fn column_info(r: &[Value]) -> ColumnInfo {
    ColumnInfo {
        name: text(&r[0]),
        type_name: text(&r[1]),
        not_null: text(&r[2]) == "NO",
        default: match &r[3] {
            Value::Null => None,
            v => Some(text(v)),
        },
        primary_key: text(&r[4]) == "PRI",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn identifiers_and_like_patterns() {
        assert_eq!(quote_ident("a`b"), "`a``b`");
        assert_eq!(like_prefix("us_r%"), "us\\_r\\%%");
    }
}

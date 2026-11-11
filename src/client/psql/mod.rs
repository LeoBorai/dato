//! PostgreSQL client built on the sync `postgres` crate.
//!
//! Statements run through the simple query protocol, so every value arrives as
//! the server's own text (what `psql` prints). A `prepare` before each statement
//! tells the column types, so numbers still align and keep exact digits.

mod options;
mod tls;
mod value;

use std::sync::Arc;
use std::time::Duration;

use postgres::config::SslMode as DriverSslMode;
use postgres::error::SqlState;
use postgres::{CancelToken, Config, NoTls, SimpleQueryMessage};
use tokio_postgres_rustls::MakeRustlsConnect;

use super::{
    Client, ClientError, ColumnInfo, Interrupt, Relation, RelationKind, ResultSet, StatementResult,
    Value,
};
use crate::dialect;
use crate::server::Server;
use crate::sql::split_statements_with;

pub use options::{PsqlOptions, SslMode};

/// Where relations come from: schemas that hold server metadata are never listed.
const USER_SCHEMAS: &str = "n.nspname NOT IN ('pg_catalog', 'information_schema') \
     AND n.nspname NOT LIKE 'pg_toast%' AND n.nspname NOT LIKE 'pg_temp%'";

/// Tables, partitioned tables, views, materialized views and foreign tables.
const RELKINDS: &str = "c.relkind IN ('r', 'p', 'v', 'm', 'f')";

/// How this connection talks TLS, kept for the cancel request.
#[derive(Clone)]
enum Tls {
    None,
    Rustls(MakeRustlsConnect),
}

pub struct PsqlClient {
    client: postgres::Client,
    tls: Tls,
    user: String,
    host: String,
    database: String,
}

/// Cancels the running statement through the protocol's cancel request.
struct PsqlInterrupt {
    token: CancelToken,
    tls: Tls,
}

impl Interrupt for PsqlInterrupt {
    fn interrupt(&self) {
        // A failed cancel is ignored: the statement then runs to completion.
        let _ = match &self.tls {
            Tls::None => self.token.cancel_query(NoTls),
            Tls::Rustls(t) => self.token.cancel_query(t.clone()),
        };
    }
}

fn message(e: &postgres::Error) -> String {
    match e.as_db_error() {
        Some(db) => format!("{}: {}", db.severity(), db.message()),
        // The driver's own text omits the cause (a bad certificate, a refused
        // connection), so walk the chain.
        None => {
            let mut text = e.to_string();
            let mut source = std::error::Error::source(e);
            while let Some(s) = source {
                text.push_str(": ");
                text.push_str(&s.to_string());
                source = s.source();
            }
            text
        }
    }
}

fn convert_err(e: postgres::Error, partial: Vec<StatementResult>) -> ClientError {
    if e.code() == Some(&SqlState::QUERY_CANCELED) {
        return ClientError::Interrupted { partial };
    }
    ClientError::Sql {
        message: message(&e),
        partial,
    }
}

fn err(e: postgres::Error) -> ClientError {
    convert_err(e, Vec::new())
}

/// `name`, quoted only when it is not a plain lower-case identifier.
fn quote_ident(name: &str) -> String {
    let plain = name
        .chars()
        .next()
        .is_some_and(|c| c.is_ascii_lowercase() || c == '_')
        && name
            .chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_');
    if plain && !dialect::psql::KEYWORDS.contains(&name.to_ascii_uppercase().as_str()) {
        name.to_string()
    } else {
        format!("\"{}\"", name.replace('"', "\"\""))
    }
}

/// Escapes `%`, `_` and `\` for `LIKE` (the default escape is `\`).
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

fn config(o: &PsqlOptions) -> Config {
    let mut c = Config::new();
    // `host_path` (unix socket directory) only exists on unix targets.
    #[cfg(unix)]
    if o.host.starts_with('/') {
        c.host_path(&o.host);
    } else {
        c.host(&o.host);
    }
    #[cfg(not(unix))]
    c.host(&o.host);
    c.port(o.port)
        .user(&o.user)
        .dbname(o.dbname.as_deref().unwrap_or(&o.user))
        .application_name("dato")
        .connect_timeout(Duration::from_secs(10))
        .ssl_mode(match o.sslmode {
            SslMode::Disable => DriverSslMode::Disable,
            SslMode::Prefer => DriverSslMode::Prefer,
            SslMode::Require | SslMode::VerifyFull => DriverSslMode::Require,
        });
    if let Some(p) = &o.password {
        c.password(p);
    }
    c
}

pub fn open(o: &PsqlOptions) -> Result<Box<dyn Client>, ClientError> {
    Ok(Box::new(PsqlClient::open(o)?))
}

impl PsqlClient {
    pub fn open(o: &PsqlOptions) -> Result<Self, ClientError> {
        let cfg = config(o);
        let connector = tls::connector(o)?;
        let connected = match &connector {
            None => cfg.connect(NoTls),
            Some(t) => cfg.connect(t.clone()),
        };
        let mut client = connected.map_err(|e| {
            let at = if o.host.starts_with('/') {
                o.host.clone()
            } else {
                format!("{}:{}", o.host, o.port)
            };
            ClientError::Open(format!(
                "cannot connect to {}@{at}: {}",
                o.user,
                message(&e)
            ))
        })?;
        let database = client
            .query_one("SELECT current_database()::text", &[])
            .map(|r| r.get::<_, String>(0))
            .unwrap_or_else(|_| o.dbname.clone().unwrap_or_else(|| o.user.clone()));
        Ok(Self {
            client,
            tls: connector.map_or(Tls::None, Tls::Rustls),
            user: o.user.clone(),
            host: o.host.clone(),
            database,
        })
    }

    /// Runs one statement: its column types first, then its rows as text.
    fn run_one(&mut self, stmt: &str) -> Result<StatementResult, postgres::Error> {
        let prepared = self.client.prepare(stmt)?;
        let kinds: Vec<value::Kind> = prepared
            .columns()
            .iter()
            .map(|c| value::kind_of(c.type_()))
            .collect();
        let columns: Vec<String> = prepared
            .columns()
            .iter()
            .map(|c| c.name().to_string())
            .collect();
        drop(prepared);

        let mut rows = Vec::new();
        let mut affected = 0;
        for msg in self.client.simple_query(stmt)? {
            match msg {
                SimpleQueryMessage::Row(r) => rows.push(
                    kinds
                        .iter()
                        .enumerate()
                        .map(|(i, k)| value::convert(*k, r.get(i)))
                        .collect(),
                ),
                SimpleQueryMessage::CommandComplete(n) => affected = n,
                _ => {}
            }
        }
        Ok(if columns.is_empty() {
            StatementResult::Affected(affected)
        } else {
            StatementResult::Rows(ResultSet { columns, rows })
        })
    }

    /// Runs a metadata query whose columns are all text and returns its rows.
    fn text_rows(
        &mut self,
        sql: &str,
        params: &[&(dyn postgres::types::ToSql + Sync)],
    ) -> Result<Vec<Vec<Option<String>>>, ClientError> {
        let rows = self.client.query(sql, params).map_err(err)?;
        Ok(rows
            .iter()
            .map(|r| {
                (0..r.len())
                    .map(|i| r.get::<_, Option<String>>(i))
                    .collect()
            })
            .collect())
    }

    fn names(
        &mut self,
        sql: &str,
        params: &[&(dyn postgres::types::ToSql + Sync)],
    ) -> Result<Vec<String>, ClientError> {
        Ok(self
            .text_rows(sql, params)?
            .into_iter()
            .filter_map(|mut r| r.swap_remove(0))
            .collect())
    }

    /// Resolves `name` or `schema.name` to a relation on the search path (or in
    /// the given schema): `(oid, schema, name, relkind)`.
    fn find_relation(
        &mut self,
        name: &str,
    ) -> Result<(String, String, String, String), ClientError> {
        let (schema, rel) = match name.split_once('.') {
            Some((s, r)) => (Some(s.trim_matches('"')), r.trim_matches('"')),
            None => (None, name.trim_matches('"')),
        };
        let rows = self.text_rows(
            &format!(
                "SELECT c.oid::text, n.nspname::text, c.relname::text, c.relkind::text \
                 FROM pg_class c JOIN pg_namespace n ON n.oid = c.relnamespace \
                 WHERE {RELKINDS} AND c.relname = $1 \
                   AND (($2::text IS NULL AND n.nspname = ANY (current_schemas(false))) \
                        OR n.nspname = $2) \
                 ORDER BY array_position(current_schemas(false), n.nspname::name) NULLS LAST \
                 LIMIT 1"
            ),
            &[&rel, &schema],
        )?;
        match rows.into_iter().next() {
            Some(r) => {
                let [Some(oid), Some(ns), Some(rel), Some(kind)] =
                    <[Option<String>; 4]>::try_from(r)
                        .map_err(|_| ClientError::Unsupported("unexpected catalog row".into()))?
                else {
                    return Err(ClientError::Unsupported("unexpected catalog row".into()));
                };
                Ok((oid, ns, rel, kind))
            }
            None => Err(ClientError::Sql {
                message: format!("ERROR: relation \"{name}\" does not exist"),
                partial: Vec::new(),
            }),
        }
    }

    /// Relations with their columns. `all` covers every user schema and sets each
    /// relation's schema; otherwise only the search path.
    fn load_relations(&mut self, all: bool) -> Result<Vec<Relation>, ClientError> {
        let scope = if all {
            USER_SCHEMAS.to_string()
        } else {
            "n.nspname = ANY (current_schemas(false))".to_string()
        };
        let rows = self.text_rows(
            &format!(
                "SELECT c.oid::text, n.nspname::text, c.relname::text, c.relkind::text, \
                   a.attname::text, format_type(a.atttypid, a.atttypmod), \
                   CASE WHEN a.attnotnull THEN 't' ELSE 'f' END, \
                   pg_get_expr(d.adbin, d.adrelid), \
                   CASE WHEN EXISTS (SELECT 1 FROM pg_index i WHERE i.indrelid = c.oid \
                        AND i.indisprimary AND a.attnum = ANY (i.indkey)) THEN 't' ELSE 'f' END \
                 FROM pg_class c JOIN pg_namespace n ON n.oid = c.relnamespace \
                 LEFT JOIN pg_attribute a ON a.attrelid = c.oid AND a.attnum > 0 \
                   AND NOT a.attisdropped \
                 LEFT JOIN pg_attrdef d ON d.adrelid = a.attrelid AND d.adnum = a.attnum \
                 WHERE {RELKINDS} AND {scope} \
                 ORDER BY n.nspname, c.relname, c.oid, a.attnum"
            ),
            &[],
        )?;
        let mut out: Vec<Relation> = Vec::new();
        let mut last_oid = String::new();
        for r in rows {
            let oid = r[0].clone().unwrap_or_default();
            if oid != last_oid {
                last_oid = oid;
                out.push(Relation {
                    schema: all.then(|| r[1].clone().unwrap_or_default()),
                    name: r[2].clone().unwrap_or_default(),
                    kind: relation_kind(r[3].as_deref()),
                    columns: Vec::new(),
                });
            }
            if r[4].is_some() {
                out.last_mut().unwrap().columns.push(column_info(&r[4..9]));
            }
        }
        if !all {
            out.sort_by(|a, b| a.name.cmp(&b.name));
        }
        Ok(out)
    }
}

fn relation_kind(relkind: Option<&str>) -> RelationKind {
    match relkind {
        Some("v") | Some("m") => RelationKind::View,
        _ => RelationKind::Table,
    }
}

/// `attname, format_type, not null ('t'/'f'), default, primary key ('t'/'f')`.
fn column_info(r: &[Option<String>]) -> ColumnInfo {
    ColumnInfo {
        name: r[0].clone().unwrap_or_default(),
        type_name: r[1].clone().unwrap_or_default(),
        not_null: r[2].as_deref() == Some("t"),
        default: r[3].clone(),
        primary_key: r[4].as_deref() == Some("t"),
    }
}

impl Client for PsqlClient {
    fn server(&self) -> Server {
        Server::PostgreSql
    }

    fn execute(&mut self, sql: &str) -> Result<Vec<StatementResult>, ClientError> {
        let mut results = Vec::new();
        for stmt in split_statements_with(sql, &dialect::psql::RULES) {
            match self.run_one(stmt) {
                Ok(r) => results.push(r),
                Err(e) => return Err(convert_err(e, results)),
            }
        }
        Ok(results)
    }

    fn tables(&mut self, prefix: Option<&str>) -> Result<Vec<String>, ClientError> {
        self.names(
            &format!(
                "SELECT DISTINCT c.relname::text FROM pg_class c \
                 JOIN pg_namespace n ON n.oid = c.relnamespace \
                 WHERE {RELKINDS} AND n.nspname = ANY (current_schemas(false)) \
                   AND c.relname LIKE $1 ORDER BY 1"
            ),
            &[&like_prefix(prefix.unwrap_or(""))],
        )
    }

    fn views(&mut self, prefix: Option<&str>) -> Result<Vec<String>, ClientError> {
        self.names(
            "SELECT DISTINCT c.relname::text FROM pg_class c \
             JOIN pg_namespace n ON n.oid = c.relnamespace \
             WHERE c.relkind IN ('v', 'm') AND n.nspname = ANY (current_schemas(false)) \
               AND c.relname LIKE $1 ORDER BY 1",
            &[&like_prefix(prefix.unwrap_or(""))],
        )
    }

    fn indexes(&mut self, table: Option<&str>) -> Result<ResultSet, ClientError> {
        let rows = self.text_rows(
            "SELECT indexname::text, indexdef FROM pg_indexes \
             WHERE schemaname = ANY (current_schemas(false)) \
               AND ($1::text IS NULL OR tablename = $1) \
             ORDER BY tablename, indexname",
            &[&table],
        )?;
        let mut rs = ResultSet::new(&["name", "sql"]);
        rs.rows = rows
            .into_iter()
            .map(|r| {
                r.into_iter()
                    .map(|v| v.map_or(Value::Null, Value::Text))
                    .collect()
            })
            .collect();
        Ok(rs)
    }

    fn schema(&mut self, table: Option<&str>) -> Result<Vec<String>, ClientError> {
        let names = match table {
            Some(t) => vec![t.to_string()],
            None => self.tables(None)?,
        };
        let mut out = Vec::new();
        for name in names {
            let (oid, _schema, rel, kind) = self.find_relation(&name)?;
            if matches!(kind.as_str(), "v" | "m") {
                let def = self.text_rows("SELECT pg_get_viewdef($1::text::oid, true)", &[&oid])?;
                let body = def
                    .into_iter()
                    .next()
                    .and_then(|mut r| r.swap_remove(0))
                    .unwrap_or_default();
                let what = if kind == "m" {
                    "MATERIALIZED VIEW"
                } else {
                    "VIEW"
                };
                out.push(format!(
                    "CREATE {what} {} AS\n{}",
                    quote_ident(&rel),
                    body.trim_end().trim_end_matches(';')
                ));
                continue;
            }
            let cols = self.text_rows(
                "SELECT a.attname::text, format_type(a.atttypid, a.atttypmod), \
                   CASE WHEN a.attnotnull THEN 't' ELSE 'f' END, \
                   pg_get_expr(d.adbin, d.adrelid), a.attidentity::text, a.attgenerated::text \
                 FROM pg_attribute a \
                 LEFT JOIN pg_attrdef d ON d.adrelid = a.attrelid AND d.adnum = a.attnum \
                 WHERE a.attrelid = $1::text::oid AND a.attnum > 0 AND NOT a.attisdropped \
                 ORDER BY a.attnum",
                &[&oid],
            )?;
            let cons = self.text_rows(
                "SELECT conname::text, pg_get_constraintdef(oid, true) FROM pg_constraint \
                 WHERE conrelid = $1::text::oid ORDER BY (contype = 'p') DESC, conname",
                &[&oid],
            )?;
            let mut lines: Vec<String> = cols
                .into_iter()
                .map(|r| {
                    let mut line = format!(
                        "  {} {}",
                        quote_ident(r[0].as_deref().unwrap_or("")),
                        r[1].as_deref().unwrap_or("")
                    );
                    let default = r[3].as_deref().unwrap_or("");
                    match (r[4].as_deref(), r[5].as_deref()) {
                        (Some("a"), _) => line.push_str(" GENERATED ALWAYS AS IDENTITY"),
                        (Some("d"), _) => line.push_str(" GENERATED BY DEFAULT AS IDENTITY"),
                        (_, Some("s")) => {
                            line.push_str(&format!(" GENERATED ALWAYS AS ({default}) STORED"))
                        }
                        _ if !default.is_empty() => line.push_str(&format!(" DEFAULT {default}")),
                        _ => {}
                    }
                    if r[2].as_deref() == Some("t") {
                        line.push_str(" NOT NULL");
                    }
                    line
                })
                .collect();
            lines.extend(cons.into_iter().map(|r| {
                format!(
                    "  CONSTRAINT {} {}",
                    quote_ident(r[0].as_deref().unwrap_or("")),
                    r[1].as_deref().unwrap_or("")
                )
            }));
            out.push(format!(
                "CREATE TABLE {} (\n{}\n)",
                quote_ident(&rel),
                lines.join(",\n")
            ));
        }
        Ok(out)
    }

    fn databases(&mut self) -> Result<ResultSet, ClientError> {
        let names = self.names(
            "SELECT datname::text AS name FROM pg_database WHERE NOT datistemplate ORDER BY 1",
            &[],
        )?;
        let mut rs = ResultSet::new(&["name"]);
        rs.rows = names.into_iter().map(|n| vec![Value::Text(n)]).collect();
        Ok(rs)
    }

    fn columns(&mut self, table: &str) -> Result<Vec<ColumnInfo>, ClientError> {
        let (oid, ..) = self.find_relation(table)?;
        let rows = self.text_rows(
            "SELECT a.attname::text, format_type(a.atttypid, a.atttypmod), \
               CASE WHEN a.attnotnull THEN 't' ELSE 'f' END, \
               pg_get_expr(d.adbin, d.adrelid), \
               CASE WHEN EXISTS (SELECT 1 FROM pg_index i WHERE i.indrelid = a.attrelid \
                    AND i.indisprimary AND a.attnum = ANY (i.indkey)) THEN 't' ELSE 'f' END \
             FROM pg_attribute a \
             LEFT JOIN pg_attrdef d ON d.adrelid = a.attrelid AND d.adnum = a.attnum \
             WHERE a.attrelid = $1::text::oid AND a.attnum > 0 AND NOT a.attisdropped \
             ORDER BY a.attnum",
            &[&oid],
        )?;
        Ok(rows.iter().map(|r| column_info(r)).collect())
    }

    /// All tables and views on the search path with their columns, in one query.
    fn relations(&mut self) -> Result<Vec<Relation>, ClientError> {
        self.load_relations(false)
    }

    fn schema_relations(&mut self) -> Result<Vec<Relation>, ClientError> {
        self.load_relations(true)
    }

    fn qualified_tables(&mut self) -> Result<Vec<(String, String)>, ClientError> {
        let rows = self.text_rows(
            &format!(
                "SELECT n.nspname::text, c.relname::text FROM pg_class c \
                 JOIN pg_namespace n ON n.oid = c.relnamespace \
                 WHERE {RELKINDS} AND {USER_SCHEMAS} ORDER BY 1, 2"
            ),
            &[],
        )?;
        Ok(rows
            .into_iter()
            .map(|r| {
                (
                    r[0].clone().unwrap_or_default(),
                    r[1].clone().unwrap_or_default(),
                )
            })
            .collect())
    }

    fn interrupt_handle(&self) -> Arc<dyn Interrupt> {
        Arc::new(PsqlInterrupt {
            token: self.client.cancel_token(),
            tls: self.tls.clone(),
        })
    }

    fn prompt_label(&self) -> String {
        format!("{}@{}/{}", self.user, self.host, self.database)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn identifiers_and_like_patterns() {
        assert_eq!(quote_ident("orders"), "orders");
        assert_eq!(quote_ident("Order Items"), "\"Order Items\"");
        assert_eq!(quote_ident("a\"b"), "\"a\"\"b\"");
        assert_eq!(quote_ident("select"), "\"select\"");
        assert_eq!(quote_ident("1abc"), "\"1abc\"");
        assert_eq!(like_prefix("a_b%"), "a\\_b\\%%");
        assert_eq!(like_prefix(""), "%");
    }

    #[test]
    fn config_follows_options() {
        let o = PsqlOptions {
            host: "/var/run/postgresql".into(),
            user: "app".into(),
            password: Some("pw".into()),
            sslmode: SslMode::VerifyFull,
            ..Default::default()
        };
        let c = config(&o);
        assert_eq!(c.get_dbname(), Some("app"));
        assert_eq!(c.get_ssl_mode(), DriverSslMode::Require);
        assert!(matches!(c.get_hosts()[0], postgres::config::Host::Unix(_)));
        let tcp = config(&PsqlOptions {
            dbname: Some("shop".into()),
            sslmode: SslMode::Disable,
            ..Default::default()
        });
        assert_eq!(tcp.get_dbname(), Some("shop"));
        assert_eq!(tcp.get_ssl_mode(), DriverSslMode::Disable);
    }
}

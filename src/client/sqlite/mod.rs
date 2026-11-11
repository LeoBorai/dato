//! SQLite client built on `rusqlite` (bundled SQLite).

use std::sync::Arc;

use rusqlite::{Connection, ErrorCode, InterruptHandle, types::ValueRef};

use super::{Client, ClientError, ColumnInfo, Interrupt, ResultSet, StatementResult, Value};
use crate::sql::split_statements;

pub struct SqliteClient {
    conn: Connection,
    name: String,
}

struct SqliteInterrupt(InterruptHandle);

impl Interrupt for SqliteInterrupt {
    fn interrupt(&self) {
        self.0.interrupt();
    }
}

fn convert(err: rusqlite::Error, partial: Vec<StatementResult>) -> ClientError {
    if err.sqlite_error_code() == Some(ErrorCode::OperationInterrupted) {
        ClientError::Interrupted { partial }
    } else {
        ClientError::Sql {
            message: err.to_string(),
            partial,
        }
    }
}

fn value_of(v: ValueRef<'_>) -> Value {
    match v {
        ValueRef::Null => Value::Null,
        ValueRef::Integer(i) => Value::Integer(i),
        ValueRef::Real(r) => Value::Real(r),
        ValueRef::Text(t) => Value::Text(String::from_utf8_lossy(t).into_owned()),
        ValueRef::Blob(b) => Value::Blob(b.to_vec()),
    }
}

/// Escapes `%`, `_` and `\` for use with `LIKE ... ESCAPE '\'`.
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

impl SqliteClient {
    /// `None` opens an in-memory database.
    pub fn open(path: Option<&str>) -> Result<Self, ClientError> {
        let (conn, name) = match path {
            None => (Connection::open_in_memory(), ":memory:".to_string()),
            Some(p) => (Connection::open(p), p.to_string()),
        };
        let conn = conn.map_err(|e| ClientError::Open(format!("cannot open {name}: {e}")))?;
        Ok(Self { conn, name })
    }

    pub fn open_memory() -> Self {
        Self::open(None).expect("in-memory database")
    }

    fn run_one(&self, sql: &str) -> Result<StatementResult, rusqlite::Error> {
        let mut stmt = self.conn.prepare(sql)?;
        let ncols = stmt.column_count();
        if ncols == 0 {
            let n = stmt.execute([])?;
            return Ok(StatementResult::Affected(n as u64));
        }
        let columns: Vec<String> = stmt.column_names().map(|c| c.to_string()).collect();
        let mut rows = stmt.query([])?;
        let mut out = Vec::new();
        while let Some(row) = rows.next()? {
            let mut vals = Vec::with_capacity(ncols);
            for i in 0..ncols {
                vals.push(value_of(row.get_ref(i)?));
            }
            out.push(vals);
        }
        Ok(StatementResult::Rows(ResultSet { columns, rows: out }))
    }

    fn query_set(
        &self,
        sql: &str,
        args: &[&dyn rusqlite::ToSql],
    ) -> Result<ResultSet, ClientError> {
        let run = || -> Result<ResultSet, rusqlite::Error> {
            let mut stmt = self.conn.prepare(sql)?;
            let columns: Vec<String> = stmt.column_names().map(|c| c.to_string()).collect();
            let n = columns.len();
            let mut rows = stmt.query(args)?;
            let mut out = Vec::new();
            while let Some(row) = rows.next()? {
                out.push(
                    (0..n)
                        .map(|i| row.get_ref(i).map(value_of))
                        .collect::<Result<_, _>>()?,
                );
            }
            Ok(ResultSet { columns, rows: out })
        };
        run().map_err(|e| convert(e, Vec::new()))
    }

    fn names(&self, sql: &str, args: &[&dyn rusqlite::ToSql]) -> Result<Vec<String>, ClientError> {
        Ok(self
            .query_set(sql, args)?
            .rows
            .into_iter()
            .filter_map(|mut r| match r.pop() {
                Some(Value::Text(t)) => Some(t),
                _ => None,
            })
            .collect())
    }
}

impl Client for SqliteClient {
    fn server(&self) -> crate::server::Server {
        crate::server::Server::Sqlite
    }

    fn execute(&mut self, sql: &str) -> Result<Vec<StatementResult>, ClientError> {
        let mut results = Vec::new();
        for stmt in split_statements(sql) {
            match self.run_one(stmt) {
                Ok(r) => results.push(r),
                Err(e) => return Err(convert(e, results)),
            }
        }
        Ok(results)
    }

    fn tables(&mut self, prefix: Option<&str>) -> Result<Vec<String>, ClientError> {
        self.names(
            "SELECT name FROM sqlite_master \
             WHERE type IN ('table','view') AND name LIKE ?1 ESCAPE '\\' \
             AND name NOT LIKE 'sqlite\\_%' ESCAPE '\\' AND name NOT LIKE 'sqlean\\_%' ESCAPE '\\' \
             ORDER BY 1",
            &[&like_prefix(prefix.unwrap_or(""))],
        )
    }

    fn views(&mut self, prefix: Option<&str>) -> Result<Vec<String>, ClientError> {
        self.names(
            "SELECT name FROM sqlite_master \
             WHERE type = 'view' AND name LIKE ?1 ESCAPE '\\' \
             AND name NOT LIKE 'sqlite\\_%' ESCAPE '\\' \
             ORDER BY 1",
            &[&like_prefix(prefix.unwrap_or(""))],
        )
    }

    fn indexes(&mut self, table: Option<&str>) -> Result<ResultSet, ClientError> {
        self.query_set(
            "SELECT name, sql FROM sqlite_master \
             WHERE type = 'index' AND name NOT LIKE 'sqlite\\_%' ESCAPE '\\' \
             AND (?1 IS NULL OR tbl_name = ?1) ORDER BY 1",
            &[&table],
        )
    }

    fn schema(&mut self, table: Option<&str>) -> Result<Vec<String>, ClientError> {
        self.names(
            "SELECT sql FROM sqlite_master \
             WHERE sql IS NOT NULL AND name NOT LIKE 'sqlite\\_%' ESCAPE '\\' \
             AND (?1 IS NULL OR tbl_name = ?1) \
             ORDER BY tbl_name, type DESC, name",
            &[&table],
        )
    }

    fn databases(&mut self) -> Result<ResultSet, ClientError> {
        self.query_set("PRAGMA database_list", &[])
    }

    fn columns(&mut self, table: &str) -> Result<Vec<ColumnInfo>, ClientError> {
        let rs = self.query_set(
            "SELECT name, type, \"notnull\", dflt_value, pk FROM pragma_table_info(?1)",
            &[&table],
        )?;
        Ok(rs
            .rows
            .into_iter()
            .map(|r| {
                let text = |v: &Value| match v {
                    Value::Text(t) => t.clone(),
                    _ => String::new(),
                };
                let int = |v: &Value| matches!(v, Value::Integer(i) if *i != 0);
                ColumnInfo {
                    name: text(&r[0]),
                    type_name: text(&r[1]),
                    not_null: int(&r[2]),
                    default: match &r[3] {
                        Value::Text(t) => Some(t.clone()),
                        _ => None,
                    },
                    primary_key: int(&r[4]),
                }
            })
            .collect())
    }

    fn interrupt_handle(&self) -> Arc<dyn Interrupt> {
        Arc::new(SqliteInterrupt(self.conn.get_interrupt_handle()))
    }

    fn prompt_label(&self) -> String {
        self.name.clone()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn client() -> SqliteClient {
        SqliteClient::open_memory()
    }

    #[test]
    fn two_statements_two_results() {
        let mut c = client();
        let r = c
            .execute("create table t(a); insert into t values (1),(2);")
            .unwrap();
        assert_eq!(r.len(), 2);
        assert_eq!(r[1], StatementResult::Affected(2));
    }

    #[test]
    fn mixed_value_types() {
        let mut c = client();
        let r = c.execute("select null, 1, 2.5, 'hi', x'0102';").unwrap();
        let StatementResult::Rows(rs) = &r[0] else {
            panic!("expected rows")
        };
        assert_eq!(
            rs.rows[0],
            vec![
                Value::Null,
                Value::Integer(1),
                Value::Real(2.5),
                Value::Text("hi".into()),
                Value::Blob(vec![1, 2]),
            ]
        );
    }

    #[test]
    fn zero_rows_keeps_columns() {
        let mut c = client();
        c.execute("create table t(a);").unwrap();
        let r = c.execute("select a from t where 0;").unwrap();
        let StatementResult::Rows(rs) = &r[0] else {
            panic!("expected rows")
        };
        assert_eq!(rs.columns, vec!["a"]);
        assert!(rs.rows.is_empty());
    }

    #[test]
    fn update_count() {
        let mut c = client();
        c.execute("create table t(a); insert into t values (1),(2),(3);")
            .unwrap();
        let r = c.execute("update t set a = 0;").unwrap();
        assert_eq!(r, vec![StatementResult::Affected(3)]);
    }

    #[test]
    fn error_halts_remaining_statements() {
        let mut c = client();
        let err = c
            .execute("select * from missing; create table later(a);")
            .unwrap_err();
        assert!(err.to_string().contains("no such table: missing"));
        assert!(c.tables(Some("later")).unwrap().is_empty());
    }

    #[test]
    fn error_keeps_partial_results() {
        let mut c = client();
        let err = c.execute("select 1; select * from missing;").unwrap_err();
        assert_eq!(err.partial().len(), 1);
    }

    #[test]
    fn interrupt_long_query_then_reuse() {
        let mut c = client();
        let h = c.interrupt_handle();
        let t = std::thread::spawn(move || {
            std::thread::sleep(std::time::Duration::from_millis(100));
            h.interrupt();
        });
        let err = c
            .execute(
                "with recursive c(x) as (select 1 union all select x+1 from c) \
                 select count(*) from c;",
            )
            .unwrap_err();
        t.join().unwrap();
        assert!(matches!(err, ClientError::Interrupted { .. }));
        assert!(c.execute("select 1;").is_ok());
    }

    #[test]
    fn tables_exclude_internal_objects() {
        let mut c = client();
        c.execute("create table users(id integer primary key autoincrement);")
            .unwrap();
        c.execute("insert into users default values;").unwrap();
        assert_eq!(c.tables(None).unwrap(), vec!["users"]);
        assert!(
            !c.schema(None)
                .unwrap()
                .iter()
                .any(|s| s.contains("sqlite_sequence"))
        );
    }

    #[test]
    fn prefix_filters_and_sorts() {
        let mut c = client();
        c.execute("create table users(a); create table user_roles(a); create table orders(a);")
            .unwrap();
        assert_eq!(c.tables(Some("user")).unwrap(), vec!["user_roles", "users"]);
    }

    #[test]
    fn views_only() {
        let mut c = client();
        c.execute("create table t(a); create view v as select a from t;")
            .unwrap();
        assert_eq!(c.views(None).unwrap(), vec!["v"]);
        assert_eq!(c.tables(None).unwrap(), vec!["t", "v"]);
    }

    #[test]
    fn columns_of_table() {
        let mut c = client();
        c.execute("create table p(id integer primary key, name text not null default 'x');")
            .unwrap();
        let cols = c.columns("p").unwrap();
        assert_eq!(cols.len(), 2);
        assert_eq!(cols[0].name, "id");
        assert_eq!(cols[0].type_name, "INTEGER");
        assert!(cols[0].primary_key);
        assert_eq!(cols[1].name, "name");
        assert!(cols[1].not_null);
        assert_eq!(cols[1].default.as_deref(), Some("'x'"));
    }

    #[test]
    fn indexes_and_schema_by_table() {
        let mut c = client();
        c.execute(
            "create table users(id, email); create index idx_users_email on users(email); \
             create table other(a); create index idx_other on other(a);",
        )
        .unwrap();
        let idx = c.indexes(Some("users")).unwrap();
        assert_eq!(idx.rows.len(), 1);
        assert_eq!(idx.rows[0][0], Value::Text("idx_users_email".into()));
        assert_eq!(c.schema(Some("users")).unwrap().len(), 2);
        assert_eq!(c.indexes(None).unwrap().rows.len(), 2);
    }

    #[test]
    fn databases_lists_main() {
        let mut c = client();
        let rs = c.databases().unwrap();
        assert_eq!(rs.rows[0][1], Value::Text("main".into()));
    }

    #[test]
    fn open_creates_file() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("new.db");
        let mut c = SqliteClient::open(Some(p.to_str().unwrap())).unwrap();
        c.execute("create table t(a);").unwrap();
        assert!(p.exists());
    }
}

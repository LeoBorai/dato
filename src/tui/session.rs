//! A connected session: runs inputs (SQL or special commands) and produces outputs.
//! Shared by the interactive shell and by batch mode.

use std::sync::{Arc, RwLock};
use std::time::{Duration, Instant};

use super::interrupt;
use crate::client::{Client, ResultSet, StatementResult};
use crate::render::{Format, render_to_string};
use crate::{special, sql};

/// What the editor completes from.
#[derive(Debug, Default, Clone)]
pub struct SchemaCache {
    pub tables: Vec<String>,
    pub columns: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Output {
    /// Rendered result or informational text, for stdout.
    Text(String),
    /// Row-count/timing line, for stdout. Only produced when `show_status` is set.
    Status(String),
    /// Error message, for stderr.
    Error(String),
    /// The user asked to leave the shell.
    Quit,
}

pub struct Session {
    pub(crate) client: Box<dyn Client>,
    pub(crate) format: Format,
    show_status: bool,
    schema: Arc<RwLock<SchemaCache>>,
}

pub fn status_line(result: &StatementResult, elapsed: Duration) -> String {
    let secs = elapsed.as_secs_f64();
    let plural = |n: u64| if n == 1 { "row" } else { "rows" };
    match result {
        StatementResult::Rows(rs) => {
            let n = rs.rows.len() as u64;
            format!("{n} {} in set, {secs:.3}s", plural(n))
        }
        StatementResult::Affected(n) => {
            format!("Query OK, {n} {} affected, {secs:.3}s", plural(*n))
        }
    }
}

fn changes_schema(stmt: &str) -> bool {
    let first = stmt
        .split(|c: char| !c.is_alphanumeric())
        .find(|w| !w.is_empty())
        .unwrap_or("")
        .to_ascii_lowercase();
    matches!(
        first.as_str(),
        "create" | "drop" | "alter" | "attach" | "detach" | "rollback" | "pragma"
    )
}

impl Session {
    pub fn new(client: Box<dyn Client>, format: Format, show_status: bool) -> Self {
        let mut s = Self {
            client,
            format,
            show_status,
            schema: Arc::default(),
        };
        s.refresh_schema();
        s
    }

    pub fn schema(&self) -> Arc<RwLock<SchemaCache>> {
        Arc::clone(&self.schema)
    }

    pub fn format(&self) -> Format {
        self.format
    }

    pub fn display_name(&self) -> String {
        self.client.display_name()
    }

    pub fn render(&self, rs: &ResultSet) -> String {
        render_to_string(rs, self.format)
    }

    pub fn refresh_schema(&mut self) {
        let tables = self.client.tables(None).unwrap_or_default();
        let mut columns: Vec<String> = Vec::new();
        for t in &tables {
            if let Ok(cols) = self.client.columns(t) {
                columns.extend(cols.into_iter().map(|c| c.name));
            }
        }
        columns.sort();
        columns.dedup();
        *self.schema.write().unwrap() = SchemaCache { tables, columns };
    }

    /// Runs one complete input and returns what to print, in order.
    pub fn handle(&mut self, input: &str) -> Vec<Output> {
        let trimmed = input.trim();
        if trimmed.is_empty() {
            return Vec::new();
        }
        if special::is_special(trimmed) {
            return special::dispatch(self, trimmed);
        }
        let (body, vertical) = sql::split_vertical(input);
        let statements = sql::split_statements(body);
        let mut outputs = Vec::new();
        let last = statements.len().saturating_sub(1);
        for (i, stmt) in statements.iter().enumerate() {
            let started = Instant::now();
            let result = {
                let _armed = interrupt::arm(self.client.interrupt_handle());
                self.client.execute(stmt)
            };
            let elapsed = started.elapsed();
            match result {
                Ok(results) => {
                    for r in &results {
                        match r {
                            StatementResult::Rows(rs) => {
                                let fmt = if vertical && i == last {
                                    Format::Vertical
                                } else {
                                    self.format
                                };
                                outputs.push(Output::Text(render_to_string(rs, fmt)));
                            }
                            StatementResult::Affected(_) => {}
                        }
                        if self.show_status {
                            outputs.push(Output::Status(status_line(r, elapsed)));
                        }
                    }
                    if changes_schema(stmt) {
                        self.refresh_schema();
                    }
                }
                Err(e) => {
                    outputs.push(Output::Error(e.to_string()));
                    if changes_schema(stmt) {
                        self.refresh_schema();
                    }
                    break;
                }
            }
        }
        outputs
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::client::sqlite::SqliteClient;

    fn session(status: bool) -> Session {
        Session::new(Box::new(SqliteClient::open_memory()), Format::Ascii, status)
    }

    fn texts(out: &[Output]) -> String {
        out.iter()
            .map(|o| match o {
                Output::Text(t) | Output::Status(t) | Output::Error(t) => t.clone(),
                Output::Quit => "<quit>".into(),
            })
            .collect::<Vec<_>>()
            .join("\n")
    }

    #[test]
    fn recovers_after_error() {
        let mut s = session(false);
        let out = s.handle("select * from missing;");
        assert!(matches!(&out[0], Output::Error(e) if e.contains("no such table: missing")));
        let out = s.handle("select 1 as n;");
        assert!(texts(&out).contains("| n |"));
    }

    #[test]
    fn status_line_after_select() {
        let mut s = session(true);
        let out = s.handle("select 1 union select 2 union select 3;");
        assert!(texts(&out).contains("3 rows in set"));
        let out = s.handle("create table t(a); insert into t values (1);");
        assert!(texts(&out).contains("Query OK, 1 row affected"));
    }

    #[test]
    fn no_status_in_batch_mode() {
        let mut s = session(false);
        let out = s.handle("select 1;");
        assert!(out.iter().all(|o| !matches!(o, Output::Status(_))));
    }

    #[test]
    fn vertical_suffix_keeps_session_format() {
        let mut s = session(false);
        let out = s.handle("select 1 as a, 2 as b\\G");
        let t = texts(&out);
        assert!(t.contains("1. row") && t.contains("a | 1"), "{t}");
        assert_eq!(s.format(), Format::Ascii);
    }

    #[test]
    fn schema_cache_refreshes_after_ddl() {
        let mut s = session(false);
        let cache = s.schema();
        assert!(cache.read().unwrap().tables.is_empty());
        s.handle("create table orders(id, total);");
        let c = cache.read().unwrap();
        assert_eq!(c.tables, vec!["orders"]);
        assert_eq!(c.columns, vec!["id", "total"]);
    }

    #[test]
    fn status_line_formats() {
        let d = Duration::from_millis(1500);
        assert_eq!(
            status_line(&StatementResult::Affected(1), d),
            "Query OK, 1 row affected, 1.500s"
        );
        assert_eq!(
            status_line(&StatementResult::Rows(ResultSet::new(&["a"])), d),
            "0 rows in set, 1.500s"
        );
    }

    #[test]
    fn interrupt_slot_cancels_running_query() {
        let mut s = session(false);
        let t = std::thread::spawn(|| {
            std::thread::sleep(Duration::from_millis(150));
            interrupt::trigger();
        });
        let out = s.handle(
            "with recursive c(x) as (select 1 union all select x+1 from c) select count(*) from c;",
        );
        t.join().unwrap();
        assert!(matches!(&out[0], Output::Error(e) if e.contains("interrupted")));
        assert!(texts(&s.handle("select 1;")).contains("| 1 |"));
    }
}

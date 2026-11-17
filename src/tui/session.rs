//! A session: runs inputs (SQL or special commands) and produces outputs.
//! Shared by the interactive shell and by batch mode. A session may be
//! disconnected; `connect` opens or replaces its connection.

use std::sync::{Arc, RwLock};
use std::time::{Duration, Instant};

use super::interrupt;
use crate::client::{self as db, Client, ClientError, ConnectOptions, ResultSet, StatementResult};
pub use crate::client::{Relation, RelationKind};
use crate::dialect::{Dialect, Qualifier};
use crate::render::{Format, render_to_string};
use crate::server::Server;
use crate::store::{default_name, prompt_detail};
use crate::{special, sql};

/// What the editor completes from.
#[derive(Debug, Default, Clone)]
pub struct SchemaCache {
    /// The connected server; `None` while disconnected.
    pub server: Option<Server>,
    /// Tables and views of the current database, sorted by name.
    pub relations: Vec<Relation>,
    /// Databases on the server (servers with `db.` qualifiers only).
    pub databases: Vec<String>,
    /// `(qualifier, table)` pairs across the server's user databases (MySQL) or
    /// schemas (PostgreSQL), for `qualifier.` completion.
    pub qualified: Vec<(String, String)>,
    /// Relations of every user schema with their columns, for `schema.table.`
    /// completion (schema-qualifying servers only).
    pub schema_relations: Vec<Relation>,
}

impl SchemaCache {
    /// The server whose dialect applies; SQLite rules while disconnected.
    pub fn dialect(&self) -> Server {
        self.server.unwrap_or(Server::Sqlite)
    }

    /// Whether `name` is a database on the server, ignoring case.
    pub fn has_database(&self, name: &str) -> bool {
        self.databases.iter().any(|d| d.eq_ignore_ascii_case(name))
    }

    /// Whether `name` can qualify a table on this server: a database (MySQL) or
    /// a schema (PostgreSQL), ignoring case.
    pub fn has_qualifier(&self, name: &str) -> bool {
        match self.dialect().qualifier() {
            Qualifier::None => false,
            Qualifier::Database => self.has_database(name),
            Qualifier::Schema => self
                .qualified
                .iter()
                .any(|(s, _)| s.eq_ignore_ascii_case(name)),
        }
    }

    /// Finds a relation of `schema` by name, ignoring case.
    pub fn find_in(&self, schema: &str, name: &str) -> Option<&Relation> {
        self.schema_relations.iter().find(|r| {
            r.schema
                .as_deref()
                .is_some_and(|s| s.eq_ignore_ascii_case(schema))
                && r.name.eq_ignore_ascii_case(name)
        })
    }

    /// Finds a relation by name, ignoring case.
    pub fn find(&self, name: &str) -> Option<&Relation> {
        self.relations
            .iter()
            .find(|r| r.name.eq_ignore_ascii_case(name))
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Output {
    /// Rendered result or informational text, for stdout.
    Text(String),
    /// Row-count/timing line, for stdout. Only produced when `show_status` is set.
    Status(String),
    /// Error message, for stderr.
    Error(String),
    /// `\\disconnect` closed this connection; for stdout (`⚠`, yellow).
    Disconnected(Target),
    /// `\\reconnect` is about to reopen this connection; for stdout (gray).
    Reconnecting(Target),
    /// The user asked to leave the shell.
    Quit,
    /// `\\connect` without arguments: the shell should run the connect wizard.
    ConnectWizard,
    /// `\\connect <server> <args>`: the shell should connect with these options
    /// (asking for the password first when the options require it).
    Connect(Box<ConnectOptions>),
}

/// A connection as the user knows it: its name and, in parentheses, where it
/// points (`user@host:port` or the SQLite file).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Target {
    pub name: String,
    pub detail: String,
}

/// Returned when a command or query needs a connection and there is none.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
#[error("not connected to a database; use \\connect")]
pub struct NotConnected;

/// Prompt label while disconnected.
pub const DISCONNECTED_LABEL: &str = "(none)";

/// The parts of the prompt's first line.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct PromptInfo {
    pub name: String,
    /// `user@host:port` or the SQLite file.
    pub detail: Option<String>,
    pub database: Option<String>,
    pub server: Option<Server>,
    /// How long the last statement took.
    pub took: Option<Duration>,
    /// The connection was closed with `\\disconnect`; `name` and `detail` are
    /// the ones it had.
    pub disconnected: bool,
}

pub struct Session {
    client: Option<Box<dyn Client>>,
    pub(crate) format: Format,
    show_status: bool,
    schema: Arc<RwLock<SchemaCache>>,
    /// Server whose dialect the editor uses; SQLite while disconnected.
    dialect: Arc<RwLock<Server>>,
    /// Names the connection in the prompt.
    connection_name: Option<String>,
    /// `user@host:port` or the SQLite file, shown after the name.
    connection_detail: String,
    last_elapsed: Option<Duration>,
    /// Options of the last successful `connect`, kept across `\\disconnect`
    /// so `\\reconnect` can reopen it.
    last_options: Option<ConnectOptions>,
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

impl Session {
    /// A session connected through `client`.
    pub fn new(client: Box<dyn Client>, format: Format, show_status: bool) -> Self {
        let mut s = Self::disconnected(format, show_status);
        s.replace_client(client);
        s
    }

    /// A session without a connection.
    pub fn disconnected(format: Format, show_status: bool) -> Self {
        Self {
            client: None,
            format,
            show_status,
            schema: Arc::default(),
            dialect: Arc::new(RwLock::new(Server::Sqlite)),
            connection_name: None,
            connection_detail: String::new(),
            last_elapsed: None,
            last_options: None,
        }
    }

    /// Opens a connection for `opts` and, only if that succeeds, replaces the
    /// current one. On failure the current connection is kept.
    pub fn connect(&mut self, opts: &ConnectOptions) -> Result<(), ClientError> {
        let client = db::open(opts)?;
        self.replace_client(client);
        // Reconnecting to the same target keeps its name.
        if self.last_options.as_ref() != Some(opts) || self.connection_name.is_none() {
            self.connection_name = Some(default_name(opts));
        }
        self.last_options = Some(opts.clone());
        self.connection_detail = prompt_detail(opts);
        self.last_elapsed = None;
        Ok(())
    }

    /// Closes the connection, leaving the session disconnected. Returns whether
    /// there was one. The last options are kept for `\\reconnect`.
    pub fn disconnect(&mut self) -> bool {
        let was = self.client.take().is_some();
        *self.dialect.write().unwrap() = Server::Sqlite;
        self.last_elapsed = None;
        self.refresh_schema();
        was
    }

    /// The current connection, or the last one while disconnected.
    pub fn target(&self) -> Option<Target> {
        let name = self
            .connection_name
            .clone()
            .or_else(|| self.client.as_ref().map(|c| c.prompt_label()))
            .or_else(|| self.last_options.as_ref().map(default_name))?;
        Some(Target {
            name,
            detail: self.connection_detail.clone(),
        })
    }

    /// Options of the last successful connect, if any.
    pub fn last_options(&self) -> Option<&ConnectOptions> {
        self.last_options.as_ref()
    }

    /// Names the current connection in the prompt.
    pub fn set_connection_name(&mut self, name: String) {
        self.connection_name = Some(name);
    }

    /// What the prompt's first line shows.
    pub fn prompt_info(&self) -> PromptInfo {
        let Some(client) = &self.client else {
            // After `\\disconnect` the prompt keeps naming the old connection.
            if let (Some(_), Some(t)) = (&self.last_options, self.target()) {
                return PromptInfo {
                    name: t.name,
                    disconnected: true,
                    ..Default::default()
                };
            }
            return PromptInfo {
                name: DISCONNECTED_LABEL.to_string(),
                ..Default::default()
            };
        };
        PromptInfo {
            name: self
                .connection_name
                .clone()
                .unwrap_or_else(|| client.prompt_label()),
            detail: Some(self.connection_detail.clone()).filter(|d| !d.is_empty()),
            database: client.database(),
            server: Some(client.server()),
            took: self.last_elapsed,
            disconnected: false,
        }
    }

    fn replace_client(&mut self, client: Box<dyn Client>) {
        *self.dialect.write().unwrap() = client.server();
        // Drop the previous client (closing its connection) before refreshing.
        self.client = Some(client);
        self.refresh_schema();
    }

    pub fn is_connected(&self) -> bool {
        self.client.is_some()
    }

    /// The connected server, if any.
    pub fn server(&self) -> Option<Server> {
        self.client.as_ref().map(|c| c.server())
    }

    /// The client, or [`NotConnected`].
    pub fn client_mut(&mut self) -> Result<&mut (dyn Client + 'static), NotConnected> {
        self.client.as_deref_mut().ok_or(NotConnected)
    }

    /// Shared handle to the server whose dialect the editor should use.
    pub fn dialect(&self) -> Arc<RwLock<Server>> {
        Arc::clone(&self.dialect)
    }

    fn current_dialect(&self) -> Server {
        *self.dialect.read().unwrap()
    }

    pub fn schema(&self) -> Arc<RwLock<SchemaCache>> {
        Arc::clone(&self.schema)
    }

    pub fn format(&self) -> Format {
        self.format
    }

    pub fn prompt_label(&self) -> String {
        self.client
            .as_ref()
            .map_or_else(|| DISCONNECTED_LABEL.to_string(), |c| c.prompt_label())
    }

    pub fn render(&self, rs: &ResultSet) -> String {
        render_to_string(rs, self.format)
    }

    pub fn refresh_schema(&mut self) {
        let cache = match self.client.as_deref_mut() {
            Some(c) => {
                let server = c.server();
                let (databases, qualified) = if server.qualifier() != Qualifier::None {
                    (
                        c.database_names().unwrap_or_default(),
                        c.qualified_tables().unwrap_or_default(),
                    )
                } else {
                    Default::default()
                };
                let schema_relations = if server.qualifier() == Qualifier::Schema {
                    c.schema_relations().unwrap_or_default()
                } else {
                    Vec::new()
                };
                SchemaCache {
                    server: Some(server),
                    relations: c.relations().unwrap_or_default(),
                    databases,
                    qualified,
                    schema_relations,
                }
            }
            None => SchemaCache::default(),
        };
        *self.schema.write().unwrap() = cache;
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
        let dialect = self.current_dialect();
        let Some(client) = self.client.as_deref_mut() else {
            return vec![Output::Error(NotConnected.to_string())];
        };
        let (body, vertical) = sql::split_vertical(input);
        let statements = sql::split_statements_with(body, &dialect.lex_rules());
        let mut outputs = Vec::new();
        let mut refresh = false;
        let last = statements.len().saturating_sub(1);
        for (i, stmt) in statements.iter().enumerate() {
            let started = Instant::now();
            let result = {
                let _armed = interrupt::arm(client.interrupt_handle());
                client.execute(stmt)
            };
            let elapsed = started.elapsed();
            self.last_elapsed = Some(elapsed);
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
                    refresh |= dialect.changes_schema(stmt);
                }
                Err(e) => {
                    outputs.push(Output::Error(e.to_string()));
                    refresh |= dialect.changes_schema(stmt);
                    break;
                }
            }
        }
        if refresh {
            self.refresh_schema();
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
                other => format!("{other:?}"),
            })
            .collect::<Vec<_>>()
            .join("\n")
    }

    #[test]
    fn prompt_shows_the_connection_name() {
        let mut s = Session::disconnected(Format::Tsv, false);
        s.connect(&ConnectOptions::sqlite(Some("/x/app.db"))).ok();
        // Not opened (no such directory), so still disconnected.
        assert_eq!(s.prompt_info().name, "(none)");
        s.connect(&ConnectOptions::sqlite(None)).unwrap();
        assert_eq!(s.prompt_info().name, "memory");
        assert_eq!(s.prompt_info().detail.as_deref(), Some(":memory:"));
        s.set_connection_name("shopify".into());
        assert_eq!(s.prompt_info().name, "shopify");
        assert_eq!(s.prompt_info().took, None);
        s.handle("select 1;");
        assert!(s.prompt_info().took.is_some());
    }

    #[test]
    fn recovers_after_error() {
        let mut s = session(false);
        let out = s.handle("select * from missing;");
        assert!(matches!(&out[0], Output::Error(e) if e.contains("no such table: missing")));
        let out = s.handle("select 1 as n;");
        assert!(texts(&out).contains("│ n │"));
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
        assert!(cache.read().unwrap().relations.is_empty());
        s.handle("create table orders(id integer, total real);");
        s.handle("create view big as select * from orders;");
        let c = cache.read().unwrap();
        let names: Vec<_> = c.relations.iter().map(|r| r.name.as_str()).collect();
        assert_eq!(names, vec!["big", "orders"]);
        let orders = c.find("ORDERS").unwrap();
        assert_eq!(orders.kind, RelationKind::Table);
        assert_eq!(c.find("big").unwrap().kind, RelationKind::View);
        let cols: Vec<_> = orders
            .columns
            .iter()
            .map(|c| (c.name.as_str(), c.type_name.as_str()))
            .collect();
        assert_eq!(cols, vec![("id", "INTEGER"), ("total", "REAL")]);
    }

    #[test]
    fn schema_cache_sees_added_column() {
        let mut s = session(false);
        let cache = s.schema();
        s.handle("create table orders(id);");
        s.handle("alter table orders add column note text;");
        let c = cache.read().unwrap();
        let names: Vec<_> = c
            .find("orders")
            .unwrap()
            .columns
            .iter()
            .map(|c| c.name.as_str())
            .collect();
        assert_eq!(names, vec!["id", "note"]);
    }

    #[test]
    fn prompt_keeps_the_name_after_disconnect() {
        let mut s = session(false);
        s.connect(&ConnectOptions::sqlite(None)).unwrap();
        s.set_connection_name("shop".into());
        s.disconnect();
        let info = s.prompt_info();
        assert_eq!(info.name, "shop");
        assert_eq!(info.detail, None);
        assert!(info.disconnected && info.server.is_none());
        s.connect(&ConnectOptions::sqlite(None)).unwrap();
        assert!(!s.prompt_info().disconnected);
    }

    #[test]
    fn sql_while_disconnected_suggests_connect() {
        let mut s = Session::disconnected(Format::Ascii, false);
        assert!(!s.is_connected());
        assert_eq!(s.prompt_label(), "(none)");
        assert_eq!(s.prompt_info().name, "(none)");
        let out = s.handle("select 1;");
        assert!(
            matches!(&out[0], Output::Error(e) if e.contains("\\connect")),
            "{out:?}"
        );
    }

    #[test]
    fn connect_replaces_and_failure_keeps_connection() {
        let dir = tempfile::tempdir().unwrap();
        let a = dir.path().join("a.db");
        let mut s = Session::disconnected(Format::Tsv, false);
        s.connect(&ConnectOptions::sqlite(a.to_str())).unwrap();
        s.handle("create table only_in_a(x);");
        assert!(s.schema().read().unwrap().find("only_in_a").is_some());

        let bad = dir.path().join("missing-dir").join("b.db");
        assert!(s.connect(&ConnectOptions::sqlite(bad.to_str())).is_err());
        assert!(s.prompt_label().ends_with("a.db"));
        assert!(s.schema().read().unwrap().find("only_in_a").is_some());

        s.connect(&ConnectOptions::sqlite(None)).unwrap();
        assert_eq!(s.prompt_label(), ":memory:");
        assert!(s.schema().read().unwrap().relations.is_empty());
        assert_eq!(s.server(), Some(Server::Sqlite));
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
        use std::sync::atomic::{AtomicBool, Ordering};
        let mut s = session(false);
        // The interrupt slot is process-wide and other tests running in parallel
        // re-arm and clear it, so re-arm this session's handle before each trigger
        // until the query has returned.
        let handle = s.client_mut().unwrap().interrupt_handle();
        let done = Arc::new(AtomicBool::new(false));
        let flag = Arc::clone(&done);
        let t = std::thread::spawn(move || {
            while !flag.load(Ordering::Relaxed) {
                std::thread::sleep(Duration::from_millis(50));
                let _armed = interrupt::arm(Arc::clone(&handle));
                interrupt::trigger();
            }
        });
        let out = s.handle(
            "with recursive c(x) as (select 1 union all select x+1 from c) select count(*) from c;",
        );
        done.store(true, Ordering::Relaxed);
        t.join().unwrap();
        assert!(matches!(&out[0], Output::Error(e) if e.contains("interrupted")));
        assert!(texts(&s.handle("select 1;")).contains("│ 1 │"));
    }
}

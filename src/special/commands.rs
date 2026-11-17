//! Handlers for the special commands.

use clap::error::ErrorKind;

use crate::client::{ClientError, ResultSet, Value};
use crate::connect::split_words;
use crate::render::Format;
use crate::server::Server;
use crate::tui::session::{NotConnected, Output, Session};

/// The session's client, or return a not-connected error from the handler.
macro_rules! client {
    ($s:expr) => {
        match $s.client_mut() {
            Ok(c) => c,
            Err(e) => return vec![Output::Error(e.to_string())],
        }
    };
}

fn fail(e: ClientError) -> Vec<Output> {
    vec![Output::Error(e.to_string())]
}

fn name_list(column: &str, names: Vec<String>) -> ResultSet {
    ResultSet {
        columns: vec![column.to_string()],
        rows: names.into_iter().map(|n| vec![Value::Text(n)]).collect(),
    }
}

pub fn tables(s: &mut Session, arg: Option<&str>) -> Vec<Output> {
    match client!(s).tables(arg) {
        Ok(t) => vec![Output::Text(s.render(&name_list("name", t)))],
        Err(e) => fail(e),
    }
}

pub fn views(s: &mut Session, arg: Option<&str>) -> Vec<Output> {
    match client!(s).views(arg) {
        Ok(t) => vec![Output::Text(s.render(&name_list("name", t)))],
        Err(e) => fail(e),
    }
}

pub fn schema(s: &mut Session, arg: Option<&str>) -> Vec<Output> {
    match client!(s).schema(arg) {
        Ok(stmts) => {
            let mut text = String::new();
            for st in stmts {
                text.push_str(&st);
                text.push_str(";\n");
            }
            vec![Output::Text(text)]
        }
        Err(e) => fail(e),
    }
}

pub fn indexes(s: &mut Session, arg: Option<&str>) -> Vec<Output> {
    match client!(s).indexes(arg) {
        Ok(rs) => vec![Output::Text(s.render(&rs))],
        Err(e) => fail(e),
    }
}

pub fn databases(s: &mut Session, _arg: Option<&str>) -> Vec<Output> {
    match client!(s).databases() {
        Ok(rs) => vec![Output::Text(s.render(&rs))],
        Err(e) => fail(e),
    }
}

pub fn describe(s: &mut Session, arg: Option<&str>) -> Vec<Output> {
    let Some(table) = arg else {
        return tables(s, None);
    };
    match client!(s).columns(table) {
        Ok(cols) if cols.is_empty() => vec![Output::Error(format!("no such table: {table}"))],
        Ok(cols) => {
            let yes_no = |b: bool| Value::Text(if b { "YES" } else { "NO" }.into());
            let rs = ResultSet {
                columns: ["column", "type", "nullable", "default", "primary_key"]
                    .map(String::from)
                    .to_vec(),
                rows: cols
                    .into_iter()
                    .map(|c| {
                        vec![
                            Value::Text(c.name),
                            Value::Text(c.type_name),
                            yes_no(!c.not_null),
                            c.default.map_or(Value::Null, Value::Text),
                            yes_no(c.primary_key),
                        ]
                    })
                    .collect(),
            };
            vec![Output::Text(s.render(&rs))]
        }
        Err(e) => fail(e),
    }
}

/// `\\structure`: every table and view with its columns, checks, defaults and foreign keys.
pub fn structure(s: &mut Session, _arg: Option<&str>) -> Vec<Output> {
    let client = client!(s);
    let mut blocks = Vec::new();
    // `tables()` lists views and materialized views too.
    let tables = match client.tables(None) {
        Ok(t) => t,
        Err(e) => return fail(e),
    };
    if tables.is_empty() {
        return vec![Output::Text("No tables.\n".into())];
    }
    for table in tables {
        let (cols, cons) = match (client.columns(&table), client.constraints(&table)) {
            (Ok(c), Ok(k)) => (c, k),
            (Err(e), _) | (_, Err(e)) => return fail(e),
        };
        blocks.push((table, cols, cons));
    }
    let mut out = Vec::new();
    for (table, cols, cons) in blocks {
        let (rs, table_checks) = structure_rows(&cols, &cons);
        let mut text = format!("{table}\n{}", s.render(&rs));
        for expr in table_checks {
            text.push_str(&format!("CHECK {expr}\n"));
        }
        out.push(Output::Text(text));
    }
    out
}

/// Whether `expr` mentions `name` as a whole identifier, quoted or not.
fn mentions(expr: &str, name: &str) -> bool {
    let is_id = |c: char| c.is_alphanumeric() || c == '_';
    let mut from = 0;
    while let Some(pos) = expr[from..].find(name) {
        let at = from + pos;
        let end = at + name.len();
        if !expr[..at].ends_with(is_id) && !expr[end..].starts_with(is_id) {
            return true;
        }
        from = end;
    }
    false
}

/// Rows for one table, and the checks that belong to no single column.
fn structure_rows(
    cols: &[crate::client::ColumnInfo],
    cons: &crate::client::Constraints,
) -> (ResultSet, Vec<String>) {
    let mut per_column: Vec<Vec<&str>> = vec![Vec::new(); cols.len()];
    let mut table_checks = Vec::new();
    for check in &cons.checks {
        let owner = match &check.column {
            Some(c) => cols.iter().position(|x| &x.name == c),
            None => {
                let hits: Vec<usize> = (0..cols.len())
                    .filter(|&i| mentions(&check.expr, &cols[i].name))
                    .collect();
                if let [one] = hits[..] {
                    Some(one)
                } else {
                    None
                }
            }
        };
        match owner {
            Some(i) => per_column[i].push(&check.expr),
            None => table_checks.push(check.expr.clone()),
        }
    }
    let rows = cols
        .iter()
        .zip(&per_column)
        .map(|(c, checks)| {
            let fks: Vec<String> = cons
                .foreign_keys
                .iter()
                .filter(|f| f.column == c.name)
                .map(|f| format!("{}({})", f.ref_table, f.ref_column))
                .collect();
            let joined = |v: Vec<String>| {
                if v.is_empty() {
                    Value::Null
                } else {
                    Value::Text(v.join("; "))
                }
            };
            vec![
                Value::Text(c.name.clone()),
                Value::Text(c.type_name.clone()),
                Value::Text(if c.not_null { "No" } else { "Yes" }.into()),
                joined(checks.iter().map(|e| e.to_string()).collect()),
                c.default.clone().map_or(Value::Null, Value::Text),
                joined(fks),
            ]
        })
        .collect();
    let rs = ResultSet {
        columns: [
            "column",
            "type",
            "nullable",
            "check",
            "default",
            "foreign_key",
        ]
        .map(String::from)
        .to_vec(),
        rows,
    };
    (rs, table_checks)
}

pub fn table_format(s: &mut Session, arg: Option<&str>) -> Vec<Output> {
    match arg {
        None => vec![Output::Text(format!(
            "Current format: {}\nValid formats: {}\n",
            s.format,
            Format::NAMES.join(", ")
        ))],
        Some(name) => match name.parse::<Format>() {
            Ok(f) => {
                s.format = f;
                vec![Output::Text(format!("Format set to {f}\n"))]
            }
            Err(e) => vec![Output::Error(e)],
        },
    }
}

/// `\\connect` alone opens the wizard; `\\connect <server> <args>` parses the
/// server's connection arguments. The shell performs the actual connection.
pub fn connect(_s: &mut Session, arg: Option<&str>) -> Vec<Output> {
    let Some(arg) = arg else {
        return vec![Output::ConnectWizard];
    };
    let words = match split_words(arg) {
        Ok(w) => w,
        Err(e) => return vec![Output::Error(e)],
    };
    let Some((name, rest)) = words.split_first() else {
        return vec![Output::ConnectWizard];
    };
    let Some(server) = Server::parse(name) else {
        return vec![Output::Error(format!(
            "unknown server: {name} (supported: {})",
            Server::names()
        ))];
    };
    match server.parse_args(rest) {
        Ok(opts) => vec![Output::Connect(Box::new(opts))],
        Err(e) if e.kind() == ErrorKind::DisplayHelp => vec![Output::Text(e.to_string())],
        Err(e) => vec![Output::Error(e.to_string().trim_end().to_string())],
    }
}

/// `\\disconnect` closes the connection and keeps the shell open.
pub fn disconnect(s: &mut Session, _arg: Option<&str>) -> Vec<Output> {
    let target = s.target();
    if !s.disconnect() {
        return vec![Output::Error(NotConnected.to_string())];
    }
    match target {
        Some(t) => vec![Output::Disconnected(t)],
        None => vec![Output::Text("Disconnected.\n".to_string())],
    }
}

/// `\\reconnect` reopens the last connection with its options. The shell
/// performs the actual connection.
pub fn reconnect(s: &mut Session, _arg: Option<&str>) -> Vec<Output> {
    let (Some(opts), Some(target)) = (s.last_options(), s.target()) else {
        return vec![Output::Error(
            "no previous connection; use \\connect".to_string(),
        )];
    };
    vec![
        Output::Reconnecting(target),
        Output::Connect(Box::new(opts.clone())),
    ]
}

pub fn help(_s: &mut Session, _arg: Option<&str>) -> Vec<Output> {
    let mut text = String::new();
    for c in super::COMMANDS {
        text.push_str(&format!("{:<20} {}\n", c.syntax, c.help));
    }
    text.push_str(&format!(
        "{:<20} {}\n",
        "<sql>\\G", "Run a query and show it in vertical format."
    ));
    text.push_str(&format!(
        "{:<20} {}\n",
        "<sql> \\e", "Edit the current input in $EDITOR (also Ctrl-O)."
    ));
    vec![Output::Text(text)]
}

pub fn quit(_s: &mut Session, _arg: Option<&str>) -> Vec<Output> {
    vec![Output::Quit]
}

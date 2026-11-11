//! Handlers for the special commands.

use clap::error::ErrorKind;

use crate::client::{ClientError, ResultSet, Value};
use crate::connect::{ServerArgs, split_words};
use crate::render::Format;
use crate::server::Server;
use crate::tui::session::{Output, Session};

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
        Ok(opts) => vec![Output::Connect(opts)],
        Err(e) if e.kind() == ErrorKind::DisplayHelp => vec![Output::Text(e.to_string())],
        Err(e) => vec![Output::Error(e.to_string().trim_end().to_string())],
    }
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

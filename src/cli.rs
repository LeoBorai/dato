//! Command-line entry point: argument parsing and mode selection.

use std::io::{self, IsTerminal, Read};
use std::process::ExitCode;

use clap::Parser;

use crate::client::{self, Target};
use crate::render::Format;
use crate::sql::is_complete;
use crate::tui::{self, session::Session};

#[derive(Debug, Parser)]
#[command(
    name = "dato",
    version,
    about = "Database CLI with completion and table formats"
)]
pub struct Args {
    /// Database to open: a SQLite file path, `:memory:`, or a server URL
    /// (postgres://, mysql://). Defaults to an in-memory SQLite database.
    pub target: Option<String>,

    /// Run this SQL and exit.
    #[arg(short = 'e', long = "execute", value_name = "SQL")]
    pub execute: Option<String>,

    /// Output format: ascii, psql, markdown, csv, tsv, json, vertical.
    #[arg(short = 'f', long = "format", value_name = "FORMAT")]
    pub format: Option<Format>,
}

#[derive(Debug, PartialEq, Eq)]
pub enum Mode {
    /// Run the given SQL.
    OneShot(String),
    /// Read SQL from stdin.
    Stdin,
    Interactive,
}

pub fn select_mode(args: &Args, stdin_is_terminal: bool) -> Mode {
    match (&args.execute, stdin_is_terminal) {
        (Some(sql), _) => Mode::OneShot(sql.clone()),
        (None, false) => Mode::Stdin,
        (None, true) => Mode::Interactive,
    }
}

/// Output format: explicit choice, else `ascii` on a terminal and `tsv` when piped.
/// The interactive shell always defaults to `ascii`.
pub fn default_format(args: &Args, interactive: bool, stdout_is_terminal: bool) -> Format {
    args.format.unwrap_or(if interactive || stdout_is_terminal {
        Format::Ascii
    } else {
        Format::Tsv
    })
}

/// Runs a script statement by statement. Returns the exit code: 0, or 1 on the first error.
pub fn run_batch(
    session: &mut Session,
    script: &str,
    out: &mut dyn io::Write,
    err: &mut dyn io::Write,
) -> i32 {
    let mut buffer = String::new();
    let mut inputs = Vec::new();
    for line in script.lines() {
        if !buffer.is_empty() {
            buffer.push('\n');
        }
        buffer.push_str(line);
        if is_complete(&buffer) {
            inputs.push(std::mem::take(&mut buffer));
        }
    }
    if !buffer.trim().is_empty() {
        inputs.push(buffer);
    }
    for input in inputs {
        let outputs = session.handle(&input);
        let failed = outputs
            .iter()
            .any(|o| matches!(o, tui::session::Output::Error(_)));
        let quit = tui::emit(&outputs, out, err);
        if failed {
            return 1;
        }
        if quit {
            break;
        }
    }
    0
}

pub fn run() -> ExitCode {
    let args = Args::parse();
    let mode = select_mode(&args, io::stdin().is_terminal());
    let interactive = mode == Mode::Interactive;
    let format = default_format(&args, interactive, io::stdout().is_terminal());

    let target = Target::parse(args.target.as_deref());
    let client = match client::open(&target) {
        Ok(c) => c,
        Err(e) => {
            eprintln!("Error: {e}");
            return ExitCode::from(1);
        }
    };
    let mut session = Session::new(client, format, interactive);

    let code = match mode {
        Mode::Interactive => tui::run(session),
        Mode::OneShot(sql) => run_batch(&mut session, &sql, &mut io::stdout(), &mut io::stderr()),
        Mode::Stdin => {
            let mut script = String::new();
            if let Err(e) = io::stdin().read_to_string(&mut script) {
                eprintln!("Error: cannot read stdin: {e}");
                return ExitCode::from(1);
            }
            run_batch(&mut session, &script, &mut io::stdout(), &mut io::stderr())
        }
    };
    ExitCode::from(code as u8)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::client::sqlite::SqliteClient;

    fn parse(argv: &[&str]) -> Args {
        Args::try_parse_from(std::iter::once("dato").chain(argv.iter().copied())).unwrap()
    }

    #[test]
    fn mode_selection() {
        assert_eq!(
            select_mode(&parse(&["-e", "select 1"]), true),
            Mode::OneShot("select 1".into())
        );
        assert_eq!(
            select_mode(&parse(&["-e", "select 1"]), false),
            Mode::OneShot("select 1".into())
        );
        assert_eq!(select_mode(&parse(&[]), false), Mode::Stdin);
        assert_eq!(select_mode(&parse(&[]), true), Mode::Interactive);
    }

    #[test]
    fn format_defaults() {
        let a = parse(&[]);
        assert_eq!(default_format(&a, true, false), Format::Ascii);
        assert_eq!(default_format(&a, false, true), Format::Ascii);
        assert_eq!(default_format(&a, false, false), Format::Tsv);
        assert_eq!(
            default_format(&parse(&["-f", "csv"]), false, false),
            Format::Csv
        );
    }

    #[test]
    fn invalid_format_is_rejected() {
        let e = Args::try_parse_from(["dato", "-f", "bogus"]).unwrap_err();
        assert!(e.to_string().contains("valid formats"));
    }

    fn run_script(script: &str) -> (i32, String, String) {
        let mut s = Session::new(Box::new(SqliteClient::open_memory()), Format::Tsv, false);
        let (mut out, mut err) = (Vec::new(), Vec::new());
        let code = run_batch(&mut s, script, &mut out, &mut err);
        (
            code,
            String::from_utf8(out).unwrap(),
            String::from_utf8(err).unwrap(),
        )
    }

    #[test]
    fn batch_runs_multi_line_statements() {
        let (code, out, _) =
            run_script("create table t(a);\ninsert into t\n values (1);\nselect a from t;\n");
        assert_eq!((code, out.as_str()), (0, "a\n1\n"));
    }

    #[test]
    fn batch_runs_trailing_statement_without_semicolon() {
        let (code, out, _) = run_script("select 7 as n");
        assert_eq!((code, out.as_str()), (0, "n\n7\n"));
    }

    #[test]
    fn batch_stops_at_first_error() {
        let (code, out, err) = run_script("select * from missing;\nselect 1;");
        assert_eq!(code, 1);
        assert!(out.is_empty());
        assert!(err.contains("no such table: missing"));
    }

    #[test]
    fn batch_runs_special_commands() {
        let (code, out, _) = run_script("create table users(a);\n.tables\n");
        assert_eq!(code, 0);
        assert!(out.contains("users"));
    }
}

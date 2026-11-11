//! Command-line entry point: argument parsing and mode selection.

use std::io::{self, IsTerminal, Read};
use std::process::ExitCode;

use clap::{Parser, Subcommand};

use crate::client::ConnectOptions;
use crate::connect::{MySqlArgs, SqliteArgs, ask_password};
use crate::render::Format;
use crate::sql::is_complete;
use crate::tui::{
    self,
    session::{Output, Session},
};

#[derive(Debug, Parser)]
#[command(
    name = "dato",
    version,
    about = "Database CLI with completion and table formats",
    args_conflicts_with_subcommands = true
)]
pub struct Args {
    #[command(subcommand)]
    pub command: Option<ServerCommand>,

    /// SQLite database file or `:memory:`; shorthand for `dato sqlite FILE`.
    /// Without a target the interactive shell starts disconnected.
    pub target: Option<String>,

    /// Run this SQL and exit.
    #[arg(short = 'e', long = "execute", value_name = "SQL", global = true)]
    pub execute: Option<String>,

    /// Output format: ascii, psql, markdown, csv, tsv, json, vertical.
    #[arg(short = 'T', long = "format", value_name = "FORMAT", global = true)]
    pub format: Option<Format>,
}

/// One subcommand per server.
#[derive(Debug, Subcommand)]
pub enum ServerCommand {
    Sqlite(SqliteArgs),
    Mysql(MySqlArgs),
}

/// Where to connect at startup: `Ok(None)` means start disconnected.
pub fn startup_options(args: &Args) -> Result<Option<ConnectOptions>, String> {
    match (&args.command, args.target.as_deref()) {
        (Some(ServerCommand::Sqlite(a)), _) => Ok(Some(a.options())),
        (Some(ServerCommand::Mysql(a)), _) => Ok(Some(a.options())),
        (None, Some(t)) if t.contains("://") => Err(format!(
            "URL targets are not supported: {t}; use `dato mysql -h HOST -u USER -p DB` \
             or `dato sqlite FILE`"
        )),
        (None, Some(t)) => Ok(Some(ConnectOptions::sqlite(Some(t)))),
        (None, None) => Ok(None),
    }
}

/// Error when one-shot or piped execution has nothing to run against.
pub const TARGET_REQUIRED: &str =
    "a database target is required with -e or piped input (e.g. `dato app.db` or `dato mysql ...`)";

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
        // `\\connect` needs the interactive shell; `emit` reports it as an error.
        let failed = outputs.iter().any(|o| {
            matches!(
                o,
                Output::Error(_) | Output::ConnectWizard | Output::Connect(_)
            )
        });
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

    let opts = match startup_options(&args) {
        Ok(o) => o,
        Err(e) => {
            eprintln!("Error: {e}");
            return ExitCode::from(2);
        }
    };
    let mut session = Session::disconnected(format, interactive);
    match opts {
        None if !interactive => {
            eprintln!("Error: {TARGET_REQUIRED}");
            return ExitCode::from(1);
        }
        None => {}
        Some(mut opts) => {
            if let Err(e) = ask_password(&mut opts) {
                eprintln!("Error: cannot read password: {e}");
                return ExitCode::from(1);
            }
            if let Err(e) = session.connect(&opts) {
                eprintln!("Error: {e}");
                return ExitCode::from(1);
            }
        }
    }

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
            default_format(&parse(&["-T", "csv"]), false, false),
            Format::Csv
        );
        assert_eq!(
            default_format(&parse(&["--format", "json"]), false, false),
            Format::Json
        );
        assert_eq!(
            default_format(&parse(&["mysql", "-u", "root", "-T", "csv"]), false, false),
            Format::Csv
        );
    }

    #[test]
    fn invalid_format_is_rejected() {
        let e = Args::try_parse_from(["dato", "-T", "bogus"]).unwrap_err();
        assert!(e.to_string().contains("valid formats"));
        assert!(Args::try_parse_from(["dato", "-f", "csv"]).is_err());
    }

    #[test]
    fn startup_targets() {
        let opts = |argv: &[&str]| startup_options(&parse(argv));
        assert_eq!(opts(&[]), Ok(None));
        assert_eq!(
            opts(&["app.db"]),
            Ok(Some(ConnectOptions::sqlite(Some("app.db"))))
        );
        assert_eq!(opts(&["sqlite", "app.db"]), opts(&["app.db"]));
        assert_eq!(opts(&["sqlite"]), Ok(Some(ConnectOptions::sqlite(None))));
        assert!(matches!(
            opts(&["mysql", "-h", "db.local", "-u", "app", "-p", "shop", "-e", "select 1"]),
            Ok(Some(ConnectOptions::MySql(o)))
                if o.host == "db.local" && o.user == "app" && o.password_required
                    && o.database.as_deref() == Some("shop")
        ));
        let err = opts(&["mysql://root@localhost/shop"]).unwrap_err();
        assert!(err.contains("dato mysql"), "{err}");
    }

    #[test]
    fn mysql_subcommand_help_and_password_flag() {
        let help = Args::try_parse_from(["dato", "mysql", "-h", "db.local", "--help"]).unwrap_err();
        assert_eq!(help.kind(), clap::error::ErrorKind::DisplayHelp);
        assert!(help.to_string().contains("-h, --host"), "{help}");
        assert!(Args::try_parse_from(["dato", "mysql", "-u", "root", "-psecret"]).is_err());
        assert!(matches!(
            parse(&["mysql", "-D", "shop", "-e", "select database();"]).command,
            Some(ServerCommand::Mysql(_))
        ));
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
    fn batch_rejects_connect() {
        for script in ["\\connect sqlite a.db\nselect 1;", "\\connect\n"] {
            let (code, out, err) = run_script(script);
            assert_eq!(code, 1, "{script}");
            assert!(out.is_empty());
            assert!(
                err.contains("\\connect is only available in the interactive shell"),
                "{err}"
            );
        }
    }

    #[test]
    fn batch_runs_special_commands() {
        let (code, out, _) = run_script("create table users(a);\n.tables\n");
        assert_eq!(code, 0);
        assert!(out.contains("users"));
    }
}

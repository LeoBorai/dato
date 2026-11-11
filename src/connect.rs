//! Connection arguments: the per-server argument layer of [`Server`].
//!
//! The same parsers back the CLI subcommands (`dato sqlite ...`, `dato mysql ...`)
//! and `\connect <server> <args>`, so both accept identical arguments. Login
//! arguments follow each server's own command-line client.

use clap::{ArgAction, Parser};

use crate::client::{ConnectOptions, MySqlOptions, default_user};
use crate::server::Server;

/// `dato sqlite [FILE]`.
#[derive(Debug, Clone, PartialEq, Eq, Parser)]
#[command(name = "sqlite", about = "Connect to a SQLite database file")]
pub struct SqliteArgs {
    /// Database file, created if missing. `:memory:` or no file opens an
    /// in-memory database.
    #[arg(value_name = "FILE")]
    pub file: Option<String>,
}

impl SqliteArgs {
    pub fn options(&self) -> ConnectOptions {
        ConnectOptions::sqlite(self.file.as_deref())
    }
}

/// `dato mysql [-h HOST] [-P PORT] [-u USER] [-p] [-S SOCKET] [-D DB | DB]`.
#[derive(Debug, Clone, PartialEq, Eq, Parser)]
#[command(
    name = "mysql",
    about = "Connect to a MySQL server",
    disable_help_flag = true
)]
pub struct MySqlArgs {
    /// Server host.
    #[arg(short = 'h', long, default_value = "localhost")]
    pub host: String,

    /// TCP port.
    #[arg(short = 'P', long, default_value_t = 3306)]
    pub port: u16,

    /// User name. Defaults to the current OS user.
    #[arg(short = 'u', long)]
    pub user: Option<String>,

    /// Ask for the password (input is not echoed).
    #[arg(short = 'p', long)]
    pub password: bool,

    /// Unix socket file; used instead of host and port.
    #[arg(short = 'S', long)]
    pub socket: Option<String>,

    /// Database to use.
    #[arg(
        short = 'D',
        long = "database",
        value_name = "DB",
        conflicts_with = "db"
    )]
    pub database: Option<String>,

    /// Database to use (same as -D).
    #[arg(value_name = "DB")]
    pub db: Option<String>,

    /// Print help.
    #[arg(long, action = ArgAction::Help)]
    help: Option<bool>,
}

impl MySqlArgs {
    pub fn options(&self) -> ConnectOptions {
        ConnectOptions::MySql(MySqlOptions {
            host: self.host.clone(),
            port: self.port,
            user: self.user.clone().unwrap_or_else(default_user),
            password: None,
            password_required: self.password,
            socket: self.socket.clone(),
            database: self.database.clone().or_else(|| self.db.clone()),
        })
    }
}

/// The argument layer of [`Server`]: parses that server's connection arguments.
pub trait ServerArgs {
    /// Parses `args` (without the server name) into connection options. The
    /// error is clap's rendered message, help output included.
    fn parse_args(&self, args: &[String]) -> Result<ConnectOptions, clap::Error>;
}

impl ServerArgs for Server {
    fn parse_args(&self, args: &[String]) -> Result<ConnectOptions, clap::Error> {
        let argv = std::iter::once(self.name().to_string()).chain(args.iter().cloned());
        match self {
            Server::Sqlite => SqliteArgs::try_parse_from(argv).map(|a| a.options()),
            Server::MySql => MySqlArgs::try_parse_from(argv).map(|a| a.options()),
        }
    }
}

/// Asks for the MySQL password on the terminal with echo off when the options
/// require one (`-p`) and none is set yet. Other options are left unchanged.
pub fn ask_password(opts: &mut ConnectOptions) -> std::io::Result<()> {
    if let ConnectOptions::MySql(o) = opts
        && o.password_required
        && o.password.is_none()
    {
        o.password = Some(rpassword::prompt_password("Enter password: ")?);
    }
    Ok(())
}

/// Splits a command line into words: whitespace separates, single and double
/// quotes group, and a backslash escapes the next character outside single quotes.
pub fn split_words(line: &str) -> Result<Vec<String>, String> {
    let mut words = Vec::new();
    let mut cur = String::new();
    let mut in_word = false;
    let mut chars = line.chars();
    while let Some(c) = chars.next() {
        match c {
            c if c.is_whitespace() => {
                if in_word {
                    words.push(std::mem::take(&mut cur));
                    in_word = false;
                }
            }
            '\'' => {
                in_word = true;
                loop {
                    match chars.next() {
                        Some('\'') => break,
                        Some(c) => cur.push(c),
                        None => return Err("unterminated ' quote".into()),
                    }
                }
            }
            '"' => {
                in_word = true;
                loop {
                    match chars.next() {
                        Some('"') => break,
                        Some('\\') => match chars.next() {
                            Some(c) => cur.push(c),
                            None => return Err("unterminated \" quote".into()),
                        },
                        Some(c) => cur.push(c),
                        None => return Err("unterminated \" quote".into()),
                    }
                }
            }
            '\\' => {
                in_word = true;
                if let Some(c) = chars.next() {
                    cur.push(c);
                }
            }
            c => {
                in_word = true;
                cur.push(c);
            }
        }
    }
    if in_word {
        words.push(cur);
    }
    Ok(words)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn words(s: &str) -> Vec<String> {
        split_words(s).unwrap()
    }

    fn mysql(args: &str) -> Result<MySqlOptions, clap::Error> {
        match Server::MySql.parse_args(&words(args))? {
            ConnectOptions::MySql(o) => Ok(o),
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn word_splitting() {
        assert_eq!(
            words(" a  'b c' \"d \\\" e\" f\\ g "),
            ["a", "b c", "d \" e", "f g"]
        );
        assert_eq!(words("''"), [""]);
        assert!(split_words("'open").is_err());
    }

    #[test]
    fn sqlite_args() {
        assert_eq!(
            Server::Sqlite.parse_args(&words("app.db")).unwrap(),
            ConnectOptions::sqlite(Some("app.db"))
        );
        assert_eq!(
            Server::Sqlite.parse_args(&[]).unwrap(),
            ConnectOptions::sqlite(None)
        );
    }

    #[test]
    fn mysql_login_args() {
        let o = mysql("-h db.local -P 3307 -u app -p shop").unwrap();
        assert_eq!(o.host, "db.local");
        assert_eq!(o.port, 3307);
        assert_eq!(o.user, "app");
        assert!(o.password_required);
        assert_eq!(o.password, None);
        assert_eq!(o.database.as_deref(), Some("shop"));

        let o = mysql("--host x --user root -D crm -S /tmp/m.sock").unwrap();
        assert_eq!(o.database.as_deref(), Some("crm"));
        assert_eq!(o.socket.as_deref(), Some("/tmp/m.sock"));
        assert!(!o.password_required);
    }

    #[test]
    fn mysql_defaults() {
        let o = mysql("").unwrap();
        assert_eq!((o.host.as_str(), o.port), ("localhost", 3306));
        assert_eq!(o.user, default_user());
        assert_eq!(o.database, None);
    }

    #[test]
    fn mysql_rejects_attached_password_and_lists_host_in_help() {
        assert!(mysql("-u root -psecret").is_err());
        let help = mysql("--help").unwrap_err();
        assert_eq!(help.kind(), clap::error::ErrorKind::DisplayHelp);
        assert!(help.to_string().contains("-h, --host"), "{help}");
        assert!(mysql("-D a b").is_err());
    }
}

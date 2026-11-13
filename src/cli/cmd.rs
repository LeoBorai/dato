//! One file per server: that server's command-line arguments.
//!
//! Each server has a `clap::Parser` struct with only its own flags, so
//! `dato <server> --help` lists that server's options and a letter can mean
//! different things on different servers (`-P` is the MySQL port, `-p` the
//! PostgreSQL one). The same structs back the CLI subcommands and
//! `\connect <server> <args>`.
//!
//! # Adding a server
//!
//! 1. Add a variant to [`Server`] (`src/server.rs`) and fix the compile errors
//!    in each layer's `match`.
//! 2. Add `cmd/<name>.rs` with a `Parser` struct and a [`ServerCmd`] impl.
//! 3. Add the struct to [`crate::cli::Command`] and to [`Server::parse_args`].
//! 4. Add `client/<name>/` (client, options, value mapping) and `dialect/<name>.rs`.
//! 5. Add the connect form entry in `ui/form.rs`.

pub mod mysql;
pub mod psql;
pub mod sqlite;

use clap::{Parser, Subcommand};

use crate::client::ConnectOptions;
use crate::server::Server;

pub use mysql::MySql;
pub use psql::PostgreSql;
pub use sqlite::Sqlite;

/// Selects the database: `dato <server> [OPTIONS] db NAME`.
#[derive(Debug, Clone, PartialEq, Eq, Subcommand)]
pub enum DbCmd {
    /// Database to use.
    Db {
        /// Database name.
        name: String,
    },
}

impl DbCmd {
    pub fn name(&self) -> String {
        let DbCmd::Db { name } = self;
        name.clone()
    }
}

/// A server's command-line arguments.
pub trait ServerCmd: Parser {
    const SERVER: Server;

    /// Connection options for these arguments. A password is never part of
    /// them; it is asked for separately, see [`ask_password`].
    fn options(&self) -> ConnectOptions;
}

impl Server {
    /// Parses `args` (without the server name) with that server's parser. The
    /// error is clap's rendered message, help output included.
    pub fn parse_args(&self, args: &[String]) -> Result<ConnectOptions, clap::Error> {
        let argv = std::iter::once(self.name().to_string()).chain(args.iter().cloned());
        match self {
            Server::Sqlite => Sqlite::try_parse_from(argv).map(|a| a.options()),
            Server::MySql => MySql::try_parse_from(argv).map(|a| a.options()),
            Server::PostgreSql => PostgreSql::try_parse_from(argv).map(|a| a.options()),
        }
    }
}

/// Asks for the password on the terminal with echo off when the options
/// require one and none is set yet.
pub fn ask_password(opts: &mut ConnectOptions) -> std::io::Result<()> {
    if let Some(prompt) = opts.needs_password() {
        opts.set_password(rpassword::prompt_password(prompt)?);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::connect::split_words;

    #[test]
    fn sqlite_args() {
        assert_eq!(
            Server::Sqlite
                .parse_args(&split_words("app.db").unwrap())
                .unwrap(),
            ConnectOptions::sqlite(Some("app.db"))
        );
        assert_eq!(
            Server::Sqlite.parse_args(&[]).unwrap(),
            ConnectOptions::sqlite(None)
        );
    }

    #[test]
    fn connect_args_match_subcommands() {
        let argv = ["dato", "mysql", "-h", "h", "-u", "u", "-p", "db", "d"];
        let cli = crate::cli::Args::try_parse_from(argv).unwrap();
        let from_cli = crate::cli::startup_options(&cli).unwrap().unwrap();
        let from_connect = Server::MySql
            .parse_args(&split_words("-h h -u u -p db d").unwrap())
            .unwrap();
        assert_eq!(from_cli, from_connect);
    }
}

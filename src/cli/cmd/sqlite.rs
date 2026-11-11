//! `dato sqlite [FILE]`.

use clap::Parser;

use super::ServerCmd;
use crate::client::ConnectOptions;
use crate::server::Server;

#[derive(Debug, Clone, PartialEq, Eq, Parser)]
#[command(name = "sqlite", about = "Connect to a SQLite database file")]
pub struct Sqlite {
    /// Database file, created if missing. `:memory:` or no file opens an
    /// in-memory database.
    #[arg(value_name = "FILE")]
    pub file: Option<String>,
}

impl ServerCmd for Sqlite {
    const SERVER: Server = Server::Sqlite;

    fn options(&self) -> ConnectOptions {
        ConnectOptions::sqlite(self.file.as_deref())
    }
}

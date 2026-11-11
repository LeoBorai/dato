//! Database servers a session can connect to.
//!
//! `Server` is the single list of supported servers. Each layer (connection,
//! dialect, connect form, ...) extends it with its own trait implemented as a
//! `match` on the variants, so adding a server means adding a variant here and
//! fixing the resulting compile errors in every layer.

use std::fmt;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Server {
    Sqlite,
    MySql,
}

impl Server {
    /// Every supported server, in the order they are offered to the user.
    pub const ALL: &'static [Server] = &[Server::Sqlite, Server::MySql];

    /// Name used by CLI subcommands, `\connect` and the connect wizard.
    pub fn name(self) -> &'static str {
        match self {
            Server::Sqlite => "sqlite",
            Server::MySql => "mysql",
        }
    }

    /// Parses a server name, ignoring case.
    pub fn parse(name: &str) -> Option<Server> {
        Server::ALL
            .iter()
            .copied()
            .find(|s| s.name().eq_ignore_ascii_case(name))
    }

    /// Comma-separated list of server names, for error messages.
    pub fn names() -> String {
        Server::ALL
            .iter()
            .map(|s| s.name())
            .collect::<Vec<_>>()
            .join(", ")
    }
}

impl fmt::Display for Server {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.name())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_round_trip() {
        for s in Server::ALL {
            assert_eq!(Server::parse(s.name()), Some(*s));
        }
        assert_eq!(Server::parse("mysql"), Some(Server::MySql));
        assert_eq!(Server::parse("SQLite"), Some(Server::Sqlite));
        assert_eq!(Server::parse("postgres"), None);
        assert_eq!(Server::names(), "sqlite, mysql");
    }
}

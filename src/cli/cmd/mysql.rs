//! `dato mysql [-h HOST] [-P PORT] [-u USER] [-p] [-S SOCKET] [-D DB | DB]`.
//!
//! Login arguments follow the `mysql` client.

use clap::{ArgAction, Parser};

use super::ServerCmd;
use crate::client::{ConnectOptions, MySqlOptions, default_user};
use crate::server::Server;

#[derive(Debug, Clone, PartialEq, Eq, Parser)]
#[command(
    name = "mysql",
    about = "Connect to a MySQL server",
    disable_help_flag = true
)]
pub struct MySql {
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

impl ServerCmd for MySql {
    const SERVER: Server = Server::MySql;

    fn options(&self) -> ConnectOptions {
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

#[cfg(test)]
mod tests {
    use super::*;

    fn words(s: &str) -> Vec<String> {
        crate::connect::split_words(s).unwrap()
    }

    fn mysql(args: &str) -> Result<MySqlOptions, clap::Error> {
        match Server::MySql.parse_args(&words(args))? {
            ConnectOptions::MySql(o) => Ok(o),
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn login_args() {
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
    fn defaults() {
        let o = mysql("").unwrap();
        assert_eq!((o.host.as_str(), o.port), ("localhost", 3306));
        assert_eq!(o.user, default_user());
        assert_eq!(o.database, None);
    }

    #[test]
    fn rejects_attached_password_and_lists_host_in_help() {
        assert!(mysql("-u root -psecret").is_err());
        let help = mysql("--help").unwrap_err();
        assert_eq!(help.kind(), clap::error::ErrorKind::DisplayHelp);
        assert!(help.to_string().contains("-h, --host"), "{help}");
        assert!(mysql("-D a b").is_err());
    }

    #[test]
    fn port_flag_is_only_for_mysql() {
        assert!(mysql("-P 3307").is_ok());
        assert!(mysql("-U app").is_err());
    }
}

//! `dato psql [-h HOST] [-p PORT] -U USER [-W] [--sslmode MODE] [db NAME]`.
//!
//! Login arguments follow the `psql` client.

use clap::{ArgAction, Parser};

use crate::client::psql::SslMode;
use crate::client::{ConnectOptions, PsqlOptions};
use crate::server::Server;

use super::ServerCmd;

#[derive(Debug, Clone, PartialEq, Eq, Parser)]
#[command(
    name = "psql",
    about = "Connect to a PostgreSQL server",
    disable_help_flag = true
)]
pub struct PostgreSql {
    /// Server host, or the directory of a Unix socket.
    #[arg(short = 'h', long, default_value = "localhost")]
    pub host: String,

    /// TCP port.
    #[arg(short = 'p', long, default_value_t = 5432)]
    pub port: u16,

    /// User name.
    #[arg(short = 'U', long, required = true)]
    pub username: String,

    /// Ask for the password (input is not echoed).
    #[arg(short = 'W', long)]
    pub password: bool,

    #[arg(short = 'd', long = "dbname")]
    pub db: Option<String>,

    /// TLS mode: disable, prefer, require or verify-full.
    #[arg(long, value_name = "MODE", default_value = "prefer")]
    pub sslmode: SslMode,

    /// Certificate file to trust with `--sslmode verify-full`, instead of the
    /// system roots.
    #[arg(long, value_name = "FILE")]
    pub sslrootcert: Option<String>,

    /// Print help.
    #[arg(long, action = ArgAction::Help)]
    help: Option<bool>,
}

impl ServerCmd for PostgreSql {
    const SERVER: Server = Server::PostgreSql;

    fn options(&self) -> ConnectOptions {
        ConnectOptions::Psql(PsqlOptions {
            host: self.host.clone(),
            port: self.port,
            user: self.username.clone(),
            password: None,
            password_required: self.password,
            dbname: self.db.to_owned(),
            sslmode: self.sslmode,
            sslrootcert: self.sslrootcert.clone(),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn words(s: &str) -> Vec<String> {
        crate::connect::split_words(s).unwrap()
    }

    fn psql(args: &str) -> Result<PsqlOptions, clap::Error> {
        match Server::PostgreSql.parse_args(&words(args))? {
            ConnectOptions::Psql(o) => Ok(o),
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn login_args() {
        let o = psql("-h db.local -p 5433 -U app -W db shop").unwrap();
        assert_eq!(o.host, "db.local");
        assert_eq!(o.port, 5433);
        assert_eq!(o.user, "app");
        assert!(o.password_required);
        assert_eq!(o.password, None);
        assert_eq!(o.dbname.as_deref(), Some("shop"));
        assert_eq!(o.sslmode, SslMode::Prefer);

        let o = psql("--host x --username root --sslmode verify-full --sslrootcert ca.pem db crm")
            .unwrap();
        assert_eq!(o.dbname.as_deref(), Some("crm"));
        assert_eq!(o.sslmode, SslMode::VerifyFull);
        assert_eq!(o.sslrootcert.as_deref(), Some("ca.pem"));
        assert!(!o.password_required);
    }

    #[test]
    fn defaults() {
        let o = psql("-U app").unwrap();
        assert_eq!((o.host.as_str(), o.port), ("localhost", 5432));
        assert_eq!(o.dbname, None);
    }

    #[test]
    fn rejects_other_servers_flags_and_attached_password() {
        assert!(psql("-U app -P 5433").is_err());
        assert!(psql("-U app -S /tmp/s.sock").is_err());
        assert!(psql("-U postgres -Wsecret").is_err());
        assert!(psql("-d a").is_err());
        assert!(psql("shop").is_err());
    }

    #[test]
    fn invalid_sslmode_lists_valid_modes() {
        let err = psql("-U app --sslmode bogus").unwrap_err().to_string();
        assert!(
            err.contains("disable, prefer, require, verify-full"),
            "{err}"
        );
    }

    #[test]
    fn help_lists_only_psql_flags() {
        let help = psql("--help").unwrap_err();
        assert_eq!(help.kind(), clap::error::ErrorKind::DisplayHelp);
        let text = help.to_string();
        for flag in [
            "-h, --host",
            "-p, --port",
            "-U, --username",
            "db",
            "-W, --password",
            "--sslmode",
        ] {
            assert!(text.contains(flag), "{flag} missing: {text}");
        }
        assert!(
            !text.contains("--socket") && !text.contains("-S,"),
            "{text}"
        );
    }
}

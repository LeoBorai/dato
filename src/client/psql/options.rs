//! PostgreSQL login parameters.

use std::fmt;
use std::str::FromStr;

use crate::client::default_user;

/// How the connection uses TLS, named as libpq's `sslmode`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum SslMode {
    /// Never use TLS.
    Disable,
    /// Use TLS when the server offers it, else connect unencrypted.
    #[default]
    Prefer,
    /// Require TLS; the certificate is not checked.
    Require,
    /// Require TLS with a certificate that chains to a trusted root and
    /// matches the host name.
    VerifyFull,
}

impl SslMode {
    pub const ALL: &'static [SslMode] = &[
        SslMode::Disable,
        SslMode::Prefer,
        SslMode::Require,
        SslMode::VerifyFull,
    ];

    pub fn name(self) -> &'static str {
        match self {
            SslMode::Disable => "disable",
            SslMode::Prefer => "prefer",
            SslMode::Require => "require",
            SslMode::VerifyFull => "verify-full",
        }
    }

    /// Mode names in `ALL` order, for the connect wizard's choice.
    pub const NAMES: &'static [&'static str] = &["disable", "prefer", "require", "verify-full"];

    /// Comma-separated mode names, for error messages.
    pub fn names() -> String {
        SslMode::ALL
            .iter()
            .map(|m| m.name())
            .collect::<Vec<_>>()
            .join(", ")
    }
}

impl fmt::Display for SslMode {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.name())
    }
}

impl FromStr for SslMode {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        SslMode::ALL
            .iter()
            .copied()
            .find(|m| m.name().eq_ignore_ascii_case(s.trim()))
            .ok_or_else(|| format!("invalid TLS mode: {s} (valid: {})", SslMode::names()))
    }
}

/// From `dato psql`, `\connect psql` or the wizard.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PsqlOptions {
    /// A host name, or a directory for a Unix socket.
    pub host: String,
    pub port: u16,
    pub user: String,
    pub password: Option<String>,
    /// `-W` was given: ask for the password before connecting.
    pub password_required: bool,
    /// Defaults to the user name when absent, as `psql` does.
    pub dbname: Option<String>,
    pub sslmode: SslMode,
    /// Certificate file trusted by `verify-full`, instead of the system roots.
    pub sslrootcert: Option<String>,
}

impl Default for PsqlOptions {
    fn default() -> Self {
        Self {
            host: "localhost".into(),
            port: 5432,
            user: default_user(),
            password: None,
            password_required: false,
            dbname: None,
            sslmode: SslMode::default(),
            sslrootcert: None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mode_names_round_trip() {
        for m in SslMode::ALL {
            assert_eq!(m.name().parse::<SslMode>(), Ok(*m));
        }
        assert_eq!("VERIFY-FULL".parse(), Ok(SslMode::VerifyFull));
        let err = "bogus".parse::<SslMode>().unwrap_err();
        assert!(
            err.contains("disable, prefer, require, verify-full"),
            "{err}"
        );
    }

    #[test]
    fn names_match_all() {
        let all: Vec<_> = SslMode::ALL.iter().map(|m| m.name()).collect();
        assert_eq!(all, SslMode::NAMES);
    }

    #[test]
    fn defaults() {
        let o = PsqlOptions::default();
        assert_eq!((o.host.as_str(), o.port), ("localhost", 5432));
        assert_eq!(o.sslmode, SslMode::Prefer);
        assert_eq!(o.dbname, None);
    }
}

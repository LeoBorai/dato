//! The connect-form layer of [`Server`]: which fields the wizard asks for and
//! how their values become [`ConnectOptions`].

use crate::client::psql::SslMode;
use crate::client::{ConnectOptions, MySqlOptions, PsqlOptions};
use crate::server::Server;

/// One field of a server's connect form.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Field {
    pub key: &'static str,
    pub label: &'static str,
    /// Pre-filled value.
    pub default: String,
    /// Shown while empty.
    pub placeholder: &'static str,
    /// Masked input.
    pub secret: bool,
    /// When not empty, the field is one of these values, picked with the arrow
    /// keys instead of typed; `default` is the one picked first.
    pub choices: &'static [&'static str],
    /// A file path: typing `@` opens a menu of files to pick from.
    pub path: bool,
}

impl Field {
    pub(crate) fn text(key: &'static str, label: &'static str, default: impl Into<String>) -> Self {
        Self {
            key,
            label,
            default: default.into(),
            placeholder: "",
            secret: false,
            choices: &[],
            path: false,
        }
    }

    pub(crate) fn choice(
        key: &'static str,
        label: &'static str,
        choices: &'static [&'static str],
        default: &'static str,
    ) -> Self {
        Self {
            choices,
            ..Self::text(key, label, default)
        }
    }
}

/// Entered values by field key.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct FieldValues(pub Vec<(&'static str, String)>);

impl FieldValues {
    pub fn get(&self, key: &str) -> &str {
        self.0
            .iter()
            .find(|(k, _)| *k == key)
            .map_or("", |(_, v)| v.as_str())
    }

    fn optional(&self, key: &str) -> Option<String> {
        let v = self.get(key).trim();
        (!v.is_empty()).then(|| v.to_string())
    }
}

/// A form validation error: the field it belongs to and the message.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FieldError {
    pub key: &'static str,
    pub message: String,
}

pub trait ConnectForm {
    fn fields(&self) -> Vec<Field>;
    fn build(&self, values: &FieldValues) -> Result<ConnectOptions, FieldError>;
}

impl ConnectForm for Server {
    fn fields(&self) -> Vec<Field> {
        match self {
            Server::Sqlite => vec![Field {
                placeholder: ":memory:",
                path: true,
                ..Field::text("file", "Database file", "")
            }],
            Server::MySql => vec![
                Field::text("host", "Host", "localhost"),
                Field::text("port", "Port", "3306"),
                Field::text("user", "User", ""),
                Field {
                    secret: true,
                    ..Field::text("password", "Password", "")
                },
                Field::text("database", "Database", ""),
            ],
            Server::PostgreSql => vec![
                Field::text("host", "Host", "localhost"),
                Field::text("port", "Port", "5432"),
                Field::text("user", "User", ""),
                Field {
                    secret: true,
                    ..Field::text("password", "Password", "")
                },
                Field::text("database", "Database", ""),
                // No TLS unless asked for; the CLI keeps libpq's `prefer`.
                Field::choice("sslmode", "TLS mode", SslMode::NAMES, "disable"),
            ],
        }
    }

    fn build(&self, v: &FieldValues) -> Result<ConnectOptions, FieldError> {
        match self {
            Server::Sqlite => Ok(ConnectOptions::sqlite(v.optional("file").as_deref())),
            Server::MySql => {
                let port = v.get("port").trim();
                let port = port.parse::<u16>().map_err(|_| FieldError {
                    key: "port",
                    message: format!("not a port number: {port}"),
                })?;
                let defaults = MySqlOptions::default();
                Ok(ConnectOptions::MySql(MySqlOptions {
                    host: v.optional("host").unwrap_or(defaults.host),
                    port,
                    user: v.optional("user").ok_or_else(|| FieldError {
                        key: "user",
                        message: "user is required".into(),
                    })?,
                    // Entered in the form, so never asked for again.
                    password: Some(v.get("password").to_string()).filter(|p| !p.is_empty()),
                    password_required: false,
                    socket: None,
                    database: v.optional("database"),
                }))
            }
            Server::PostgreSql => {
                let port = v.get("port").trim();
                let port = port.parse::<u16>().map_err(|_| FieldError {
                    key: "port",
                    message: format!("not a port number: {port}"),
                })?;
                let sslmode = match v.optional("sslmode") {
                    Some(m) => m.parse::<SslMode>().map_err(|message| FieldError {
                        key: "sslmode",
                        message,
                    })?,
                    None => SslMode::Disable,
                };
                let defaults = PsqlOptions::default();
                Ok(ConnectOptions::Psql(PsqlOptions {
                    host: v.optional("host").unwrap_or(defaults.host),
                    port,
                    user: v.optional("user").ok_or_else(|| FieldError {
                        key: "user",
                        message: "user is required".into(),
                    })?,
                    // Entered in the form, so never asked for again.
                    password: Some(v.get("password").to_string()).filter(|p| !p.is_empty()),
                    password_required: false,
                    dbname: v.optional("database"),
                    sslmode,
                    sslrootcert: None,
                }))
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn values(pairs: &[(&'static str, &str)]) -> FieldValues {
        FieldValues(pairs.iter().map(|(k, v)| (*k, v.to_string())).collect())
    }

    #[test]
    fn mysql_fields_and_defaults() {
        let f = Server::MySql.fields();
        let keys: Vec<_> = f.iter().map(|f| f.key).collect();
        assert_eq!(keys, ["host", "port", "user", "password", "database"]);
        assert_eq!(f[0].default, "localhost");
        assert_eq!(f[1].default, "3306");
        assert!(f[3].secret);
        assert!(!f[0].secret);
    }

    #[test]
    fn mysql_build() {
        let o = Server::MySql
            .build(&values(&[
                ("host", "db.local"),
                ("port", "3307"),
                ("user", "app"),
                ("password", "secret"),
                ("database", "shop"),
            ]))
            .unwrap();
        assert_eq!(
            o,
            ConnectOptions::MySql(MySqlOptions {
                host: "db.local".into(),
                port: 3307,
                user: "app".into(),
                password: Some("secret".into()),
                password_required: false,
                socket: None,
                database: Some("shop".into()),
            })
        );
        let err = Server::MySql
            .build(&values(&[("port", "nope")]))
            .unwrap_err();
        assert_eq!(err.key, "port");
    }

    #[test]
    fn sqlite_build() {
        assert_eq!(Server::Sqlite.fields()[0].placeholder, ":memory:");
        assert_eq!(
            Server::Sqlite
                .build(&values(&[("file", "app.db")]))
                .unwrap(),
            ConnectOptions::sqlite(Some("app.db"))
        );
        assert_eq!(
            Server::Sqlite.build(&values(&[("file", "")])).unwrap(),
            ConnectOptions::sqlite(None)
        );
    }

    #[test]
    fn psql_fields_and_defaults() {
        let f = Server::PostgreSql.fields();
        let keys: Vec<_> = f.iter().map(|f| f.key).collect();
        assert_eq!(
            keys,
            ["host", "port", "user", "password", "database", "sslmode"]
        );
        assert_eq!(f[0].default, "localhost");
        assert_eq!(f[1].default, "5432");
        assert_eq!(f[5].default, "disable");
        assert_eq!(f[5].choices, SslMode::NAMES);
        assert!(f[3].secret && !f[5].secret && f[0].choices.is_empty());
    }

    #[test]
    fn psql_build() {
        let o = Server::PostgreSql
            .build(&values(&[
                ("host", "db.local"),
                ("port", "5433"),
                ("user", "app"),
                ("password", "secret"),
                ("database", "shop"),
                ("sslmode", "require"),
            ]))
            .unwrap();
        assert_eq!(
            o,
            ConnectOptions::Psql(PsqlOptions {
                host: "db.local".into(),
                port: 5433,
                user: "app".into(),
                password: Some("secret".into()),
                password_required: false,
                dbname: Some("shop".into()),
                sslmode: SslMode::Require,
                sslrootcert: None,
            })
        );
        // No TLS unless picked.
        let o = Server::PostgreSql
            .build(&values(&[("user", "app"), ("port", "5432")]))
            .unwrap();
        assert!(matches!(o, ConnectOptions::Psql(p) if p.sslmode == SslMode::Disable));
        let err = Server::PostgreSql
            .build(&values(&[("port", "nope")]))
            .unwrap_err();
        assert_eq!(err.key, "port");
        let err = Server::PostgreSql
            .build(&values(&[("port", "5432"), ("sslmode", "bogus")]))
            .unwrap_err();
        assert_eq!(err.key, "sslmode");
        assert!(err.message.contains("verify-full"), "{}", err.message);
    }
}

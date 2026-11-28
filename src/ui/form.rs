//! The connect-form layer of [`Server`]: which fields the wizard asks for and
//! how their values become [`ConnectOptions`].

use crate::auth::vault::{self, SecretType, VaultConfig};
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
    /// When set, the field is only asked for while the field with this key is
    /// asked for too and holds one of these values.
    pub when: Option<(&'static str, &'static [&'static str])>,
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
            when: None,
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

    /// Asks for the field only while the field `key` holds one of `values`.
    pub(crate) fn when(self, key: &'static str, values: &'static [&'static str]) -> Self {
        Self {
            when: Some((key, values)),
            ..self
        }
    }
}

/// Whether `specs[i]` is asked for, given the values entered so far: its
/// condition holds and the field the condition reads is asked for too.
pub fn is_visible(specs: &[Field], values: &dyn Fn(usize) -> String, i: usize) -> bool {
    let Some((key, allowed)) = specs[i].when else {
        return true;
    };
    match specs.iter().position(|s| s.key == key) {
        Some(j) if j != i => is_visible(specs, values, j) && allowed.contains(&values(j).as_str()),
        _ => false,
    }
}

/// Values of the "Authentication" field.
pub const AUTH_PASSWORD: &str = "password";
pub const AUTH_VAULT: &str = "vault";
const AUTH_METHODS: &[&str] = &[AUTH_PASSWORD, AUTH_VAULT];
const KV_TYPES: &[&str] = &["kv1", "kv2"];

/// The "Authentication" choice and the fields of each method: a user and
/// password, or where to find them in Vault.
fn auth_fields() -> Vec<Field> {
    const PW: &[&str] = &[AUTH_PASSWORD];
    const VAULT: &[&str] = &[AUTH_VAULT];
    vec![
        Field::choice("auth", "Authentication", AUTH_METHODS, AUTH_PASSWORD),
        Field::text("user", "User", "").when("auth", PW),
        Field {
            secret: true,
            ..Field::text("password", "Password", "")
        }
        .when("auth", PW),
        Field {
            placeholder: "database/creds/my-role",
            ..Field::text("vault_secret", "Vault secret", "")
        }
        .when("auth", VAULT),
        Field {
            placeholder: "$VAULT_ADDR",
            ..Field::text("vault_address", "Vault address", "")
        }
        .when("auth", VAULT),
        Field {
            path: true,
            ..Field::text("vault_token_file", "Token file", vault::DEFAULT_TOKEN_FILE)
        }
        .when("auth", VAULT),
        Field {
            path: true,
            placeholder: "$VAULT_CACERT",
            ..Field::text("vault_ca_cert", "SSL certificate", "")
        }
        .when("auth", VAULT),
        Field {
            placeholder: "$VAULT_NAMESPACE",
            ..Field::text("vault_namespace", "Namespace", "")
        }
        .when("auth", VAULT),
        Field::choice(
            "vault_secret_type",
            "Secret type",
            SecretType::NAMES,
            SecretType::DynamicRole.name(),
        )
        .when("auth", VAULT),
        Field::text(
            "vault_username_key",
            "Username key",
            vault::DEFAULT_USERNAME_KEY,
        )
        .when("vault_secret_type", KV_TYPES),
        Field::text(
            "vault_password_key",
            "Password key",
            vault::DEFAULT_PASSWORD_KEY,
        )
        .when("vault_secret_type", KV_TYPES),
    ]
}

/// User, password and Vault settings from the auth fields. With Vault, user
/// and password are empty until Vault provides them when connecting.
fn build_auth(
    v: &FieldValues,
) -> Result<(String, Option<String>, Option<VaultConfig>), FieldError> {
    if v.get("auth") != AUTH_VAULT {
        let user = v.optional("user").ok_or_else(|| FieldError {
            key: "user",
            message: "user is required".into(),
        })?;
        // Entered in the form, so never asked for again.
        let password = Some(v.get("password").to_string()).filter(|p| !p.is_empty());
        return Ok((user, password, None));
    }
    let secret = v.optional("vault_secret").ok_or_else(|| FieldError {
        key: "vault_secret",
        message: "Vault secret is required".into(),
    })?;
    let secret_type = match v.optional("vault_secret_type") {
        Some(t) => t.parse::<SecretType>().map_err(|message| FieldError {
            key: "vault_secret_type",
            message,
        })?,
        None => SecretType::default(),
    };
    let key = |k: &'static str, default: &str| -> Result<String, FieldError> {
        if !secret_type.uses_keys() {
            return Ok(default.into());
        }
        v.optional(k).ok_or_else(|| FieldError {
            key: k,
            message: "key is required for KV secrets".into(),
        })
    };
    let vault = VaultConfig {
        secret,
        address: v.optional("vault_address"),
        token_file: v.optional("vault_token_file"),
        ca_cert: v.optional("vault_ca_cert"),
        namespace: v.optional("vault_namespace"),
        secret_type,
        username_key: key("vault_username_key", vault::DEFAULT_USERNAME_KEY)?,
        password_key: key("vault_password_key", vault::DEFAULT_PASSWORD_KEY)?,
    };
    Ok((String::new(), None, Some(vault)))
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

/// The form values that describe `opts`: the reverse of [`ConnectForm::build`].
/// The password is left out, so it is never put back into the form.
pub fn form_values(opts: &ConnectOptions) -> FieldValues {
    let text = |v: &Option<String>| v.clone().unwrap_or_default();
    let mut out: Vec<(&'static str, String)> = Vec::new();
    let vault = match opts {
        ConnectOptions::Sqlite { path } => {
            out.push(("file", text(path)));
            None
        }
        ConnectOptions::MySql(o) => {
            out.extend([
                ("host", o.host.clone()),
                ("port", o.port.to_string()),
                ("user", o.user.clone()),
                ("database", text(&o.database)),
            ]);
            o.vault.as_ref()
        }
        ConnectOptions::Psql(o) => {
            out.extend([
                ("host", o.host.clone()),
                ("port", o.port.to_string()),
                ("user", o.user.clone()),
                ("database", text(&o.dbname)),
                ("sslmode", o.sslmode.name().to_string()),
            ]);
            o.vault.as_ref()
        }
    };
    if let Some(v) = vault {
        out.extend([
            ("auth", AUTH_VAULT.to_string()),
            ("vault_secret", v.secret.clone()),
            ("vault_address", text(&v.address)),
            ("vault_token_file", text(&v.token_file)),
            ("vault_ca_cert", text(&v.ca_cert)),
            ("vault_namespace", text(&v.namespace)),
            ("vault_secret_type", v.secret_type.name().to_string()),
            ("vault_username_key", v.username_key.clone()),
            ("vault_password_key", v.password_key.clone()),
        ]);
    }
    FieldValues(out)
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
            Server::MySql => [
                Field::text("host", "Host", "localhost"),
                Field::text("port", "Port", "3306"),
            ]
            .into_iter()
            .chain(auth_fields())
            .chain([Field::text("database", "Database", "")])
            .collect(),
            Server::PostgreSql => [
                Field::text("host", "Host", "localhost"),
                Field::text("port", "Port", "5432"),
            ]
            .into_iter()
            .chain(auth_fields())
            .chain([
                Field::text("database", "Database", ""),
                // No TLS unless asked for; the CLI keeps libpq's `prefer`.
                Field::choice("sslmode", "TLS mode", SslMode::NAMES, "disable"),
            ])
            .collect(),
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
                let (user, password, vault) = build_auth(v)?;
                let defaults = MySqlOptions::default();
                Ok(ConnectOptions::MySql(MySqlOptions {
                    host: v.optional("host").unwrap_or(defaults.host),
                    port,
                    user,
                    password,
                    password_required: false,
                    socket: None,
                    database: v.optional("database"),
                    vault,
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
                let (user, password, vault) = build_auth(v)?;
                let defaults = PsqlOptions::default();
                Ok(ConnectOptions::Psql(PsqlOptions {
                    host: v.optional("host").unwrap_or(defaults.host),
                    port,
                    user,
                    password,
                    password_required: false,
                    dbname: v.optional("database"),
                    sslmode,
                    sslrootcert: None,
                    vault,
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
        assert_eq!(keys[..5], ["host", "port", "auth", "user", "password"]);
        assert_eq!(keys.last(), Some(&"database"));
        assert_eq!(f[0].default, "localhost");
        assert_eq!(f[1].default, "3306");
        assert_eq!(f[2].default, "password");
        assert!(f[4].secret);
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
                vault: None,
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
        assert_eq!(keys[..5], ["host", "port", "auth", "user", "password"]);
        assert_eq!(keys[keys.len() - 2..], ["database", "sslmode"]);
        let ssl = f.last().unwrap();
        assert_eq!(f[0].default, "localhost");
        assert_eq!(f[1].default, "5432");
        assert_eq!(ssl.default, "disable");
        assert_eq!(ssl.choices, SslMode::NAMES);
        assert!(f[4].secret && !ssl.secret && f[0].choices.is_empty());
    }

    fn visible_keys(server: Server, picked: &[(&str, &str)]) -> Vec<&'static str> {
        let specs = server.fields();
        let value = |j: usize| {
            let key = specs[j].key;
            picked
                .iter()
                .find(|(k, _)| *k == key)
                .map_or_else(|| specs[j].default.clone(), |(_, v)| v.to_string())
        };
        (0..specs.len())
            .filter(|&i| is_visible(&specs, &value, i))
            .map(|i| specs[i].key)
            .collect()
    }

    #[test]
    fn vault_fields_show_only_with_vault() {
        assert_eq!(
            visible_keys(Server::MySql, &[]),
            ["host", "port", "auth", "user", "password", "database"]
        );
        assert_eq!(
            visible_keys(Server::PostgreSql, &[("auth", "vault")]),
            [
                "host",
                "port",
                "auth",
                "vault_secret",
                "vault_address",
                "vault_token_file",
                "vault_ca_cert",
                "vault_namespace",
                "vault_secret_type",
                "database",
                "sslmode"
            ]
        );
        let kv = visible_keys(
            Server::PostgreSql,
            &[("auth", "vault"), ("vault_secret_type", "kv2")],
        );
        assert!(kv.contains(&"vault_username_key") && kv.contains(&"vault_password_key"));
        // Keys need Vault even when a KV type is the hidden choice's value.
        let pw = visible_keys(Server::MySql, &[("vault_secret_type", "kv1")]);
        assert!(!pw.contains(&"vault_username_key"), "{pw:?}");
        let token = Server::MySql
            .fields()
            .into_iter()
            .find(|f| f.key == "vault_token_file")
            .unwrap();
        assert_eq!(token.default, "~/.dato/.vault-token");
        assert!(token.path);
    }

    #[test]
    fn vault_build() {
        let o = Server::PostgreSql
            .build(&values(&[
                ("port", "5432"),
                ("auth", "vault"),
                ("vault_secret", "database/creds/app"),
                ("vault_address", "https://vault:8200"),
                ("vault_token_file", "~/.dato/.vault-token"),
                ("vault_secret_type", "dynamic-role"),
                ("database", "shop"),
            ]))
            .unwrap();
        let ConnectOptions::Psql(p) = o else {
            unreachable!()
        };
        assert_eq!((p.user.as_str(), p.password.as_deref()), ("", None));
        assert_eq!(
            p.vault,
            Some(VaultConfig {
                secret: "database/creds/app".into(),
                address: Some("https://vault:8200".into()),
                token_file: Some("~/.dato/.vault-token".into()),
                ca_cert: None,
                namespace: None,
                secret_type: SecretType::DynamicRole,
                username_key: "username".into(),
                password_key: "password".into(),
            })
        );

        let o = Server::MySql
            .build(&values(&[
                ("port", "3306"),
                ("auth", "vault"),
                ("vault_secret", "kv/data/shop"),
                ("vault_namespace", "team"),
                ("vault_secret_type", "kv2"),
                ("vault_username_key", "user"),
                ("vault_password_key", "pass"),
            ]))
            .unwrap();
        let ConnectOptions::MySql(m) = o else {
            unreachable!()
        };
        let v = m.vault.unwrap();
        assert_eq!(v.secret_type, SecretType::Kv2);
        assert_eq!(
            (v.username_key.as_str(), v.password_key.as_str()),
            ("user", "pass")
        );
        assert_eq!(v.namespace.as_deref(), Some("team"));

        let err = Server::MySql
            .build(&values(&[("port", "3306"), ("auth", "vault")]))
            .unwrap_err();
        assert_eq!(err.key, "vault_secret");
        let err = Server::MySql
            .build(&values(&[
                ("port", "3306"),
                ("auth", "vault"),
                ("vault_secret", "kv/shop"),
                ("vault_secret_type", "kv1"),
            ]))
            .unwrap_err();
        assert_eq!(err.key, "vault_username_key");
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
                vault: None,
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

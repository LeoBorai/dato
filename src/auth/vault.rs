//! Database credentials from [HashiCorp Vault](https://developer.hashicorp.com/vault).
//!
//! The user logs in to Vault outside of dato (e.g. `vault login`), which leaves
//! a token on disk. When connecting, dato reads that token, asks Vault for the
//! secret and uses the user name and password found in it. Supported secrets:
//!
//! - Database secrets engine: dynamic roles (`<mount>/creds/<role>`) and static
//!   roles (`<mount>/static-creds/<role>`); credentials are in `data`.
//! - KV secrets engine version 1 (`<mount>/<path>`): credentials are in `data`,
//!   under the configured keys.
//! - KV secrets engine version 2 (`<mount>/data/<path>`): credentials are in
//!   `data.data`, under the configured keys.
//!
//! Settings left empty fall back to Vault's usual environment variables:
//! `VAULT_AGENT_ADDR` then `VAULT_ADDR` for the address, `VAULT_CACERT` for the
//! certificate and `VAULT_NAMESPACE` for the namespace. The token is read from
//! the token file (default `~/.dato/.vault-token`), else `VAULT_TOKEN`, else
//! `~/.vault-token`, where the `vault` CLI keeps it.

use std::fmt;
use std::path::{Path, PathBuf};
use std::str::FromStr;
use std::sync::Arc;
use std::time::Duration;

use serde_json::Value;
use ureq::tls::{Certificate, PemItem, RootCerts, TlsConfig};

/// Name of the token file inside `~/.dato`.
pub const TOKEN_FILE_NAME: &str = ".vault-token";

/// Default token file as shown to the user.
pub const DEFAULT_TOKEN_FILE: &str = "~/.dato/.vault-token";

/// Key of the user name in a secret, unless configured otherwise.
pub const DEFAULT_USERNAME_KEY: &str = "username";

/// Key of the password in a secret, unless configured otherwise.
pub const DEFAULT_PASSWORD_KEY: &str = "password";

const ENV_AGENT_ADDR: &str = "VAULT_AGENT_ADDR";
const ENV_ADDR: &str = "VAULT_ADDR";
const ENV_CACERT: &str = "VAULT_CACERT";
const ENV_NAMESPACE: &str = "VAULT_NAMESPACE";
const ENV_TOKEN: &str = "VAULT_TOKEN";

const HEADER_TOKEN: &str = "X-Vault-Token";
const HEADER_NAMESPACE: &str = "X-Vault-Namespace";

/// How long a request to Vault may take, connecting included.
const TIMEOUT: Duration = Duration::from_secs(10);

#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum VaultError {
    #[error("Vault secret not defined")]
    NoSecret,
    #[error("Vault address not defined: set it or VAULT_ADDR")]
    NoAddress,
    #[error("Vault token not found: log in with `vault login` or set the token file")]
    NoToken,
    #[error("cannot read Vault token file {path}: {message}")]
    TokenFile { path: String, message: String },
    #[error("cannot read Vault certificate {path}: {message}")]
    Certificate { path: String, message: String },
    #[error("cannot reach Vault at {address}: {message}")]
    Request { address: String, message: String },
    #[error("Vault returned {status} for {secret}: {message}")]
    Status {
        status: u16,
        secret: String,
        message: String,
    },
    #[error("unexpected Vault response: {0}")]
    Response(String),
    #[error("Vault secret {secret} has no string value for key {key}")]
    MissingKey { secret: String, key: String },
}

/// Which secrets engine the secret belongs to, which decides where the
/// credentials are in Vault's response.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub enum SecretType {
    /// Database secrets engine, dynamic role: new credentials on each read.
    #[default]
    DynamicRole,
    /// Database secrets engine, static role: rotated credentials of an existing user.
    StaticRole,
    /// KV secrets engine, version 1.
    Kv1,
    /// KV secrets engine, version 2.
    Kv2,
}

impl SecretType {
    pub const ALL: &'static [SecretType] = &[
        SecretType::DynamicRole,
        SecretType::StaticRole,
        SecretType::Kv1,
        SecretType::Kv2,
    ];

    /// Names in `ALL` order, for the connect wizard's choice.
    pub const NAMES: &'static [&'static str] = &["dynamic-role", "static-role", "kv1", "kv2"];

    pub fn name(self) -> &'static str {
        match self {
            SecretType::DynamicRole => "dynamic-role",
            SecretType::StaticRole => "static-role",
            SecretType::Kv1 => "kv1",
            SecretType::Kv2 => "kv2",
        }
    }

    /// Whether the user name and password keys are configurable. The database
    /// engine always names them `username` and `password`.
    pub fn uses_keys(self) -> bool {
        matches!(self, SecretType::Kv1 | SecretType::Kv2)
    }

    /// Comma-separated names, for error messages.
    pub fn names() -> String {
        Self::NAMES.join(", ")
    }
}

impl fmt::Display for SecretType {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.name())
    }
}

impl FromStr for SecretType {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        SecretType::ALL
            .iter()
            .copied()
            .find(|t| t.name().eq_ignore_ascii_case(s.trim()))
            .ok_or_else(|| {
                format!(
                    "invalid Vault secret type: {s} (valid: {})",
                    SecretType::names()
                )
            })
    }
}

/// Where the credentials are in Vault, as the user configured it. Optional
/// settings left as `None` fall back to the environment when connecting.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VaultConfig {
    /// API path of the secret, without `/v1/` (e.g. `database/creds/app`).
    pub secret: String,
    /// Vault server URL (e.g. `https://vault.example.com:8200`).
    pub address: Option<String>,
    /// File holding the Vault token; `~` is expanded.
    pub token_file: Option<String>,
    /// PEM file with the certificate(s) to trust instead of the system roots.
    pub ca_cert: Option<String>,
    pub namespace: Option<String>,
    pub secret_type: SecretType,
    /// Key of the user name in a KV secret.
    pub username_key: String,
    /// Key of the password in a KV secret.
    pub password_key: String,
}

impl Default for VaultConfig {
    fn default() -> Self {
        Self {
            secret: String::new(),
            address: None,
            token_file: None,
            ca_cert: None,
            namespace: None,
            secret_type: SecretType::default(),
            username_key: DEFAULT_USERNAME_KEY.into(),
            password_key: DEFAULT_PASSWORD_KEY.into(),
        }
    }
}

/// A user name and password read from Vault.
#[derive(Clone, PartialEq, Eq)]
pub struct Credentials {
    pub username: String,
    pub password: String,
}

impl fmt::Debug for Credentials {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Credentials")
            .field("username", &self.username)
            .field("password", &"***")
            .finish()
    }
}

/// Everything needed for one request, with the environment fallbacks applied.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Request {
    url: String,
    address: String,
    secret: String,
    namespace: Option<String>,
    ca_cert: Option<PathBuf>,
}

fn non_blank(v: Option<String>) -> Option<String> {
    v.map(|s| s.trim().to_string()).filter(|s| !s.is_empty())
}

/// Replaces a leading `~` or `~/` with `home`.
fn expand_tilde(path: &str, home: Option<&Path>) -> PathBuf {
    let rest = if path == "~" {
        Some("")
    } else {
        path.strip_prefix("~/")
    };
    match (rest, home) {
        (Some(rest), Some(home)) => home.join(rest),
        _ => PathBuf::from(path),
    }
}

impl VaultConfig {
    /// Applies the environment fallbacks. `env` looks a variable up.
    fn request(&self, env: &dyn Fn(&str) -> Option<String>) -> Result<Request, VaultError> {
        let secret = self.secret.trim().trim_matches('/').to_string();
        if secret.is_empty() {
            return Err(VaultError::NoSecret);
        }
        let address = non_blank(self.address.clone())
            .or_else(|| non_blank(env(ENV_AGENT_ADDR)))
            .or_else(|| non_blank(env(ENV_ADDR)))
            .ok_or(VaultError::NoAddress)?;
        let address = address.trim_end_matches('/').to_string();
        let home = dirs::home_dir();
        Ok(Request {
            url: format!("{address}/v1/{secret}"),
            address,
            secret,
            namespace: non_blank(self.namespace.clone()).or_else(|| non_blank(env(ENV_NAMESPACE))),
            ca_cert: non_blank(self.ca_cert.clone())
                .or_else(|| non_blank(env(ENV_CACERT)))
                .map(|p| expand_tilde(&p, home.as_deref())),
        })
    }

    /// The token: the token file (default `~/.dato/.vault-token`) when it
    /// exists, else `VAULT_TOKEN`, else `~/.vault-token`.
    fn token(
        &self,
        env: &dyn Fn(&str) -> Option<String>,
        home: Option<&Path>,
    ) -> Result<String, VaultError> {
        let configured = match non_blank(self.token_file.clone()) {
            Some(p) => Some(expand_tilde(&p, home)),
            None => home.map(|h| h.join(".dato").join(TOKEN_FILE_NAME)),
        };
        if let Some(path) = configured.filter(|p| p.exists()) {
            return read_token(&path);
        }
        if let Some(token) = non_blank(env(ENV_TOKEN)) {
            return Ok(token);
        }
        match home.map(|h| h.join(TOKEN_FILE_NAME)) {
            Some(path) if path.exists() => read_token(&path),
            _ => Err(VaultError::NoToken),
        }
    }

    /// Asks Vault for the secret and returns the credentials in it.
    pub fn fetch(&self) -> Result<Credentials, VaultError> {
        let env = |k: &str| std::env::var(k).ok();
        let request = self.request(&env)?;
        let token = self.token(&env, dirs::home_dir().as_deref())?;
        let body = get(&request, &token)?;
        let doc: Value = serde_json::from_str(&body)
            .map_err(|e| VaultError::Response(format!("not JSON: {e}")))?;
        self.credentials(&request.secret, &doc)
    }

    /// Picks the credentials out of Vault's response for this secret type.
    fn credentials(&self, secret: &str, doc: &Value) -> Result<Credentials, VaultError> {
        let data = doc
            .get("data")
            .ok_or_else(|| VaultError::Response("no `data` in the response".into()))?;
        let (data, user_key, pass_key) = match self.secret_type {
            SecretType::DynamicRole | SecretType::StaticRole => {
                (data, DEFAULT_USERNAME_KEY, DEFAULT_PASSWORD_KEY)
            }
            SecretType::Kv1 => (data, self.username_key.as_str(), self.password_key.as_str()),
            SecretType::Kv2 => (
                data.get("data").ok_or_else(|| {
                    VaultError::Response(
                        "no `data.data` in the response: is it a KV v2 secret?".into(),
                    )
                })?,
                self.username_key.as_str(),
                self.password_key.as_str(),
            ),
        };
        let field = |key: &str| {
            data.get(key)
                .and_then(Value::as_str)
                .map(str::to_string)
                .ok_or_else(|| VaultError::MissingKey {
                    secret: secret.to_string(),
                    key: key.to_string(),
                })
        };
        Ok(Credentials {
            username: field(user_key)?,
            password: field(pass_key)?,
        })
    }
}

fn read_token(path: &Path) -> Result<String, VaultError> {
    let err = |message: String| VaultError::TokenFile {
        path: path.display().to_string(),
        message,
    };
    if !path.is_file() {
        return Err(err("not a file".into()));
    }
    let text = std::fs::read_to_string(path).map_err(|e| err(e.to_string()))?;
    non_blank(Some(text)).ok_or_else(|| err("the file is empty".into()))
}

/// Roots trusted for HTTPS: the certificates in `ca_cert`, else the system's.
fn root_certs(ca_cert: Option<&Path>) -> Result<RootCerts, VaultError> {
    let Some(path) = ca_cert else {
        let found = rustls_native_certs::load_native_certs();
        return Ok(found
            .certs
            .iter()
            .map(|c| Certificate::from_der(c.as_ref()).to_owned())
            .into());
    };
    let err = |message: String| VaultError::Certificate {
        path: path.display().to_string(),
        message,
    };
    let pem = std::fs::read(path).map_err(|e| err(e.to_string()))?;
    let mut certs = Vec::new();
    for item in ureq::tls::parse_pem(&pem) {
        if let PemItem::Certificate(c) = item.map_err(|e| err(e.to_string()))? {
            certs.push(c);
        }
    }
    if certs.is_empty() {
        return Err(err("no PEM certificate in the file".into()));
    }
    Ok(certs.into())
}

/// `GET`s the secret and returns the response body.
fn get(request: &Request, token: &str) -> Result<String, VaultError> {
    let mut config = ureq::Agent::config_builder()
        .http_status_as_error(false)
        .timeout_global(Some(TIMEOUT));
    if request.url.starts_with("https://") {
        let provider = Arc::new(rustls::crypto::ring::default_provider());
        config = config.tls_config(
            TlsConfig::builder()
                .root_certs(root_certs(request.ca_cert.as_deref())?)
                .unversioned_rustls_crypto_provider(provider)
                .build(),
        );
    }
    let agent = ureq::Agent::new_with_config(config.build());
    let mut call = agent.get(&request.url).header(HEADER_TOKEN, token);
    if let Some(ns) = &request.namespace {
        call = call.header(HEADER_NAMESPACE, ns);
    }
    let unreachable = |e: ureq::Error| VaultError::Request {
        address: request.address.clone(),
        message: e.to_string(),
    };
    let mut response = call.call().map_err(unreachable)?;
    let status = response.status().as_u16();
    let body = response.body_mut().read_to_string().map_err(unreachable)?;
    if status != 200 {
        return Err(VaultError::Status {
            status,
            secret: request.secret.clone(),
            message: error_message(&body),
        });
    }
    Ok(body)
}

/// Vault's `{"errors": [...]}`, joined; the body itself when it is not that.
fn error_message(body: &str) -> String {
    let errors = serde_json::from_str::<Value>(body).ok().and_then(|v| {
        v.get("errors")?.as_array().map(|a| {
            a.iter()
                .filter_map(Value::as_str)
                .collect::<Vec<_>>()
                .join("; ")
        })
    });
    match errors {
        Some(e) if !e.is_empty() => e,
        _ if body.trim().is_empty() => "no details".into(),
        _ => body.trim().to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use std::collections::HashMap;
    use std::io::{BufRead, BufReader, Write};
    use std::net::TcpListener;

    fn env(vars: &[(&str, &str)]) -> impl Fn(&str) -> Option<String> {
        let map: HashMap<String, String> = vars
            .iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect();
        move |k| map.get(k).cloned()
    }

    fn config(secret: &str, secret_type: SecretType) -> VaultConfig {
        VaultConfig {
            secret: secret.into(),
            address: Some("http://vault:8200".into()),
            secret_type,
            ..Default::default()
        }
    }

    #[test]
    fn secret_type_names_round_trip() {
        for t in SecretType::ALL {
            assert_eq!(t.name().parse::<SecretType>(), Ok(*t));
        }
        let names: Vec<_> = SecretType::ALL.iter().map(|t| t.name()).collect();
        assert_eq!(names, SecretType::NAMES);
        assert_eq!("KV2".parse(), Ok(SecretType::Kv2));
        let err = "nope".parse::<SecretType>().unwrap_err();
        assert!(err.contains("dynamic-role, static-role, kv1, kv2"), "{err}");
        assert!(SecretType::Kv1.uses_keys() && !SecretType::StaticRole.uses_keys());
    }

    #[test]
    fn address_falls_back_to_the_environment() {
        let mut c = config("/database/creds/app/", SecretType::DynamicRole);
        let r = c.request(&env(&[("VAULT_ADDR", "http://other")])).unwrap();
        assert_eq!(r.url, "http://vault:8200/v1/database/creds/app");
        assert_eq!(r.secret, "database/creds/app");

        c.address = Some("  ".into());
        let r = c
            .request(&env(&[
                ("VAULT_AGENT_ADDR", "http://agent:8100/"),
                ("VAULT_ADDR", "http://other"),
            ]))
            .unwrap();
        assert_eq!(r.url, "http://agent:8100/v1/database/creds/app");
        let r = c.request(&env(&[("VAULT_ADDR", "https://v")])).unwrap();
        assert_eq!(r.address, "https://v");
        assert_eq!(c.request(&env(&[])), Err(VaultError::NoAddress));

        c.secret = " / ".into();
        assert_eq!(c.request(&env(&[])), Err(VaultError::NoSecret));
    }

    #[test]
    fn namespace_and_certificate_fall_back_to_the_environment() {
        let mut c = config("kv/db", SecretType::Kv1);
        let vars = env(&[("VAULT_NAMESPACE", "team"), ("VAULT_CACERT", "/env/ca.pem")]);
        let r = c.request(&vars).unwrap();
        assert_eq!(r.namespace.as_deref(), Some("team"));
        assert_eq!(r.ca_cert.as_deref(), Some(Path::new("/env/ca.pem")));
        c.namespace = Some("admin/team".into());
        c.ca_cert = Some("/etc/ca.pem".into());
        let r = c.request(&vars).unwrap();
        assert_eq!(r.namespace.as_deref(), Some("admin/team"));
        assert_eq!(r.ca_cert.as_deref(), Some(Path::new("/etc/ca.pem")));
        let r = config("kv/db", SecretType::Kv1).request(&env(&[])).unwrap();
        assert_eq!((r.namespace, r.ca_cert), (None, None));
    }

    #[test]
    fn token_lookup_order() {
        let home = tempfile::tempdir().unwrap();
        let home = home.path();
        let mut c = config("kv/db", SecretType::Kv1);
        let none = env(&[]);
        let vars = env(&[("VAULT_TOKEN", "from-env")]);

        assert_eq!(c.token(&none, Some(home)), Err(VaultError::NoToken));
        std::fs::write(home.join(".vault-token"), "from-cli\n").unwrap();
        assert_eq!(c.token(&none, Some(home)).unwrap(), "from-cli");
        assert_eq!(c.token(&vars, Some(home)).unwrap(), "from-env");

        // The default token file, `~/.dato/.vault-token`, wins.
        std::fs::create_dir(home.join(".dato")).unwrap();
        std::fs::write(home.join(".dato/.vault-token"), " from-dato \n").unwrap();
        assert_eq!(c.token(&vars, Some(home)).unwrap(), "from-dato");

        // A configured file replaces the default one; `~` is the home.
        std::fs::write(home.join("mine"), "from-mine").unwrap();
        c.token_file = Some("~/mine".into());
        assert_eq!(c.token(&vars, Some(home)).unwrap(), "from-mine");
        // A configured file that does not exist falls through.
        c.token_file = Some("~/missing".into());
        assert_eq!(c.token(&vars, Some(home)).unwrap(), "from-env");

        std::fs::write(home.join("empty"), "\n").unwrap();
        c.token_file = Some("~/empty".into());
        assert!(matches!(
            c.token(&vars, Some(home)),
            Err(VaultError::TokenFile { .. })
        ));
        c.token_file = Some("~/.dato".into());
        assert!(matches!(
            c.token(&vars, Some(home)),
            Err(VaultError::TokenFile { message, .. }) if message == "not a file"
        ));
    }

    #[test]
    fn credentials_per_secret_type() {
        let db = json!({"lease_id": "x", "data": {"username": "v-app-1", "password": "p1"}});
        let kv1 = json!({"data": {"user": "app", "pass": "p2"}});
        let kv2 = json!({"data": {"data": {"user": "app", "pass": "p3"}, "metadata": {}}});
        let creds = |c: &VaultConfig, doc: &Value| c.credentials("s", doc);

        for t in [SecretType::DynamicRole, SecretType::StaticRole] {
            let got = creds(&config("s", t), &db).unwrap();
            assert_eq!(
                (got.username.as_str(), got.password.as_str()),
                ("v-app-1", "p1")
            );
        }

        let mut kv = config("s", SecretType::Kv1);
        kv.username_key = "user".into();
        kv.password_key = "pass".into();
        assert_eq!(creds(&kv, &kv1).unwrap().password, "p2");
        kv.secret_type = SecretType::Kv2;
        assert_eq!(creds(&kv, &kv2).unwrap().password, "p3");
        assert!(matches!(creds(&kv, &kv1), Err(VaultError::Response(_))));

        kv.password_key = "missing".into();
        assert_eq!(
            creds(&kv, &kv2),
            Err(VaultError::MissingKey {
                secret: "s".into(),
                key: "missing".into()
            })
        );
        assert!(matches!(
            creds(&kv, &json!({"errors": []})),
            Err(VaultError::Response(_))
        ));
    }

    #[test]
    fn password_is_not_debug_printed() {
        let c = Credentials {
            username: "u".into(),
            password: "hunter2".into(),
        };
        assert!(!format!("{c:?}").contains("hunter2"));
    }

    #[test]
    fn error_messages_from_vault() {
        assert_eq!(
            error_message(r#"{"errors":["permission denied","x"]}"#),
            "permission denied; x"
        );
        assert_eq!(error_message(r#"{"errors":[]}"#), r#"{"errors":[]}"#);
        assert_eq!(error_message(""), "no details");
        assert_eq!(error_message("bad gateway"), "bad gateway");
    }

    /// Serves one HTTP request with `status` and `body`; returns the address and
    /// a handle yielding the request's head.
    fn serve_once(status: u16, body: &str) -> (String, std::thread::JoinHandle<String>) {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = format!("http://{}", listener.local_addr().unwrap());
        let body = body.to_string();
        let handle = std::thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            let mut reader = BufReader::new(stream.try_clone().unwrap());
            let mut head = String::new();
            loop {
                let mut line = String::new();
                reader.read_line(&mut line).unwrap();
                if line == "\r\n" || line.is_empty() {
                    break;
                }
                head.push_str(&line);
            }
            write!(
                stream,
                "HTTP/1.1 {status} X\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                body.len()
            )
            .unwrap();
            head
        });
        (addr, handle)
    }

    fn request_to(address: &str, namespace: Option<&str>) -> Request {
        Request {
            url: format!("{address}/v1/database/creds/app"),
            address: address.into(),
            secret: "database/creds/app".into(),
            namespace: namespace.map(String::from),
            ca_cert: None,
        }
    }

    #[test]
    fn sends_token_and_namespace() {
        let (addr, server) = serve_once(200, r#"{"data":{"username":"u","password":"p"}}"#);
        let body = get(&request_to(&addr, Some("team")), "s.token").unwrap();
        assert!(body.contains("\"username\""));
        let head = server.join().unwrap().to_ascii_lowercase();
        assert!(
            head.starts_with("get /v1/database/creds/app http/1.1"),
            "{head}"
        );
        assert!(head.contains("x-vault-token: s.token"), "{head}");
        assert!(head.contains("x-vault-namespace: team"), "{head}");
    }

    #[test]
    fn reports_vault_errors() {
        let (addr, server) = serve_once(403, r#"{"errors":["permission denied"]}"#);
        let err = get(&request_to(&addr, None), "t").unwrap_err();
        server.join().unwrap();
        assert_eq!(
            err.to_string(),
            "Vault returned 403 for database/creds/app: permission denied"
        );
    }

    #[test]
    fn unreachable_vault_is_an_error() {
        let addr = {
            let l = TcpListener::bind("127.0.0.1:0").unwrap();
            format!("http://{}", l.local_addr().unwrap())
        };
        let err = get(&request_to(&addr, None), "t").unwrap_err();
        assert!(matches!(err, VaultError::Request { .. }), "{err}");
    }

    #[test]
    fn https_uses_configured_roots() {
        // Without explicit roots ureq would panic (no WebPki roots compiled in).
        let addr = {
            let l = TcpListener::bind("127.0.0.1:0").unwrap();
            format!("https://{}", l.local_addr().unwrap())
        };
        let err = get(&request_to(&addr, None), "t").unwrap_err();
        assert!(matches!(err, VaultError::Request { .. }), "{err}");
    }

    #[test]
    fn certificate_file_must_hold_a_certificate() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("ca.pem");
        std::fs::write(&path, "not a pem").unwrap();
        assert!(matches!(
            root_certs(Some(&path)),
            Err(VaultError::Certificate { .. })
        ));
        assert!(matches!(
            root_certs(Some(&dir.path().join("missing.pem"))),
            Err(VaultError::Certificate { .. })
        ));
        assert!(root_certs(None).is_ok());
    }
}

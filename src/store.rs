//! Saved connections, kept in `~/.dato/connections.json`.
//!
//! Connection details are stored without the password. The password is kept
//! only when the user ticks "Save password" in the connect wizard, in which
//! case it is written in clear text to a file only the user can read (`0600`).

use std::io;
use std::path::PathBuf;

use serde_json::{Map, Value, json};

use crate::client::psql::SslMode;
use crate::client::{ConnectOptions, MySqlOptions, PsqlOptions};
use crate::server::Server;

/// One entry of the saved connections list.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SavedConnection {
    pub name: String,
    /// Holds a password only when the user chose to save it. Otherwise
    /// `password_required` tells whether to ask for one when connecting.
    pub opts: ConnectOptions,
    /// Seconds since the Unix epoch.
    pub last_used: u64,
}

impl SavedConnection {
    pub fn server(&self) -> Server {
        self.opts.server()
    }

    /// `user@host:port`, or the database file for SQLite.
    pub fn detail(&self) -> String {
        match &self.opts {
            ConnectOptions::Sqlite { path } => path
                .as_deref()
                .map_or_else(|| ":memory:".into(), crate::paths::collapse_home),
            ConnectOptions::MySql(o) => format!("{}@{}:{}", o.user, o.host, o.port),
            ConnectOptions::Psql(o) => format!("{}@{}:{}", o.user, o.host, o.port),
        }
    }

    fn to_json(&self) -> Value {
        let mut m = Map::new();
        m.insert("name".into(), json!(self.name));
        m.insert("server".into(), json!(self.server().name()));
        m.insert("last_used".into(), json!(self.last_used));
        match &self.opts {
            ConnectOptions::Sqlite { path } => {
                m.insert("path".into(), json!(path));
            }
            ConnectOptions::MySql(o) => {
                m.insert("host".into(), json!(o.host));
                m.insert("port".into(), json!(o.port));
                m.insert("user".into(), json!(o.user));
                m.insert("database".into(), json!(o.database));
                m.insert("socket".into(), json!(o.socket));
                m.insert("password".into(), json!(o.password));
                m.insert("password_required".into(), json!(o.password_required));
            }
            ConnectOptions::Psql(o) => {
                m.insert("host".into(), json!(o.host));
                m.insert("port".into(), json!(o.port));
                m.insert("user".into(), json!(o.user));
                m.insert("database".into(), json!(o.dbname));
                m.insert("sslmode".into(), json!(o.sslmode.name()));
                m.insert("sslrootcert".into(), json!(o.sslrootcert));
                m.insert("password".into(), json!(o.password));
                m.insert("password_required".into(), json!(o.password_required));
            }
        }
        Value::Object(m)
    }

    fn from_json(v: &Value) -> Option<Self> {
        let text = |k: &str| v.get(k).and_then(Value::as_str).map(str::to_string);
        let flag = |k: &str| v.get(k).and_then(Value::as_bool).unwrap_or(false);
        let port = |default: u16| {
            v.get("port")
                .and_then(Value::as_u64)
                .and_then(|p| u16::try_from(p).ok())
                .unwrap_or(default)
        };
        let opts = match Server::parse(&text("server")?)? {
            Server::Sqlite => ConnectOptions::Sqlite { path: text("path") },
            Server::MySql => {
                let d = MySqlOptions::default();
                ConnectOptions::MySql(MySqlOptions {
                    host: text("host").unwrap_or(d.host),
                    port: port(d.port),
                    user: text("user").unwrap_or_default(),
                    password: text("password"),
                    password_required: flag("password_required"),
                    socket: text("socket"),
                    database: text("database"),
                })
            }
            Server::PostgreSql => {
                let d = PsqlOptions::default();
                ConnectOptions::Psql(PsqlOptions {
                    host: text("host").unwrap_or(d.host),
                    port: port(d.port),
                    user: text("user").unwrap_or_default(),
                    password: text("password"),
                    password_required: flag("password_required"),
                    dbname: text("database"),
                    sslmode: text("sslmode")
                        .and_then(|m| m.parse::<SslMode>().ok())
                        .unwrap_or(SslMode::Disable),
                    sslrootcert: text("sslrootcert"),
                })
            }
        };
        Some(SavedConnection {
            name: text("name")?,
            opts,
            last_used: v.get("last_used").and_then(Value::as_u64).unwrap_or(0),
        })
    }
}

/// A name for a connection the user did not name: the database, else the host.
pub fn default_name(opts: &ConnectOptions) -> String {
    match opts {
        ConnectOptions::Sqlite { path } => path
            .as_deref()
            .and_then(|p| std::path::Path::new(p).file_name())
            .map_or_else(|| "memory".into(), |f| f.to_string_lossy().into_owned()),
        ConnectOptions::MySql(o) => o.database.clone().unwrap_or_else(|| o.host.clone()),
        ConnectOptions::Psql(o) => o.dbname.clone().unwrap_or_else(|| o.host.clone()),
    }
}

/// Options as they are stored: the password is dropped unless `keep_password`.
/// A dropped password makes the next connect ask for one.
fn sanitize(opts: &ConnectOptions, keep_password: bool) -> ConnectOptions {
    let mut opts = opts.clone();
    let (password, required) = match &mut opts {
        ConnectOptions::Sqlite { .. } => return opts,
        ConnectOptions::MySql(o) => (&mut o.password, &mut o.password_required),
        ConnectOptions::Psql(o) => (&mut o.password, &mut o.password_required),
    };
    if keep_password && password.is_some() {
        *required = false;
    } else {
        *required |= password.take().is_some();
    }
    opts
}

/// The saved connections, most recently used first.
#[derive(Debug, Clone, Default)]
pub struct Store {
    /// `None` keeps the store in memory only.
    path: Option<PathBuf>,
    items: Vec<SavedConnection>,
}

impl Store {
    pub fn empty(path: Option<PathBuf>) -> Self {
        Self {
            path,
            items: Vec::new(),
        }
    }

    /// Reads the store at `path`; a missing file is an empty store.
    pub fn load(path: PathBuf) -> io::Result<Self> {
        let text = match std::fs::read_to_string(&path) {
            Ok(t) => t,
            Err(e) if e.kind() == io::ErrorKind::NotFound => {
                return Ok(Self::empty(Some(path)));
            }
            Err(e) => return Err(e),
        };
        let doc: Value = serde_json::from_str(&text)
            .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))?;
        let items = doc
            .get("connections")
            .and_then(Value::as_array)
            .map(|a| a.iter().filter_map(SavedConnection::from_json).collect())
            .unwrap_or_default();
        let mut store = Self {
            path: Some(path),
            items,
        };
        store.sort();
        Ok(store)
    }

    pub fn items(&self) -> &[SavedConnection] {
        &self.items
    }

    fn sort(&mut self) {
        self.items.sort_by_key(|c| std::cmp::Reverse(c.last_used));
    }

    /// Adds or replaces the connection called `name`, used at `now`.
    pub fn remember(&mut self, name: &str, opts: &ConnectOptions, keep_password: bool, now: u64) {
        self.items.retain(|c| c.name != name);
        self.items.insert(
            0,
            SavedConnection {
                name: name.to_string(),
                opts: sanitize(opts, keep_password),
                last_used: now,
            },
        );
        self.sort();
    }

    /// Marks the connection called `name` as used at `now`.
    pub fn touch(&mut self, name: &str, now: u64) {
        if let Some(c) = self.items.iter_mut().find(|c| c.name == name) {
            c.last_used = now;
        }
        self.sort();
    }

    /// Writes the store. The file is private to the user on Unix.
    pub fn save(&self) -> io::Result<()> {
        let Some(path) = &self.path else {
            return Ok(());
        };
        if let Some(dir) = path.parent() {
            crate::paths::ensure_dir(dir)?;
        }
        let doc = json!({ "connections": self.items.iter().map(SavedConnection::to_json).collect::<Vec<_>>() });
        let text = serde_json::to_string_pretty(&doc).map_err(io::Error::other)?;
        let tmp = path.with_extension("json.tmp");
        write_private(&tmp, text.as_bytes())?;
        std::fs::rename(&tmp, path)
    }
}

fn write_private(path: &std::path::Path, bytes: &[u8]) -> io::Result<()> {
    use std::io::Write;
    let mut opts = std::fs::OpenOptions::new();
    opts.write(true).create(true).truncate(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        opts.mode(0o600);
    }
    opts.open(path)?.write_all(bytes)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pg(db: &str, password: Option<&str>) -> ConnectOptions {
        ConnectOptions::Psql(PsqlOptions {
            user: "app".into(),
            password: password.map(String::from),
            dbname: Some(db.into()),
            sslmode: SslMode::Require,
            ..Default::default()
        })
    }

    #[test]
    fn password_is_dropped_unless_kept() {
        let mut s = Store::empty(None);
        s.remember("a", &pg("shop", Some("pw")), false, 1);
        s.remember("b", &pg("shop", Some("pw")), true, 2);
        s.remember("c", &pg("shop", None), false, 3);
        let get = |n: &str| match &s.items().iter().find(|c| c.name == n).unwrap().opts {
            ConnectOptions::Psql(o) => (o.password.clone(), o.password_required),
            _ => unreachable!(),
        };
        assert_eq!(get("a"), (None, true));
        assert_eq!(get("b"), (Some("pw".into()), false));
        assert_eq!(get("c"), (None, false));
    }

    #[test]
    fn most_recently_used_first() {
        let mut s = Store::empty(None);
        s.remember("a", &pg("a", None), false, 1);
        s.remember("b", &pg("b", None), false, 2);
        let names = |s: &Store| s.items().iter().map(|c| c.name.clone()).collect::<Vec<_>>();
        assert_eq!(names(&s), ["b", "a"]);
        s.touch("a", 3);
        assert_eq!(names(&s), ["a", "b"]);
        // Same name replaces instead of duplicating.
        s.remember("b", &pg("b2", None), false, 4);
        assert_eq!(names(&s), ["b", "a"]);
        assert_eq!(s.items().len(), 2);
    }

    #[test]
    fn round_trips_through_the_file() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("connections.json");
        assert!(Store::load(path.clone()).unwrap().items().is_empty());
        let mut s = Store::empty(Some(path.clone()));
        s.remember("shop", &pg("shop", Some("pw")), false, 5);
        s.remember("lite", &ConnectOptions::sqlite(Some("app.db")), false, 7);
        s.remember(
            "my",
            &ConnectOptions::MySql(MySqlOptions {
                user: "root".into(),
                database: Some("d".into()),
                ..Default::default()
            }),
            false,
            6,
        );
        s.save().unwrap();
        let text = std::fs::read_to_string(&path).unwrap();
        assert!(!text.contains("\"pw\""), "{text}");
        let loaded = Store::load(path.clone()).unwrap();
        assert_eq!(loaded.items(), s.items());
        assert_eq!(loaded.items()[0].name, "lite");
        assert_eq!(loaded.items()[1].detail(), "root@localhost:3306");
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = std::fs::metadata(&path).unwrap().permissions().mode();
            assert_eq!(mode & 0o777, 0o600);
        }
    }

    #[test]
    fn corrupt_file_is_an_error() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("connections.json");
        std::fs::write(&path, "{nope").unwrap();
        assert!(Store::load(path).is_err());
    }

    #[test]
    fn default_names() {
        assert_eq!(default_name(&pg("shop", None)), "shop");
        assert_eq!(
            default_name(&ConnectOptions::sqlite(Some("/x/app.db"))),
            "app.db"
        );
        assert_eq!(default_name(&ConnectOptions::sqlite(None)), "memory");
    }
}

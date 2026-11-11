//! PostgreSQL client tests against a real server.
//!
//! Gated on `DATO_PSQL_URL` (e.g. `postgres://postgres:secret@127.0.0.1:5432`).
//! The user must be able to create and drop databases: each test works in its own
//! `dato_test_*` database. Without the variable every test returns early.
//!
//! TLS tests adapt to the server: when it has `ssl = on`, `DATO_PSQL_CA` names
//! the CA certificate file that signed its certificate (for `verify-full`; the
//! server certificate must be an end-entity certificate, not a self-signed CA).

use std::time::{Duration, Instant};

use dato::client::psql::SslMode;
use dato::client::{
    self, Client, ClientError, ConnectOptions, PsqlOptions, StatementResult, Value,
};
use dato::render::Format;
use dato::tui::session::Session;

/// Parses `postgres://user[:pass]@host[:port][/db]`.
fn options_from_url(url: &str) -> PsqlOptions {
    let rest = url
        .strip_prefix("postgres://")
        .expect("DATO_PSQL_URL must start with postgres://");
    let (auth, rest) = rest.rsplit_once('@').unwrap_or(("postgres", rest));
    let (user, password) = match auth.split_once(':') {
        Some((u, p)) => (u.to_string(), Some(p.to_string())),
        None => (auth.to_string(), None),
    };
    let (hostport, dbname) = match rest.split_once('/') {
        Some((h, d)) if !d.is_empty() => (h, Some(d.to_string())),
        Some((h, _)) => (h, None),
        None => (rest, None),
    };
    let (host, port) = match hostport.rsplit_once(':') {
        Some((h, p)) => (h.to_string(), p.parse().expect("port")),
        None => (hostport.to_string(), 5432),
    };
    PsqlOptions {
        host,
        port,
        user,
        password,
        dbname,
        ..Default::default()
    }
}

fn base_options() -> Option<PsqlOptions> {
    match std::env::var("DATO_PSQL_URL") {
        Ok(url) => Some(options_from_url(&url)),
        Err(_) => {
            eprintln!("DATO_PSQL_URL not set; skipping PostgreSQL test");
            None
        }
    }
}

/// Options for a fresh, empty database `dato_test_<name>`.
fn fresh_db(name: &str) -> Option<PsqlOptions> {
    let mut opts = base_options()?;
    opts.dbname = Some("postgres".into());
    let db = format!("dato_test_{name}");
    let mut admin = client::open(&ConnectOptions::Psql(opts.clone())).unwrap();
    admin
        .execute(&format!(
            "drop database if exists {db} with (force); create database {db};"
        ))
        .unwrap();
    opts.dbname = Some(db);
    Some(opts)
}

fn connect(name: &str) -> Option<Box<dyn Client>> {
    let opts = fresh_db(name)?;
    Some(client::open(&ConnectOptions::Psql(opts)).unwrap())
}

fn rows(r: &StatementResult) -> &Vec<Vec<Value>> {
    match r {
        StatementResult::Rows(rs) => &rs.rows,
        other => panic!("expected rows, got {other:?}"),
    }
}

fn text(out: &[dato::tui::session::Output]) -> String {
    format!("{out:?}")
}

/// Whether connections to the server can use TLS. `DATO_PSQL_TLS=on|off` says so
/// when the server sits behind a TLS-terminating proxy, where `ssl` reads `off`.
fn server_has_tls(c: &mut dyn Client) -> bool {
    if let Ok(v) = std::env::var("DATO_PSQL_TLS") {
        return v == "on";
    }
    let r = c
        .execute("select setting from pg_settings where name = 'ssl'")
        .unwrap();
    rows(&r[0])[0][0] == Value::Text("on".into())
}

#[test]
fn connect_with_database() {
    let Some(mut c) = connect("connect") else {
        return;
    };
    let r = c.execute("select current_database()").unwrap();
    assert_eq!(rows(&r[0])[0][0], Value::Text("dato_test_connect".into()));
    assert!(c.prompt_label().ends_with("/dato_test_connect"));
    assert!(c.prompt_label().contains('@'));
}

#[test]
fn wrong_password() {
    let Some(mut opts) = base_options() else {
        return;
    };
    opts.password = Some("definitely-not-the-password".into());
    let err = client::open(&ConnectOptions::Psql(opts)).err().unwrap();
    assert!(
        err.to_string().contains("password authentication failed"),
        "{err}"
    );
}

#[test]
fn tls_modes() {
    let Some(mut opts) = base_options() else {
        return;
    };
    opts.dbname = Some("postgres".into());
    let mut probe = client::open(&ConnectOptions::Psql(opts.clone())).unwrap();
    let tls = server_has_tls(probe.as_mut());

    let open = |mode, root: Option<String>| {
        let mut o = opts.clone();
        o.sslmode = mode;
        o.sslrootcert = root;
        client::open(&ConnectOptions::Psql(o))
    };
    assert!(open(SslMode::Disable, None).is_ok());
    // `prefer` works either way: encrypted when offered, plain when not.
    assert!(open(SslMode::Prefer, None).is_ok());
    if tls {
        assert!(open(SslMode::Require, None).is_ok());
        // The test certificate is signed by a private CA, so the system roots reject it.
        let err = open(SslMode::VerifyFull, None).err().unwrap();
        assert!(err.to_string().to_lowercase().contains("cert"), "{err}");
        if let Ok(ca) = std::env::var("DATO_PSQL_CA")
            && let Err(e) = open(SslMode::VerifyFull, Some(ca))
        {
            panic!("verify-full with the server certificate failed: {e}");
        }
    } else {
        let err = open(SslMode::Require, None).err().unwrap();
        assert!(err.to_string().contains("TLS"), "{err}");
        let err = open(SslMode::VerifyFull, None).err().unwrap();
        assert!(err.to_string().contains("TLS"), "{err}");
    }
}

#[test]
fn two_statements_and_update_count() {
    let Some(mut c) = connect("two") else { return };
    let r = c
        .execute("create table t(a int); insert into t values (1), (2), (3);")
        .unwrap();
    assert_eq!(r.len(), 2);
    assert_eq!(r[1], StatementResult::Affected(3));
    let r = c.execute("update t set a = a + 1;").unwrap();
    assert_eq!(r, vec![StatementResult::Affected(3)]);
}

#[test]
fn error_halts_remaining_statements() {
    let Some(mut c) = connect("halt") else { return };
    let err = c
        .execute("select 1; select * from missing; select 2;")
        .unwrap_err();
    assert!(err.to_string().contains("does not exist"), "{err}");
    assert_eq!(err.partial().len(), 1);
    assert_eq!(rows(&err.partial()[0])[0][0], Value::Integer(1));
}

#[test]
fn dollar_quoted_body_is_one_statement() {
    let Some(mut c) = connect("dollar") else {
        return;
    };
    let r = c
        .execute(
            "create function two() returns int language sql as $$ select 1; select 2; $$; \
             select two();",
        )
        .unwrap();
    assert_eq!(r.len(), 2);
    assert_eq!(rows(&r[1])[0][0], Value::Integer(2));
    let r = c.execute("select $a$ it's; $$ fine $a$").unwrap();
    assert_eq!(rows(&r[0])[0][0], Value::Text(" it's; $$ fine ".into()));
}

#[test]
fn value_mapping_keeps_precision() {
    let Some(mut c) = connect("values") else {
        return;
    };
    let r = c
        .execute(
            "select 9223372036854775807::bigint as b, 1.10::numeric(5,2) as d, 2.5::float8 as f, \
             true as t, null as n, '\\x00ff'::bytea as y, \
             array[1,2,3] as a, '{\"k\": 1}'::jsonb as j, \
             '2026-01-02 03:04:05+00'::timestamptz as ts",
        )
        .unwrap();
    let row = &rows(&r[0])[0];
    assert_eq!(row[0], Value::Integer(i64::MAX));
    assert_eq!(row[1], Value::Numeric("1.10".into()));
    assert_eq!(row[2], Value::Real(2.5));
    assert_eq!(row[3], Value::Text("t".into()));
    assert_eq!(row[4], Value::Null);
    assert_eq!(row[5], Value::Blob(vec![0, 255]));
    assert_eq!(row[6], Value::Text("{1,2,3}".into()));
    assert_eq!(row[7], Value::Text("{\"k\": 1}".into()));
    assert!(matches!(&row[8], Value::Text(t) if t.starts_with("2026-01-02 03:04:05")));

    let mut s = Session::new(c, Format::Psql, false);
    let out = text(&s.handle("select 1.10::numeric(5,2) as d;"));
    assert!(out.contains("\\n 1.10 \\n"), "{out}");
}

#[test]
fn tables_views_columns_and_create() {
    let Some(mut c) = connect("schema") else {
        return;
    };
    c.execute(
        "create table orders(id int primary key, total numeric(10,2) not null, \
           note text default 'x'); \
         create view big_orders as select * from orders where total > 100; \
         create index orders_total on orders (total);",
    )
    .unwrap();
    assert_eq!(c.tables(None).unwrap(), ["big_orders", "orders"]);
    assert_eq!(c.views(None).unwrap(), ["big_orders"]);
    assert_eq!(c.tables(Some("ord")).unwrap(), ["orders"]);
    let cols = c.columns("orders").unwrap();
    assert_eq!(cols[0].name, "id");
    assert_eq!(cols[0].type_name, "integer");
    assert!(cols[0].primary_key);
    assert_eq!(
        (cols[1].name.as_str(), cols[1].type_name.as_str()),
        ("total", "numeric(10,2)")
    );
    assert!(cols[1].not_null && !cols[1].primary_key);
    assert_eq!(cols[2].default.as_deref(), Some("'x'::text"));

    let rels = c.relations().unwrap();
    let names: Vec<_> = rels.iter().map(|r| r.name.as_str()).collect();
    assert_eq!(names, ["big_orders", "orders"]);
    assert_eq!(rels[1].columns.len(), 3);
    assert_eq!(rels[0].kind, dato::client::RelationKind::View);

    let create = &c.schema(Some("orders")).unwrap()[0];
    assert!(create.starts_with("CREATE TABLE orders ("), "{create}");
    assert!(create.contains("id integer NOT NULL"), "{create}");
    assert!(create.contains("total numeric(10,2) NOT NULL"), "{create}");
    assert!(create.contains("note text DEFAULT 'x'::text"), "{create}");
    assert!(create.contains("PRIMARY KEY (id)"), "{create}");
    assert!(c.schema(Some("big_orders")).unwrap()[0].starts_with("CREATE VIEW big_orders AS"));

    let idx = c.indexes(Some("orders")).unwrap();
    assert_eq!(idx.columns, ["name", "sql"]);
    assert_eq!(idx.rows[0][0], Value::Text("orders_pkey".into()));
    assert_eq!(idx.rows.len(), 2);

    let err = c.columns("missing").unwrap_err();
    assert!(err.to_string().contains("does not exist"), "{err}");
}

#[test]
fn system_schemas_never_appear() {
    let Some(mut c) = connect("system") else {
        return;
    };
    c.execute("create table t(a int)").unwrap();
    let qualified = c.qualified_tables().unwrap();
    assert_eq!(qualified, [("public".to_string(), "t".to_string())]);
    assert!(
        c.schema_relations()
            .unwrap()
            .iter()
            .all(|r| r.schema.as_deref() == Some("public"))
    );
    assert!(c.relations().unwrap().iter().all(|r| r.name != "pg_class"));
}

#[test]
fn databases_command_lists_databases() {
    let Some(opts) = fresh_db("dblist") else {
        return;
    };
    let mut s = Session::disconnected(Format::Tsv, false);
    s.connect(&ConnectOptions::Psql(opts)).unwrap();
    let out = text(&s.handle("\\l"));
    assert!(out.contains("dato_test_dblist"), "{out}");
    assert!(
        !out.contains("template0") && !out.contains("template1"),
        "{out}"
    );
    assert!(text(&s.handle(".databases")).contains("dato_test_dblist"));
}

#[test]
fn search_path_change_refreshes_completion() {
    let Some(opts) = fresh_db("path") else {
        return;
    };
    let mut s = Session::disconnected(Format::Tsv, false);
    s.connect(&ConnectOptions::Psql(opts)).unwrap();
    s.handle("create schema sales; create table sales.invoices(total int);");
    assert!(s.schema().read().unwrap().find("invoices").is_none());
    {
        let cache = s.schema().read().unwrap().clone();
        assert!(cache.has_qualifier("sales"));
        assert!(cache.find_in("sales", "invoices").is_some());
    }
    s.handle("set search_path to sales;");
    assert!(s.schema().read().unwrap().find("invoices").is_some());
}

#[test]
fn interrupt_stops_query_and_keeps_connection() {
    let Some(mut c) = connect("interrupt") else {
        return;
    };
    let handle = c.interrupt_handle();
    let t = std::thread::spawn(move || {
        std::thread::sleep(Duration::from_millis(500));
        handle.interrupt();
    });
    let started = Instant::now();
    let err = c.execute("select pg_sleep(30);").unwrap_err();
    t.join().unwrap();
    assert!(matches!(err, ClientError::Interrupted { .. }), "{err}");
    assert!(started.elapsed() < Duration::from_secs(5));
    let r = c.execute("select 1").unwrap();
    assert_eq!(rows(&r[0])[0][0], Value::Integer(1));
}

#[test]
fn completion_offers_schema_qualified_names() {
    let Some(opts) = fresh_db("complete") else {
        return;
    };
    let mut s = Session::disconnected(Format::Tsv, false);
    s.connect(&ConnectOptions::Psql(opts)).unwrap();
    s.handle("create schema sales; create table sales.invoices(id int, total numeric(10,2));");
    s.handle("create table orders(id int);");
    let cache = s.schema().read().unwrap().clone();
    let values = |text: &str| -> Vec<String> {
        let pos = text.find('|').unwrap();
        let line = text.replacen('|', "", 1);
        dato::editor::candidates(&cache, &line, pos)
            .1
            .into_iter()
            .map(|c| c.value)
            .collect()
    };
    assert_eq!(values("select * from sales.|"), ["invoices"]);
    assert_eq!(
        values("select sales.invoices.| from sales.invoices"),
        ["id", "total"]
    );
    assert_eq!(values("select i.| from sales.invoices i"), ["id", "total"]);
    assert!(values("select * from |").contains(&"orders".to_string()));
    assert!(!values("select * from |").contains(&"invoices".to_string()));
}

//! MySQL client tests against a real server.
//!
//! Gated on `DATO_MYSQL_URL` (e.g. `mysql://root:secret@127.0.0.1:3306`). The
//! user must be able to create and drop databases: each test works in its own
//! `dato_test_*` database. Without the variable every test returns early.

use std::time::{Duration, Instant};

use dato::client::{
    self, Client, ClientError, ConnectOptions, MySqlOptions, StatementResult, Value,
};
use dato::render::Format;
use dato::tui::session::Session;

/// Parses `mysql://user[:pass]@host[:port][/db]`.
fn options_from_url(url: &str) -> MySqlOptions {
    let rest = url
        .strip_prefix("mysql://")
        .expect("DATO_MYSQL_URL must start with mysql://");
    let (auth, rest) = rest.rsplit_once('@').unwrap_or(("root", rest));
    let (user, password) = match auth.split_once(':') {
        Some((u, p)) => (u.to_string(), Some(p.to_string())),
        None => (auth.to_string(), None),
    };
    let (hostport, database) = match rest.split_once('/') {
        Some((h, d)) if !d.is_empty() => (h, Some(d.to_string())),
        Some((h, _)) => (h, None),
        None => (rest, None),
    };
    let (host, port) = match hostport.rsplit_once(':') {
        Some((h, p)) => (h.to_string(), p.parse().expect("port")),
        None => (hostport.to_string(), 3306),
    };
    MySqlOptions {
        host,
        port,
        user,
        password,
        password_required: false,
        socket: None,
        database,
    }
}

fn base_options() -> Option<MySqlOptions> {
    match std::env::var("DATO_MYSQL_URL") {
        Ok(url) => Some(options_from_url(&url)),
        Err(_) => {
            eprintln!("DATO_MYSQL_URL not set; skipping MySQL test");
            None
        }
    }
}

/// Options for a fresh, empty database `dato_test_<name>`.
fn fresh_db(name: &str) -> Option<MySqlOptions> {
    let mut opts = base_options()?;
    opts.database = None;
    let db = format!("dato_test_{name}");
    let mut admin = client::open(&ConnectOptions::MySql(opts.clone())).unwrap();
    admin
        .execute(&format!(
            "drop database if exists {db}; create database {db};"
        ))
        .unwrap();
    opts.database = Some(db);
    Some(opts)
}

fn connect(name: &str) -> Option<Box<dyn Client>> {
    let opts = fresh_db(name)?;
    Some(client::open(&ConnectOptions::MySql(opts)).unwrap())
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

#[test]
fn connect_with_database() {
    let Some(mut c) = connect("connect") else {
        return;
    };
    let r = c.execute("select database()").unwrap();
    assert_eq!(rows(&r[0])[0][0], Value::Text("dato_test_connect".into()));
    assert!(c.prompt_label().ends_with("/dato_test_connect"));
}

#[test]
fn wrong_password() {
    let Some(mut opts) = base_options() else {
        return;
    };
    opts.password = Some("definitely-not-the-password".into());
    let err = client::open(&ConnectOptions::MySql(opts)).err().unwrap();
    assert!(err.to_string().contains("Access denied"), "{err}");
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
    assert!(err.to_string().contains("doesn't exist"), "{err}");
    assert_eq!(err.partial().len(), 1);
    assert_eq!(rows(&err.partial()[0])[0][0], Value::Integer(1));
}

#[test]
fn value_mapping_keeps_precision() {
    let Some(mut c) = connect("values") else {
        return;
    };
    let r = c
        .execute(
            "select cast(18446744073709551615 as unsigned) as u, cast(1.10 as decimal(5,2)) as d, \
             cast('2026-01-02 03:04:05' as datetime) as t, 2.5e0 as f, null as n, x'00ff' as b",
        )
        .unwrap();
    assert_eq!(
        rows(&r[0])[0],
        vec![
            Value::Numeric("18446744073709551615".into()),
            Value::Numeric("1.10".into()),
            Value::Text("2026-01-02 03:04:05".into()),
            Value::Real(2.5),
            Value::Null,
            Value::Blob(vec![0, 255]),
        ]
    );

    let mut s = Session::new(c, Format::Psql, false);
    let out = text(&s.handle("select cast(1.10 as decimal(5,2)) as d;"));
    assert!(out.contains("    d \\n------\\n 1.10 "), "{out}");
}

#[test]
fn tables_views_and_columns() {
    let Some(mut c) = connect("schema") else {
        return;
    };
    c.execute(
        "create table orders(id int primary key, total decimal(10,2) not null); \
         create view big_orders as select * from orders where total > 100;",
    )
    .unwrap();
    assert_eq!(c.tables(None).unwrap(), ["big_orders", "orders"]);
    assert_eq!(c.views(None).unwrap(), ["big_orders"]);
    assert_eq!(c.tables(Some("ord")).unwrap(), ["orders"]);
    let cols = c.columns("orders").unwrap();
    assert_eq!(cols[0].name, "id");
    // MySQL 8 reports `int`, MariaDB `int(11)`.
    assert!(cols[0].type_name.starts_with("int"), "{:?}", cols[0]);
    assert!(cols[0].primary_key);
    assert_eq!(
        (cols[1].name.as_str(), cols[1].type_name.as_str()),
        ("total", "decimal(10,2)")
    );
    assert!(cols[1].not_null && !cols[1].primary_key);

    let rels = c.relations().unwrap();
    let names: Vec<_> = rels.iter().map(|r| r.name.as_str()).collect();
    assert_eq!(names, ["big_orders", "orders"]);
    assert_eq!(rels[1].columns.len(), 2);

    assert!(c.schema(Some("orders")).unwrap()[0].contains("CREATE TABLE `orders`"));
    let idx = c.indexes(Some("orders")).unwrap();
    assert_eq!(idx.columns, ["name", "sql"]);
    assert_eq!(idx.rows[0][0], Value::Text("PRIMARY".into()));
}

#[test]
fn databases_command_lists_schemas() {
    let Some(opts) = fresh_db("dblist") else {
        return;
    };
    let mut s = Session::disconnected(Format::Tsv, false);
    s.connect(&ConnectOptions::MySql(opts)).unwrap();
    let out = text(&s.handle("\\l"));
    assert!(out.contains("dato_test_dblist"), "{out}");
    assert!(text(&s.handle(".databases")).contains("dato_test_dblist"));
}

#[test]
fn use_switches_current_database() {
    let Some(opts) = fresh_db("use_a") else {
        return;
    };
    let other = fresh_db("use_b").unwrap();
    let mut s = Session::disconnected(Format::Tsv, false);
    s.connect(&ConnectOptions::MySql(other)).unwrap();
    s.handle("create table contacts(id int);");
    s.connect(&ConnectOptions::MySql(opts)).unwrap();
    assert!(s.prompt_label().ends_with("/dato_test_use_a"));
    assert!(s.schema().read().unwrap().find("contacts").is_none());

    s.handle("use dato_test_use_b;");
    assert!(
        s.prompt_label().ends_with("/dato_test_use_b"),
        "{}",
        s.prompt_label()
    );
    assert!(s.schema().read().unwrap().find("contacts").is_some());
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
    let err = c.execute("select sleep(30);").unwrap_err();
    t.join().unwrap();
    assert!(matches!(err, ClientError::Interrupted { .. }), "{err}");
    assert!(started.elapsed() < Duration::from_secs(5));
    let r = c.execute("select 1").unwrap();
    assert_eq!(rows(&r[0])[0][0], Value::Integer(1));
}

#[test]
fn interrupt_stops_long_running_select() {
    let Some(mut c) = connect("interrupt_heavy") else {
        return;
    };
    let handle = c.interrupt_handle();
    let t = std::thread::spawn(move || {
        std::thread::sleep(Duration::from_millis(500));
        handle.interrupt();
    });
    let started = Instant::now();
    let err = c
        .execute("select benchmark(10000000000, md5('x'));")
        .unwrap_err();
    t.join().unwrap();
    assert!(matches!(err, ClientError::Interrupted { .. }), "{err}");
    assert!(started.elapsed() < Duration::from_secs(5));
    assert!(c.execute("select 1").is_ok());
}

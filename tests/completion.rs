//! Completion against a real in-memory SQLite session.

use dato::client::{Target, open};
use dato::editor::{Candidate, candidates};
use dato::render::Format;
use dato::tui::session::Session;

fn session() -> Session {
    let client = open(&Target::parse(Some(":memory:"))).unwrap();
    Session::new(client, Format::Ascii, false)
}

/// Candidates at the `|` cursor marker, against the session's current schema.
fn complete(s: &Session, text: &str) -> Vec<Candidate> {
    let pos = text.find('|').unwrap();
    let line = text.replacen('|', "", 1);
    let schema = s.schema();
    let cache = schema.read().unwrap();
    candidates(&cache, &line, pos).1
}

fn values(s: &Session, text: &str) -> Vec<String> {
    complete(s, text).into_iter().map(|c| c.value).collect()
}

fn with_tables() -> Session {
    let mut s = session();
    s.handle("create table customers(id integer, email text);");
    s.handle("create table orders(id integer, total real, customer_id integer);");
    s
}

#[test]
fn alias_dot_offers_the_aliased_tables_columns() {
    let s = with_tables();
    assert_eq!(
        values(&s, "select o.| from orders o"),
        vec!["customer_id", "id", "total"]
    );
}

#[test]
fn columns_are_scoped_to_the_query() {
    let s = with_tables();
    assert!(!values(&s, "select e| from orders").contains(&"email".to_string()));
    assert!(values(&s, "select e| from customers").contains(&"email".to_string()));
}

#[test]
fn new_table_and_column_appear_after_ddl() {
    let mut s = with_tables();
    assert!(values(&s, "select * from inv|").is_empty());
    s.handle("create table invoices(id);");
    assert_eq!(values(&s, "select * from inv|"), vec!["invoices"]);

    s.handle("alter table orders add column note text;");
    assert_eq!(values(&s, "select no| from orders")[0], "note");
}

#[test]
fn views_and_column_details() {
    let mut s = with_tables();
    s.handle("create view big as select * from orders where total > 100;");
    let c = complete(&s, "select * from bi|");
    assert_eq!(c[0].detail, "view");
    let c = complete(&s, "select o.tot| from orders o");
    assert_eq!(c[0].detail, "column real (orders)");
}

//! Keyword, special-command and schema completion.
//!
//! `analyze` decides what kinds of names fit the cursor; this module collects
//! them from the schema, matches them against the typed fragment and ranks them.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, RwLock};

use reedline::{Completer, CompletionResult, Span, Suggestion};

use super::context::{Context, Expect, Reference, analyze_with};
use crate::dialect::{Dialect, Qualifier};
use crate::render::Format;
#[cfg(test)]
use crate::server::Server;
use crate::special::symbolic_names;
use crate::tui::session::{Relation, RelationKind, SchemaCache};

/// Most items offered at once.
const MAX_CANDIDATES: usize = 100;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    Table,
    View,
    Schema,
    Column,
    Keyword,
    Command,
    Database,
    Format,
}

impl Kind {
    /// Shown on the suggestion's row.
    fn label(self) -> &'static str {
        match self {
            Kind::Table => "table",
            Kind::View => "view",
            Kind::Schema => "schema",
            Kind::Column => "column",
            Kind::Keyword => "keyword",
            Kind::Command => "command",
            Kind::Database => "database",
            Kind::Format => "format",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Candidate {
    pub value: String,
    pub kind: Kind,
    /// Shown beside the item: its kind, and for columns the type and relation.
    pub detail: String,
    /// Character positions of `value` that matched the typed text.
    pub matched: Vec<usize>,
}

/// How well a name matches: prefix matches first, then tighter, then earlier.
type Rank = (bool, usize, usize);

/// Matches `prefix` (already lower-cased) as a case-insensitive subsequence of `name`.
fn fuzzy(prefix: &[char], name: &str) -> Option<(Rank, Vec<usize>)> {
    if prefix.is_empty() {
        return Some(((false, 0, 0), Vec::new()));
    }
    let mut matched = Vec::with_capacity(prefix.len());
    let mut wanted = prefix.iter().peekable();
    for (i, c) in name.chars().enumerate() {
        match wanted.peek() {
            Some(&&p) if c.to_lowercase().next() == Some(p) => {
                matched.push(i);
                wanted.next();
            }
            Some(_) => {}
            None => break,
        }
    }
    if wanted.peek().is_some() {
        return None;
    }
    let (first, last) = (matched.first().copied()?, matched.last().copied()?);
    let span = last - first + 1;
    Some(((first != 0 || span != prefix.len(), span, first), matched))
}

/// Candidates of one kind whose name fuzzy-matches `prefix`, best first.
fn rank<'a>(
    prefix: &[char],
    items: impl Iterator<Item = (&'a str, Kind, String)>,
) -> Vec<Candidate> {
    let mut hits: Vec<(Rank, Candidate)> = items
        .filter_map(|(name, kind, detail)| {
            let (rank, matched) = fuzzy(prefix, name)?;
            // Completing a word to itself adds nothing.
            if !prefix.is_empty()
                && name.chars().count() == prefix.len()
                && rank == (false, prefix.len(), 0)
            {
                return None;
            }
            Some((
                rank,
                Candidate {
                    value: name.to_string(),
                    kind,
                    detail,
                    matched,
                },
            ))
        })
        .collect();
    hits.sort_by(|a, b| {
        a.0.cmp(&b.0)
            .then_with(|| a.1.value.to_lowercase().cmp(&b.1.value.to_lowercase()))
            .then_with(|| a.1.detail.cmp(&b.1.detail))
    });
    hits.dedup_by(|a, b| a.1 == b.1);
    hits.into_iter().map(|(_, c)| c).collect()
}

/// The relation a reference names: in its schema when it gives one, else on the
/// current search path.
fn resolve<'a>(cache: &'a SchemaCache, r: &Reference) -> Option<&'a Relation> {
    r.schema
        .as_deref()
        .and_then(|s| cache.find_in(s, &r.name))
        .or_else(|| cache.find(&r.name))
}

/// Relations whose columns are in scope at the cursor.
fn scope<'a>(cache: &'a SchemaCache, ctx: &Context) -> Vec<&'a Relation> {
    if let Some(q) = &ctx.qualifier {
        // `schema.table.`
        if let Some(schema) = &ctx.outer_qualifier {
            return cache.find_in(schema, q).into_iter().collect();
        }
        let by_alias = ctx.references.iter().find(|r| {
            r.alias
                .as_deref()
                .is_some_and(|a| a.eq_ignore_ascii_case(q))
        });
        return match by_alias {
            Some(r) => resolve(cache, r),
            None => cache.find(q),
        }
        .into_iter()
        .collect();
    }
    let mut found: Vec<&Relation> = Vec::new();
    for r in ctx.references.iter().filter_map(|r| resolve(cache, r)) {
        if !found.iter().any(|f| std::ptr::eq(*f, r)) {
            found.push(r);
        }
    }
    if found.is_empty() {
        // Nothing resolved: better every column than none.
        found = cache.relations.iter().collect();
    }
    found
}

/// Candidates for the fragment before `pos`, with the fragment's start offset.
/// Analysis and keywords follow the cache's server.
pub fn candidates(cache: &SchemaCache, line: &str, pos: usize) -> (usize, Vec<Candidate>) {
    let server = cache.dialect();
    let ctx = analyze_with(line, pos, server);
    let prefix: Vec<char> = ctx
        .prefix
        .chars()
        .map(|c| c.to_lowercase().next().unwrap_or(c))
        .collect();
    let mut out = Vec::new();
    for expect in &ctx.expect {
        match expect {
            Expect::Relations => {
                let relations = cache.relations.iter().map(|r| {
                    let (kind, label) = match r.kind {
                        RelationKind::Table => (Kind::Table, "table"),
                        RelationKind::View => (Kind::View, "view"),
                    };
                    (r.name.as_str(), kind, label.to_string())
                });
                // Schemas other than the default one, to qualify a table with.
                let schemas = cache
                    .qualified
                    .iter()
                    .filter(|(s, _)| server.qualifier() == Qualifier::Schema && s != "public")
                    .map(|(s, _)| (s.as_str(), Kind::Schema, "schema".to_string()));
                let mut hits = rank(&prefix, relations.chain(schemas));
                // With nothing typed, schemas lead so a long table list cannot
                // push them past the candidate limit.
                if prefix.is_empty() {
                    hits.sort_by_key(|c| c.kind != Kind::Schema);
                }
                out.extend(hits);
            }
            // `db.` or `schema.`: that database's or schema's tables, unless the
            // name is also a relation or alias in scope.
            Expect::Columns
                if server.qualifier() != Qualifier::None
                    && ctx.outer_qualifier.is_none()
                    && scope(cache, &ctx).is_empty()
                    && ctx
                        .qualifier
                        .as_deref()
                        .is_some_and(|q| cache.has_qualifier(q)) =>
            {
                let q = ctx.qualifier.as_deref().unwrap_or_default();
                out.extend(rank(
                    &prefix,
                    cache
                        .qualified
                        .iter()
                        .filter(|(db, _)| db.eq_ignore_ascii_case(q))
                        .map(|(db, t)| (t.as_str(), Kind::Table, format!("table ({db})"))),
                ));
            }
            Expect::Databases => out.extend(rank(
                &prefix,
                cache
                    .databases
                    .iter()
                    .map(|d| (d.as_str(), Kind::Database, "database".to_string())),
            )),
            Expect::Columns => out.extend(rank(
                &prefix,
                scope(cache, &ctx).into_iter().flat_map(|r| {
                    r.columns.iter().map(move |c| {
                        let detail = if c.type_name.is_empty() {
                            format!("column ({})", r.name)
                        } else {
                            format!("column {} ({})", c.type_name, r.name)
                        };
                        (c.name.as_str(), Kind::Column, detail)
                    })
                }),
            )),
            Expect::Keywords if !prefix.is_empty() => {
                let lower = ctx.prefix.chars().last().is_some_and(char::is_lowercase);
                let mut hits = rank(
                    &prefix,
                    server
                        .keywords()
                        .iter()
                        .filter(|k| k.to_lowercase().starts_with(&ctx.prefix.to_lowercase()))
                        .map(|k| (*k, Kind::Keyword, "keyword".to_string())),
                );
                if lower {
                    for h in &mut hits {
                        h.value = h.value.to_lowercase();
                    }
                }
                out.extend(hits);
            }
            Expect::Keywords => {}
            Expect::Formats => out.extend(rank(
                &prefix,
                Format::NAMES
                    .iter()
                    .map(|n| (*n, Kind::Format, "format".to_string())),
            )),
            Expect::Commands => out.extend(rank(
                &prefix,
                symbolic_names()
                    .filter(|n| n.starts_with(ctx.prefix.as_str()))
                    .map(|n| (n, Kind::Command, "command".to_string())),
            )),
        }
    }
    out.truncate(MAX_CANDIDATES);
    (ctx.start, out)
}

pub struct SqlCompleter {
    schema: Arc<RwLock<SchemaCache>>,
    after_space: Arc<AtomicBool>,
}

impl SqlCompleter {
    /// `after_space` is set while the last key typed was a space.
    pub fn new(schema: Arc<RwLock<SchemaCache>>, after_space: Arc<AtomicBool>) -> Self {
        Self {
            schema,
            after_space,
        }
    }
}

impl Completer for SqlCompleter {
    fn complete(&mut self, line: &str, pos: usize) -> CompletionResult {
        let cache = self.schema.read().unwrap();
        let (start, mut found) = candidates(&cache, line, pos);
        // A space opens the menu only for `\T`; elsewhere it waits for Tab or
        // the next word.
        if self.after_space.load(Ordering::Relaxed) {
            found.retain(|c| c.kind == Kind::Format);
        }
        // Names are padded so the types line up in a column.
        let width = found.iter().map(|c| c.value.chars().count()).max();
        let width = width.unwrap_or(0);
        let suggestions: Vec<Suggestion> = found
            .into_iter()
            .map(|c| Suggestion {
                display_override: Some(format!("{:width$}  {}", c.value, c.kind.label())),
                value: c.value,
                description: Some(c.detail),
                match_indices: (!c.matched.is_empty()).then_some(c.matched),
                span: Span::new(start, pos),
                ..Default::default()
            })
            .collect();
        CompletionResult::fresh(suggestions)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::client::ColumnInfo;

    fn rel(name: &str, kind: RelationKind, cols: &[(&str, &str)]) -> Relation {
        Relation {
            schema: None,
            name: name.into(),
            kind,
            columns: cols
                .iter()
                .map(|(n, t)| ColumnInfo {
                    name: (*n).into(),
                    type_name: (*t).into(),
                    not_null: false,
                    default: None,
                    primary_key: false,
                })
                .collect(),
        }
    }

    fn cache() -> SchemaCache {
        SchemaCache {
            relations: vec![
                rel(
                    "customers",
                    RelationKind::Table,
                    &[("id", "INTEGER"), ("email", "TEXT")],
                ),
                rel(
                    "orders",
                    RelationKind::Table,
                    &[
                        ("id", "INTEGER"),
                        ("total", "REAL"),
                        ("subtotal", "REAL"),
                        ("order_total", "REAL"),
                    ],
                ),
                rel("big_orders", RelationKind::View, &[("id", "INTEGER")]),
            ],
            server: Some(Server::Sqlite),
            ..Default::default()
        }
    }

    fn mysql_cache() -> SchemaCache {
        SchemaCache {
            server: Some(Server::MySql),
            relations: vec![rel("orders", RelationKind::Table, &[("id", "int")])],
            databases: vec!["crm".into(), "shop".into()],
            qualified: vec![
                ("crm".into(), "contacts".into()),
                ("crm".into(), "deals".into()),
                ("shop".into(), "orders".into()),
            ],
            schema_relations: Vec::new(),
        }
    }

    /// A cache whose `crm` qualifier has fully known relations, as a
    /// schema-qualifying server provides.
    fn schema_cache() -> SchemaCache {
        let mut contacts = rel("contacts", RelationKind::Table, &[("email", "text")]);
        contacts.schema = Some("crm".into());
        let mut c = mysql_cache();
        c.schema_relations = vec![contacts];
        c
    }

    fn values_in(cache: &SchemaCache, text: &str) -> Vec<String> {
        let pos = text.find('|').expect("cursor marker");
        let line = text.replacen('|', "", 1);
        candidates(cache, &line, pos)
            .1
            .into_iter()
            .map(|c| c.value)
            .collect()
    }

    #[test]
    fn keywords_follow_the_server() {
        assert!(values_in(&mysql_cache(), "sho|").contains(&"show".to_string()));
        assert!(!values("sho|").contains(&"show".to_string()));
        assert!(!values_in(&mysql_cache(), "prag|").contains(&"pragma".to_string()));
        assert!(values("prag|").contains(&"pragma".to_string()));
    }

    #[test]
    fn database_dot_offers_its_tables_on_mysql() {
        let c = mysql_cache();
        assert_eq!(values_in(&c, "select * from crm.|"), ["contacts", "deals"]);
        assert_eq!(values_in(&c, "select * from crm.co|"), ["contacts"]);
        // An alias or table named like a database still means that relation.
        assert_eq!(values_in(&c, "select crm.| from orders crm"), ["id"]);
        // Unknown qualifier: nothing.
        assert!(values_in(&c, "select * from nope.|").is_empty());
        // SQLite has no `db.` qualifiers.
        let mut sqlite = c.clone();
        sqlite.server = Some(Server::Sqlite);
        assert!(values_in(&sqlite, "select * from crm.|").is_empty());
    }

    #[test]
    fn use_offers_databases() {
        let c = mysql_cache();
        assert_eq!(values_in(&c, "use c|"), ["crm"]);
        assert_eq!(values_in(&c, "use |"), ["crm", "shop"]);
    }

    #[test]
    fn disconnected_offers_only_keywords_and_commands() {
        let c = SchemaCache::default();
        assert!(values_in(&c, "select * from o|").is_empty());
        assert!(values_in(&c, "sel|").contains(&"select".to_string()));
        assert!(values_in(&c, "\\con|").contains(&"\\connect".to_string()));
    }

    /// Candidate values at the `|` cursor marker.
    fn at(text: &str) -> (usize, Vec<Candidate>) {
        let pos = text.find('|').expect("cursor marker");
        let line = text.replacen('|', "", 1);
        candidates(&cache(), &line, pos)
    }

    fn values(text: &str) -> Vec<String> {
        at(text).1.into_iter().map(|c| c.value).collect()
    }

    #[test]
    fn keyword_casing_follows_input() {
        assert_eq!(values("sel|"), vec!["select"]);
        assert_eq!(values("SEL|"), vec!["SELECT"]);
        assert_eq!(values("Sel|"), vec!["select"]);
    }

    #[test]
    fn tables_after_from_without_keywords_or_columns() {
        let (start, c) = at("select * from c|");
        assert_eq!(start, 14);
        assert_eq!(c.len(), 1);
        assert_eq!((c[0].value.as_str(), c[0].kind), ("customers", Kind::Table));
    }

    #[test]
    fn views_are_offered_as_views() {
        let c = at("select * from big|").1;
        assert_eq!((c[0].value.as_str(), c[0].kind), ("big_orders", Kind::View));
    }

    #[test]
    fn empty_prefix_lists_relations_after_from() {
        assert_eq!(
            values("select * from |"),
            vec!["big_orders", "customers", "orders"]
        );
    }

    #[test]
    fn no_keywords_for_empty_prefix() {
        assert!(values("select * from orders |").is_empty());
        assert!(values("|").is_empty());
    }

    #[test]
    fn columns_in_where() {
        assert_eq!(
            values("select * from orders where t|")
                .into_iter()
                .filter(|v| v == "total")
                .count(),
            1
        );
    }

    #[test]
    fn only_in_scope_columns() {
        let v = values("select e| from orders");
        assert!(!v.contains(&"email".to_string()), "{v:?}");
        let v = values("select e| from customers");
        assert!(v.contains(&"email".to_string()), "{v:?}");
    }

    #[test]
    fn table_after_cursor_is_in_scope() {
        let mut v = values("select | from orders");
        v.retain(|c| c != "id");
        assert_eq!(v, vec!["order_total", "subtotal", "total"]);
    }

    #[test]
    fn unknown_tables_fall_back_to_all_columns() {
        let v = values("select ordtot| from t");
        assert_eq!(v, vec!["order_total"]);
    }

    #[test]
    fn alias_dot_offers_that_relations_columns() {
        let v = values("select o.| from orders o");
        assert_eq!(v, vec!["id", "order_total", "subtotal", "total"]);
        let v = values("select c.| from orders o join customers as c on 1");
        assert_eq!(v, vec!["email", "id"]);
    }

    #[test]
    fn table_name_dot_and_replacement_span() {
        let (start, c) = at("select orders.to| from orders");
        assert_eq!(start, 14);
        assert_eq!(c[0].value, "total");
    }

    #[test]
    fn unknown_qualifier_offers_nothing() {
        assert!(values("select x.|").is_empty());
        assert!(values("select x.tot|").is_empty());
    }

    #[test]
    fn fuzzy_subsequence_and_prefix_first() {
        assert_eq!(values("select ordtot| from orders"), vec!["order_total"]);
        let v = values("select tot| from orders");
        let pos = |n: &str| v.iter().position(|x| x == n).unwrap();
        assert!(pos("total") < pos("subtotal"), "{v:?}");
        assert!(pos("total") < pos("order_total"), "{v:?}");
    }

    #[test]
    fn fuzzy_records_matched_positions() {
        let c = at("select ordtot| from orders").1;
        assert_eq!(c[0].matched, vec![0, 1, 2, 6, 7, 8]);
    }

    #[test]
    fn completing_a_word_to_itself_is_skipped() {
        assert!(!values("select * from orders|").contains(&"orders".to_string()));
    }

    #[test]
    fn completes_special_commands() {
        let (start, c) = at(".tab|");
        assert_eq!(start, 0);
        assert_eq!(
            c.iter().map(|c| c.value.as_str()).collect::<Vec<_>>(),
            vec![".tables"]
        );
        assert!(c.iter().all(|c| c.kind == Kind::Command));
        let v = values("\\d|");
        assert!(v.contains(&"\\dt".to_string()) && v.contains(&"\\di".to_string()));
    }

    #[test]
    fn column_detail_has_kind_type_and_relation() {
        let c = at("select o.to| from orders o").1;
        assert_eq!(c[0].kind, Kind::Column);
        assert_eq!(c[0].detail, "column REAL (orders)");
        let c = at("select * from cus|").1;
        assert_eq!(c[0].detail, "table");
        assert_eq!(at("sel|").1[0].detail, "keyword");
    }

    #[test]
    fn nothing_inside_string_or_comment() {
        assert!(values("select 'ord|").is_empty());
        assert!(values("select 1 -- ord|").is_empty());
    }

    #[test]
    fn suggestion_carries_span_and_description() {
        let schema = Arc::new(RwLock::new(cache()));
        let mut comp = SqlCompleter::new(schema, Arc::default());
        let r = comp.complete("select * from ord", 17);
        let s = r.suggestions();
        assert_eq!(s[0].value, "orders");
        assert_eq!((s[0].span.start, s[0].span.end), (14, 17));
        assert_eq!(s[0].description.as_deref(), Some("table"));
    }

    #[test]
    fn every_row_shows_its_type_in_a_column() {
        let mut comp = SqlCompleter::new(Arc::new(RwLock::new(cache())), Arc::default());
        let r = comp.complete("select * from ", 14);
        let rows: Vec<_> = r.suggestions().iter().map(|s| s.display_value()).collect();
        assert_eq!(
            rows,
            ["big_orders  view", "customers   table", "orders      table"]
        );
        // Accepting a row inserts the name alone.
        assert_eq!(r.suggestions()[0].value, "big_orders");
    }

    #[test]
    fn format_command_offers_every_format() {
        let mut all = Format::NAMES.to_vec();
        all.sort_unstable();
        assert_eq!(values("\\T |"), all);
        assert_eq!(values("\\T js|"), ["json"]);
        assert!(values("\\T json|").is_empty());
    }

    #[test]
    fn space_offers_only_formats() {
        let after_space = Arc::new(AtomicBool::new(true));
        let mut comp = SqlCompleter::new(Arc::new(RwLock::new(cache())), after_space);
        assert!(comp.complete("select * from ", 14).suggestions().is_empty());
        assert_eq!(
            comp.complete("\\T ", 3).suggestions().len(),
            Format::NAMES.len()
        );
    }

    #[test]
    fn schema_table_dot_offers_columns() {
        let c = schema_cache();
        assert_eq!(
            values_in(&c, "select crm.contacts.| from crm.contacts"),
            ["email"]
        );
        assert!(values_in(&c, "select crm.nope.|").is_empty());
    }

    #[test]
    fn alias_of_schema_qualified_table() {
        let c = schema_cache();
        assert_eq!(values_in(&c, "select k.| from crm.contacts k"), ["email"]);
    }

    #[test]
    fn alias_dot_and_database_dot_unchanged() {
        assert_eq!(
            values_in(&mysql_cache(), "select o.| from orders o"),
            ["id"]
        );
        assert_eq!(
            values_in(&mysql_cache(), "select * from crm.|"),
            ["contacts", "deals"]
        );
    }

    #[test]
    fn has_qualifier_follows_the_server() {
        assert!(mysql_cache().has_qualifier("CRM"));
        assert!(!mysql_cache().has_qualifier("nope"));
        assert!(!cache().has_qualifier("crm"));
    }

    fn psql_cache() -> SchemaCache {
        let mut invoices = rel("invoices", RelationKind::Table, &[("total", "numeric")]);
        invoices.schema = Some("sales".into());
        SchemaCache {
            server: Some(Server::PostgreSql),
            relations: vec![rel("orders", RelationKind::Table, &[("id", "integer")])],
            qualified: vec![
                ("public".into(), "orders".into()),
                ("sales".into(), "invoices".into()),
            ],
            schema_relations: vec![invoices],
            ..Default::default()
        }
    }

    #[test]
    fn schema_dot_and_schema_table_dot_on_postgres() {
        let c = psql_cache();
        assert_eq!(values_in(&c, "select * from sales.|"), ["invoices"]);
        assert_eq!(
            values_in(&c, "select sales.invoices.| from sales.invoices"),
            ["total"]
        );
        assert_eq!(values_in(&c, "select i.| from sales.invoices i"), ["total"]);
        assert!(values_in(&c, "select x.|").is_empty());
        assert!(values_in(&c, "retur|").contains(&"returning".to_string()));
        assert!(!values_in(&c, "prag|").contains(&"pragma".to_string()));
    }

    #[test]
    fn schemas_are_offered_where_a_table_goes_on_postgres() {
        let mut c = psql_cache();
        c.qualified.push(("sales".into(), "refunds".into()));
        // Once each, leading, and without the default schema.
        assert_eq!(values_in(&c, "select * from |"), ["sales", "orders"]);
        assert_eq!(values_in(&c, "select * from sa|"), ["sales"]);
        assert_eq!(values_in(&c, "select * from orders join sa|"), ["sales"]);
        let found = candidates(&c, "select * from sa", 16).1;
        assert_eq!(
            (found[0].kind, found[0].detail.as_str()),
            (Kind::Schema, "schema")
        );
        // Only the default schema: nothing extra.
        c.qualified.retain(|(s, _)| s == "public");
        assert_eq!(values_in(&c, "select * from |"), ["orders"]);
        // MySQL qualifiers are databases, not schemas.
        assert_eq!(values_in(&mysql_cache(), "select * from |"), ["orders"]);
    }
}

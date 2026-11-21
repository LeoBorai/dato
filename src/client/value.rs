//! Backend-neutral value and result types.

#[derive(Debug, Clone, PartialEq)]
pub enum Value {
    Null,
    Integer(i64),
    Real(f64),
    /// Exact numeric text (`DECIMAL`, integers beyond `i64`), kept digit for digit.
    Numeric(String),
    Text(String),
    Blob(Vec<u8>),
}

impl Value {
    pub fn is_numeric(&self) -> bool {
        matches!(self, Value::Integer(_) | Value::Real(_) | Value::Numeric(_))
    }
}

/// What the server reports about a result column, for rendering.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct ColumnMeta {
    /// Declared type as the server names it; `None` when unknown (expressions).
    pub type_name: Option<String>,
    /// The column belongs to its table's primary key.
    pub primary_key: bool,
    /// The column holds booleans; values keep the engine's own display.
    pub boolean: bool,
}

#[derive(Debug, Clone, PartialEq, Default)]
pub struct ResultSet {
    pub columns: Vec<String>,
    pub rows: Vec<Vec<Value>>,
    /// One entry per column, or empty when the source has no type information.
    pub meta: Vec<ColumnMeta>,
}

impl ResultSet {
    pub fn new(columns: &[&str]) -> Self {
        Self {
            columns: columns.iter().map(|c| c.to_string()).collect(),
            rows: Vec::new(),
            meta: Vec::new(),
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub enum StatementResult {
    Rows(ResultSet),
    Affected(u64),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RelationKind {
    Table,
    View,
}

/// A table or view with its columns.
#[derive(Debug, Clone, PartialEq)]
pub struct Relation {
    /// Schema the relation lives in, for servers that qualify names by schema.
    pub schema: Option<String>,
    pub name: String,
    pub kind: RelationKind,
    pub columns: Vec<ColumnInfo>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct ColumnInfo {
    pub name: String,
    pub type_name: String,
    pub not_null: bool,
    pub default: Option<String>,
    pub primary_key: bool,
}

/// A `CHECK` constraint. `column` is set when the server ties it to one column.
#[derive(Debug, Clone, PartialEq)]
pub struct Check {
    pub column: Option<String>,
    pub expr: String,
}

/// One column of a foreign key and the column it references.
#[derive(Debug, Clone, PartialEq)]
pub struct ForeignKey {
    pub column: String,
    pub ref_table: String,
    pub ref_column: String,
}

/// Constraints of a table that [`ColumnInfo`] does not carry.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Constraints {
    pub checks: Vec<Check>,
    pub foreign_keys: Vec<ForeignKey>,
}

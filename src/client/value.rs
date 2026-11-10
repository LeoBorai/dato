//! Backend-neutral value and result types.

#[derive(Debug, Clone, PartialEq)]
pub enum Value {
    Null,
    Integer(i64),
    Real(f64),
    Text(String),
    Blob(Vec<u8>),
}

impl Value {
    pub fn is_numeric(&self) -> bool {
        matches!(self, Value::Integer(_) | Value::Real(_))
    }
}

#[derive(Debug, Clone, PartialEq, Default)]
pub struct ResultSet {
    pub columns: Vec<String>,
    pub rows: Vec<Vec<Value>>,
}

impl ResultSet {
    pub fn new(columns: &[&str]) -> Self {
        Self {
            columns: columns.iter().map(|c| c.to_string()).collect(),
            rows: Vec::new(),
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub enum StatementResult {
    Rows(ResultSet),
    Affected(u64),
}

#[derive(Debug, Clone, PartialEq)]
pub struct ColumnInfo {
    pub name: String,
    pub type_name: String,
    pub not_null: bool,
    pub default: Option<String>,
    pub primary_key: bool,
}

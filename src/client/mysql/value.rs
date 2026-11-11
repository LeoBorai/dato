//! MySQL column values to backend-neutral [`Value`]s, without losing precision.

use mysql::Value as My;
use mysql::consts::ColumnType as T;

use crate::client::Value;

/// MySQL's `binary` character set: string-typed columns with it hold bytes.
const BINARY_CHARSET: u16 = 63;

/// How a column's values are mapped.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    /// Exact integer: `i64` when it fits, else exact text.
    Integer,
    /// `DECIMAL`: exact text.
    Decimal,
    Float,
    /// Strings, dates, times, JSON: the server's text form.
    Text,
    Binary,
}

pub fn kind_of(column_type: T, charset: u16) -> Kind {
    match column_type {
        T::MYSQL_TYPE_TINY
        | T::MYSQL_TYPE_SHORT
        | T::MYSQL_TYPE_INT24
        | T::MYSQL_TYPE_LONG
        | T::MYSQL_TYPE_LONGLONG
        | T::MYSQL_TYPE_YEAR => Kind::Integer,
        T::MYSQL_TYPE_DECIMAL | T::MYSQL_TYPE_NEWDECIMAL => Kind::Decimal,
        T::MYSQL_TYPE_FLOAT | T::MYSQL_TYPE_DOUBLE => Kind::Float,
        T::MYSQL_TYPE_BIT | T::MYSQL_TYPE_GEOMETRY => Kind::Binary,
        T::MYSQL_TYPE_VARCHAR
        | T::MYSQL_TYPE_VAR_STRING
        | T::MYSQL_TYPE_STRING
        | T::MYSQL_TYPE_TINY_BLOB
        | T::MYSQL_TYPE_MEDIUM_BLOB
        | T::MYSQL_TYPE_LONG_BLOB
        | T::MYSQL_TYPE_BLOB
            if charset == BINARY_CHARSET =>
        {
            Kind::Binary
        }
        _ => Kind::Text,
    }
}

fn text(bytes: Vec<u8>) -> String {
    String::from_utf8(bytes).unwrap_or_else(|e| String::from_utf8_lossy(e.as_bytes()).into_owned())
}

fn integer(digits: String) -> Value {
    match digits.parse::<i64>() {
        Ok(i) => Value::Integer(i),
        Err(_) => Value::Numeric(digits),
    }
}

/// Converts one value of a column of the given kind. Handles both the text
/// protocol (everything arrives as bytes) and binary-protocol typed values.
pub fn convert(kind: Kind, v: My) -> Value {
    match v {
        My::NULL => Value::Null,
        My::Int(i) => Value::Integer(i),
        My::UInt(u) => integer(u.to_string()),
        My::Float(f) => Value::Real(f64::from(f)),
        My::Double(d) => Value::Real(d),
        My::Bytes(b) => match kind {
            Kind::Binary => Value::Blob(b),
            Kind::Integer => integer(text(b)),
            Kind::Decimal => Value::Numeric(text(b)),
            Kind::Float => {
                let t = text(b);
                t.parse::<f64>().map_or(Value::Text(t), Value::Real)
            }
            Kind::Text => Value::Text(text(b)),
        },
        // Binary-protocol temporal values: render as the server would.
        other @ (My::Date(..) | My::Time(..)) => {
            Value::Text(other.as_sql(true).trim_matches('\'').to_string())
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn bytes(kind: Kind, s: &str) -> Value {
        convert(kind, My::Bytes(s.as_bytes().to_vec()))
    }

    #[test]
    fn kinds_by_column_type() {
        assert_eq!(
            kind_of(T::MYSQL_TYPE_LONGLONG, BINARY_CHARSET),
            Kind::Integer
        );
        assert_eq!(
            kind_of(T::MYSQL_TYPE_NEWDECIMAL, BINARY_CHARSET),
            Kind::Decimal
        );
        assert_eq!(kind_of(T::MYSQL_TYPE_DOUBLE, BINARY_CHARSET), Kind::Float);
        assert_eq!(kind_of(T::MYSQL_TYPE_VAR_STRING, 255), Kind::Text);
        assert_eq!(kind_of(T::MYSQL_TYPE_BLOB, 255), Kind::Text);
        assert_eq!(kind_of(T::MYSQL_TYPE_BLOB, BINARY_CHARSET), Kind::Binary);
        assert_eq!(kind_of(T::MYSQL_TYPE_DATETIME, BINARY_CHARSET), Kind::Text);
        assert_eq!(kind_of(T::MYSQL_TYPE_JSON, BINARY_CHARSET), Kind::Text);
    }

    #[test]
    fn exact_numbers() {
        assert_eq!(bytes(Kind::Integer, "42"), Value::Integer(42));
        assert_eq!(
            bytes(Kind::Integer, "-9223372036854775808"),
            Value::Integer(i64::MIN)
        );
        assert_eq!(
            bytes(Kind::Integer, "18446744073709551615"),
            Value::Numeric("18446744073709551615".into())
        );
        assert_eq!(
            convert(Kind::Integer, My::UInt(u64::MAX)),
            Value::Numeric("18446744073709551615".into())
        );
        assert_eq!(convert(Kind::Integer, My::UInt(7)), Value::Integer(7));
        assert_eq!(bytes(Kind::Decimal, "1.10"), Value::Numeric("1.10".into()));
        assert_eq!(bytes(Kind::Float, "2.5"), Value::Real(2.5));
    }

    #[test]
    fn text_binary_and_null() {
        assert_eq!(
            bytes(Kind::Text, "2026-01-02 03:04:05"),
            Value::Text("2026-01-02 03:04:05".into())
        );
        assert_eq!(
            convert(Kind::Binary, My::Bytes(vec![0, 255])),
            Value::Blob(vec![0, 255])
        );
        assert_eq!(convert(Kind::Text, My::NULL), Value::Null);
        assert_eq!(
            convert(Kind::Text, My::Date(2026, 1, 2, 3, 4, 5, 0)),
            Value::Text("2026-01-02 03:04:05".into())
        );
    }
}

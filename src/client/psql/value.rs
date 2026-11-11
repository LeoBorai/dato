//! PostgreSQL text values to result values, by column type.
//!
//! Rows come back from the simple query protocol as server-formatted text, the
//! way `psql` prints them. The column type decides only what a value becomes:
//! numbers stay numbers (right-aligned, exact), `bytea` becomes binary, and
//! everything else, booleans (`t`/`f`) included, is kept as the server's text.

use postgres::types::Type;

use crate::client::Value;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    Integer,
    Numeric,
    Float,
    Bytea,
    Text,
}

pub fn kind_of(ty: &Type) -> Kind {
    match *ty {
        Type::INT2 | Type::INT4 | Type::INT8 | Type::OID => Kind::Integer,
        Type::NUMERIC => Kind::Numeric,
        Type::FLOAT4 | Type::FLOAT8 => Kind::Float,
        Type::BYTEA => Kind::Bytea,
        _ => Kind::Text,
    }
}

/// `\x48656c6c6f` to bytes; `None` when `s` is not hex output.
fn decode_hex(s: &str) -> Option<Vec<u8>> {
    let hex = s.strip_prefix("\\x")?;
    if hex.len() % 2 != 0 {
        return None;
    }
    hex.as_bytes()
        .chunks(2)
        .map(|p| {
            let hi = (p[0] as char).to_digit(16)?;
            let lo = (p[1] as char).to_digit(16)?;
            Some((hi * 16 + lo) as u8)
        })
        .collect()
}

pub fn convert(kind: Kind, text: Option<&str>) -> Value {
    let Some(t) = text else {
        return Value::Null;
    };
    match kind {
        Kind::Integer => t
            .parse::<i64>()
            .map_or_else(|_| Value::Numeric(t.to_string()), Value::Integer),
        // `NaN` is a valid numeric value but not a number to align; keep its text.
        Kind::Numeric if t.eq_ignore_ascii_case("nan") => Value::Text(t.to_string()),
        Kind::Numeric => Value::Numeric(t.to_string()),
        Kind::Float => t
            .parse::<f64>()
            .map_or_else(|_| Value::Text(t.to_string()), Value::Real),
        Kind::Bytea => decode_hex(t).map_or_else(|| Value::Text(t.to_string()), Value::Blob),
        Kind::Text => Value::Text(t.to_string()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn kinds_by_type() {
        assert_eq!(kind_of(&Type::INT4), Kind::Integer);
        assert_eq!(kind_of(&Type::INT8), Kind::Integer);
        assert_eq!(kind_of(&Type::NUMERIC), Kind::Numeric);
        assert_eq!(kind_of(&Type::FLOAT8), Kind::Float);
        assert_eq!(kind_of(&Type::BYTEA), Kind::Bytea);
        for ty in [
            Type::BOOL,
            Type::TEXT,
            Type::TIMESTAMPTZ,
            Type::JSONB,
            Type::UUID,
            Type::INT4_ARRAY,
        ] {
            assert_eq!(kind_of(&ty), Kind::Text, "{ty}");
        }
    }

    #[test]
    fn integers_keep_digits() {
        assert_eq!(convert(Kind::Integer, Some("42")), Value::Integer(42));
        assert_eq!(
            convert(Kind::Integer, Some("9223372036854775807")),
            Value::Integer(i64::MAX)
        );
        assert_eq!(
            convert(Kind::Integer, Some("18446744073709551615")),
            Value::Numeric("18446744073709551615".into())
        );
    }

    #[test]
    fn numeric_keeps_scale() {
        assert_eq!(
            convert(Kind::Numeric, Some("1.10")),
            Value::Numeric("1.10".into())
        );
        assert_eq!(
            convert(Kind::Numeric, Some("NaN")),
            Value::Text("NaN".into())
        );
    }

    #[test]
    fn floats_bytea_text_and_null() {
        assert_eq!(convert(Kind::Float, Some("1.5")), Value::Real(1.5));
        assert!(
            matches!(convert(Kind::Float, Some("Infinity")), Value::Real(f) if f.is_infinite())
        );
        assert_eq!(
            convert(Kind::Bytea, Some("\\x48690a")),
            Value::Blob(b"Hi\n".to_vec())
        );
        assert_eq!(convert(Kind::Bytea, Some("\\x")), Value::Blob(Vec::new()));
        assert_eq!(convert(Kind::Text, Some("t")), Value::Text("t".into()));
        assert_eq!(
            convert(Kind::Text, Some("{1,2,3}")),
            Value::Text("{1,2,3}".into())
        );
        for k in [
            Kind::Integer,
            Kind::Numeric,
            Kind::Float,
            Kind::Bytea,
            Kind::Text,
        ] {
            assert_eq!(convert(k, None), Value::Null);
        }
    }
}

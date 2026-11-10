//! `json` format: an array with one object per row.

use std::io::{self, Write};

use serde_json::{Map, Number, Value as Json};

use super::hex;
use crate::client::{ResultSet, Value};

fn to_json(v: &Value) -> Json {
    match v {
        Value::Null => Json::Null,
        Value::Integer(i) => Json::from(*i),
        Value::Real(r) => Number::from_f64(*r).map_or(Json::Null, Json::Number),
        Value::Text(t) => Json::String(t.clone()),
        Value::Blob(b) => Json::String(format!("x'{}'", hex(b))),
    }
}

pub fn render(rs: &ResultSet, out: &mut dyn Write) -> io::Result<()> {
    let rows: Vec<Json> = rs
        .rows
        .iter()
        .map(|row| {
            let mut m = Map::new();
            for (c, v) in rs.columns.iter().zip(row) {
                m.insert(c.clone(), to_json(v));
            }
            Json::Object(m)
        })
        .collect();
    serde_json::to_writer(&mut *out, &Json::Array(rows))?;
    writeln!(out)
}

#[cfg(test)]
mod tests {
    use super::super::{Format, render_to_string};
    use crate::client::{ResultSet, Value};

    #[test]
    fn json_rows() {
        let r = ResultSet {
            columns: vec!["a".into(), "b".into(), "c".into(), "d".into()],
            rows: vec![vec![
                Value::Integer(1),
                Value::Null,
                Value::Real(2.5),
                Value::Blob(vec![1, 2]),
            ]],
        };
        assert_eq!(
            render_to_string(&r, Format::Json),
            "[{\"a\":1,\"b\":null,\"c\":2.5,\"d\":\"x'0102'\"}]\n"
        );
    }

    #[test]
    fn json_empty() {
        let r = ResultSet::new(&["a"]);
        assert_eq!(render_to_string(&r, Format::Json), "[]\n");
    }
}

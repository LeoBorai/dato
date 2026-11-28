//! `json` format: a pretty-printed array with one object per row.

use std::io::{self, Write};

use nu_ansi_term::{Color, Style};
use serde_json::{Map, Number, Value as Json};

use super::hex;
use crate::client::{ResultSet, Value};

fn to_json(v: &Value) -> Json {
    match v {
        Value::Null => Json::Null,
        Value::Integer(i) => Json::from(*i),
        Value::Real(r) => Number::from_f64(*r).map_or(Json::Null, Json::Number),
        Value::Numeric(n) => n
            .parse::<Number>()
            .map_or_else(|_| Json::String(n.clone()), Json::Number),
        Value::Text(t) => Json::String(t.clone()),
        Value::Blob(b) => Json::String(format!("x'{}'", hex(b))),
    }
}

/// Paints pretty-printed JSON: keys, strings, numbers, booleans and nulls.
/// Expects valid JSON, as written by `serde_json`.
fn highlight(json: &str, out: &mut dyn Write) -> io::Result<()> {
    let bytes = json.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        let start = i;
        let style = match bytes[i] {
            b'"' => {
                i += 1;
                while i < bytes.len() && bytes[i] != b'"' {
                    i += if bytes[i] == b'\\' { 2 } else { 1 };
                }
                i += 1;
                Some(if bytes.get(i) == Some(&b':') {
                    Style::new().fg(Color::Cyan)
                } else {
                    Style::new().fg(Color::Green)
                })
            }
            b'-' | b'0'..=b'9' => {
                while i < bytes.len()
                    && matches!(bytes[i], b'-' | b'+' | b'.' | b'e' | b'E' | b'0'..=b'9')
                {
                    i += 1;
                }
                Some(Style::new().fg(Color::Purple))
            }
            b't' | b'f' | b'n' => {
                while i < bytes.len() && bytes[i].is_ascii_lowercase() {
                    i += 1;
                }
                Some(if bytes[start] == b'n' {
                    Style::new().fg(Color::DarkGray)
                } else {
                    Style::new().bold().fg(Color::Blue)
                })
            }
            _ => {
                // Punctuation and whitespace, all single ASCII bytes.
                i += 1;
                None
            }
        };
        let text = &json[start..i];
        match style {
            Some(s) => write!(out, "{}", s.paint(text))?,
            None => out.write_all(text.as_bytes())?,
        }
    }
    Ok(())
}

pub fn render(rs: &ResultSet, colors: bool, out: &mut dyn Write) -> io::Result<()> {
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
    let json = serde_json::to_string_pretty(&Json::Array(rows))?;
    if colors {
        highlight(&json, out)?;
    } else {
        out.write_all(json.as_bytes())?;
    }
    writeln!(out)
}

#[cfg(test)]
mod tests {
    use super::super::{Format, render_to_string, render_to_string_with};
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
            ..Default::default()
        };
        assert_eq!(
            render_to_string(&r, Format::Json),
            concat!(
                "[\n  {\n    \"a\": 1,\n    \"b\": null,\n",
                "    \"c\": 2.5,\n    \"d\": \"x'0102'\"\n  }\n]\n"
            )
        );
    }

    #[test]
    fn json_numeric_keeps_exact_digits() {
        let r = ResultSet {
            columns: vec!["u".into(), "d".into(), "x".into()],
            rows: vec![vec![
                Value::Numeric("18446744073709551615".into()),
                Value::Numeric("1.10".into()),
                Value::Numeric("not-a-number".into()),
            ]],
            ..Default::default()
        };
        assert_eq!(
            render_to_string(&r, Format::Json),
            concat!(
                "[\n  {\n    \"u\": 18446744073709551615,\n",
                "    \"d\": 1.10,\n    \"x\": \"not-a-number\"\n  }\n]\n"
            )
        );
    }

    #[test]
    fn json_colors_leave_the_text_intact() {
        let r = ResultSet {
            columns: vec!["k\"ey".into(), "n".into(), "z".into()],
            rows: vec![vec![
                Value::Text("a \\ \"b\": é".into()),
                Value::Real(-1.5e30),
                Value::Null,
            ]],
            ..Default::default()
        };
        let colored = render_to_string_with(&r, Format::Json, true);
        assert!(colored.contains('\x1b'));
        let mut stripped = String::new();
        let mut chars = colored.chars();
        while let Some(c) = chars.next() {
            if c == '\x1b' {
                chars.by_ref().find(|&c| c == 'm');
            } else {
                stripped.push(c);
            }
        }
        assert_eq!(stripped, render_to_string(&r, Format::Json));
    }

    #[test]
    fn json_empty() {
        let r = ResultSet::new(&["a"]);
        assert_eq!(render_to_string(&r, Format::Json), "[]\n");
    }
}

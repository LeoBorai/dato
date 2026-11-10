//! `csv` (RFC 4180) and `tsv` formats.

use std::io::{self, Write};

use super::plain;
use crate::client::ResultSet;

fn csv_field(s: &str) -> String {
    if s.contains([',', '"', '\n', '\r']) {
        format!("\"{}\"", s.replace('"', "\"\""))
    } else {
        s.to_string()
    }
}

fn tsv_field(s: &str) -> String {
    s.replace('\t', "\\t")
        .replace('\n', "\\n")
        .replace('\r', "\\r")
}

pub fn render(rs: &ResultSet, delim: char, out: &mut dyn Write) -> io::Result<()> {
    let enc: fn(&str) -> String = if delim == ',' { csv_field } else { tsv_field };
    let sep = delim.to_string();
    let header: Vec<String> = rs.columns.iter().map(|c| enc(c)).collect();
    writeln!(out, "{}", header.join(&sep))?;
    for row in &rs.rows {
        let cells: Vec<String> = row.iter().map(|v| enc(&plain(v))).collect();
        writeln!(out, "{}", cells.join(&sep))?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::super::{Format, render_to_string};
    use crate::client::{ResultSet, Value};

    #[test]
    fn csv_quoting() {
        let r = ResultSet {
            columns: vec!["a".into(), "b".into()],
            rows: vec![vec![Value::Text("say \"hi\", ok".into()), Value::Null]],
        };
        assert_eq!(
            render_to_string(&r, Format::Csv),
            "a,b\n\"say \"\"hi\"\", ok\",\n"
        );
    }

    #[test]
    fn csv_plain_values() {
        let r = ResultSet {
            columns: vec!["a".into(), "b".into()],
            rows: vec![vec![Value::Integer(1), Value::Text("x".into())]],
        };
        assert_eq!(render_to_string(&r, Format::Csv), "a,b\n1,x\n");
    }

    #[test]
    fn tsv_escapes_tabs_and_newlines() {
        let r = ResultSet {
            columns: vec!["a".into()],
            rows: vec![vec![Value::Text("x\ty\nz".into())]],
        };
        assert_eq!(render_to_string(&r, Format::Tsv), "a\nx\\ty\\nz\n");
    }
}

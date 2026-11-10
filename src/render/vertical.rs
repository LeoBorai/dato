//! `vertical` format: one block per row, one `column | value` line per field.

use std::io::{self, Write};

use unicode_width::UnicodeWidthStr;

use super::display;
use crate::client::ResultSet;

pub fn render(rs: &ResultSet, out: &mut dyn Write) -> io::Result<()> {
    let width = rs
        .columns
        .iter()
        .map(|c| UnicodeWidthStr::width(c.as_str()))
        .max()
        .unwrap_or(0);
    for (i, row) in rs.rows.iter().enumerate() {
        writeln!(
            out,
            "*************************** {}. row ***************************",
            i + 1
        )?;
        for (c, v) in rs.columns.iter().zip(row) {
            let gap = width - UnicodeWidthStr::width(c.as_str());
            writeln!(out, "{c}{} | {}", " ".repeat(gap), display(v))?;
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::super::{Format, render_to_string};
    use crate::client::{ResultSet, Value};

    #[test]
    fn vertical_record() {
        let r = ResultSet {
            columns: vec!["id".into(), "name".into()],
            rows: vec![vec![Value::Integer(1), Value::Text("x".into())]],
        };
        let s = render_to_string(&r, Format::Vertical);
        assert!(s.contains("1. row"));
        assert!(s.contains("id   | 1\n"));
        assert!(s.contains("name | x\n"));
    }
}

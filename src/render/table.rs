//! `ascii`, `psql` and `markdown` table formats.

use std::io::{self, Write};

use unicode_width::UnicodeWidthStr;

use super::{Format, display};
use crate::client::ResultSet;

struct Cell {
    text: String,
    right: bool,
}

fn clean(s: &str) -> String {
    s.replace('\r', "\\r")
        .replace('\n', "\\n")
        .replace('\t', "\\t")
}

fn pad(text: &str, width: usize, right: bool) -> String {
    let gap = width.saturating_sub(UnicodeWidthStr::width(text));
    if right {
        format!("{}{}", " ".repeat(gap), text)
    } else {
        format!("{}{}", text, " ".repeat(gap))
    }
}

pub fn render(rs: &ResultSet, format: Format, out: &mut dyn Write) -> io::Result<()> {
    let markdown = format == Format::Markdown;
    let header: Vec<String> = rs
        .columns
        .iter()
        .map(|c| {
            let c = clean(c);
            if markdown { c.replace('|', "\\|") } else { c }
        })
        .collect();
    let rows: Vec<Vec<Cell>> = rs
        .rows
        .iter()
        .map(|r| {
            r.iter()
                .map(|v| {
                    let t = clean(&display(v));
                    Cell {
                        text: if markdown { t.replace('|', "\\|") } else { t },
                        right: v.is_numeric(),
                    }
                })
                .collect()
        })
        .collect();

    let mut widths: Vec<usize> = header
        .iter()
        .map(|h| UnicodeWidthStr::width(h.as_str()))
        .collect();
    for row in &rows {
        for (i, c) in row.iter().enumerate() {
            widths[i] = widths[i].max(UnicodeWidthStr::width(c.text.as_str()));
        }
    }
    if markdown {
        for w in &mut widths {
            *w = (*w).max(3);
        }
    }
    // A column is right-aligned when every cell in it is numeric.
    let right: Vec<bool> = (0..header.len())
        .map(|i| !rows.is_empty() && rows.iter().all(|r| r[i].right))
        .collect();

    match format {
        Format::Ascii => {
            let sep = format!(
                "+{}+\n",
                widths
                    .iter()
                    .map(|w| "-".repeat(w + 2))
                    .collect::<Vec<_>>()
                    .join("+")
            );
            let line = |cells: Vec<String>| format!("| {} |\n", cells.join(" | "));
            write!(out, "{sep}")?;
            write!(
                out,
                "{}",
                line(
                    header
                        .iter()
                        .zip(&widths)
                        .map(|(h, w)| pad(h, *w, false))
                        .collect()
                )
            )?;
            write!(out, "{sep}")?;
            for row in &rows {
                write!(
                    out,
                    "{}",
                    line(
                        row.iter()
                            .zip(&widths)
                            .zip(&right)
                            .map(|((c, w), r)| pad(&c.text, *w, *r))
                            .collect()
                    )
                )?;
            }
            if !rows.is_empty() {
                write!(out, "{sep}")?;
            }
        }
        Format::Psql => {
            let line = |cells: Vec<String>| format!(" {} \n", cells.join(" | "));
            write!(
                out,
                "{}",
                line(
                    header
                        .iter()
                        .zip(&widths)
                        .map(|(h, w)| pad(h, *w, false))
                        .collect()
                )
            )?;
            writeln!(
                out,
                "{}",
                widths
                    .iter()
                    .map(|w| "-".repeat(w + 2))
                    .collect::<Vec<_>>()
                    .join("+")
            )?;
            for row in &rows {
                write!(
                    out,
                    "{}",
                    line(
                        row.iter()
                            .zip(&widths)
                            .zip(&right)
                            .map(|((c, w), r)| pad(&c.text, *w, *r))
                            .collect()
                    )
                )?;
            }
        }
        _ => {
            let line = |cells: Vec<String>| format!("| {} |\n", cells.join(" | "));
            write!(
                out,
                "{}",
                line(
                    header
                        .iter()
                        .zip(&widths)
                        .map(|(h, w)| pad(h, *w, false))
                        .collect()
                )
            )?;
            writeln!(
                out,
                "| {} |",
                widths
                    .iter()
                    .zip(&right)
                    .map(|(w, r)| if *r {
                        format!("{}:", "-".repeat(w - 1))
                    } else {
                        "-".repeat(*w)
                    })
                    .collect::<Vec<_>>()
                    .join(" | ")
            )?;
            for row in &rows {
                write!(
                    out,
                    "{}",
                    line(
                        row.iter()
                            .zip(&widths)
                            .zip(&right)
                            .map(|((c, w), r)| pad(&c.text, *w, *r))
                            .collect()
                    )
                )?;
            }
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::super::{Format, render_to_string};
    use crate::client::{ResultSet, Value};

    fn rs(col: &str, vals: Vec<Value>) -> ResultSet {
        ResultSet {
            columns: vec![col.into()],
            rows: vals.into_iter().map(|v| vec![v]).collect(),
        }
    }

    #[test]
    fn ascii_output() {
        let r = rs("n", vec![Value::Integer(1), Value::Integer(22)]);
        assert_eq!(
            render_to_string(&r, Format::Ascii),
            "+----+\n| n  |\n+----+\n|  1 |\n| 22 |\n+----+\n"
        );
    }

    #[test]
    fn ascii_wide_characters() {
        let r = rs(
            "c",
            vec![Value::Text("日本".into()), Value::Text("ab".into())],
        );
        assert_eq!(
            render_to_string(&r, Format::Ascii),
            "+------+\n| c    |\n+------+\n| 日本 |\n| ab   |\n+------+\n"
        );
    }

    #[test]
    fn ascii_null_blob_and_newline() {
        let r = rs(
            "v",
            vec![
                Value::Null,
                Value::Blob(vec![1, 2]),
                Value::Text("a\nb".into()),
            ],
        );
        let s = render_to_string(&r, Format::Ascii);
        assert!(s.contains("<null>"));
        assert!(s.contains("<blob 2 bytes>"));
        assert!(s.contains("a\\nb"));
    }

    #[test]
    fn ascii_empty_result_shows_header() {
        let r = rs("a", vec![]);
        assert_eq!(render_to_string(&r, Format::Ascii), "+---+\n| a |\n+---+\n");
    }

    #[test]
    fn psql_output_aligns_numbers() {
        let r = rs("n", vec![Value::Integer(1), Value::Integer(22)]);
        assert_eq!(
            render_to_string(&r, Format::Psql),
            " n  \n----\n  1 \n 22 \n"
        );
    }

    #[test]
    fn numeric_is_right_aligned_verbatim() {
        let r = rs(
            "d",
            vec![
                Value::Numeric("1.10".into()),
                Value::Numeric("100.00".into()),
            ],
        );
        assert_eq!(
            render_to_string(&r, Format::Psql),
            " d      \n--------\n   1.10 \n 100.00 \n"
        );
    }

    #[test]
    fn psql_multiple_columns() {
        let r = ResultSet {
            columns: vec!["a".into(), "b".into()],
            rows: vec![vec![Value::Integer(1), Value::Text("x".into())]],
        };
        assert_eq!(
            render_to_string(&r, Format::Psql),
            " a | b \n---+---\n 1 | x \n"
        );
    }

    #[test]
    fn markdown_escapes_pipes_and_aligns() {
        let r = rs("t", vec![Value::Text("a|b".into())]);
        assert_eq!(
            render_to_string(&r, Format::Markdown),
            "| t    |\n| ---- |\n| a\\|b |\n"
        );
        let n = rs("n", vec![Value::Integer(5)]);
        assert_eq!(
            render_to_string(&n, Format::Markdown),
            "| n   |\n| --: |\n|   5 |\n"
        );
    }
}

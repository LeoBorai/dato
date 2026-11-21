//! `ascii`, `psql` and `markdown` table formats.

use std::io::{self, Write};

use nu_ansi_term::{Color, Style};
use unicode_width::UnicodeWidthStr;

use super::{Format, display};
use crate::client::{ColumnMeta, ResultSet, Value};

struct Cell {
    text: String,
    right: bool,
}

fn clean(s: &str) -> String {
    s.replace('\r', "\\r")
        .replace('\n', "\\n")
        .replace('\t', "\\t")
        .chars()
        .map(|c| if c.is_control() { '\u{FFFD}' } else { c })
        .collect()
}

/// Longest cell text in the `ascii` table; longer text ends in `…`.
const MAX_CELL_WIDTH: usize = 60;

fn truncate(s: String) -> String {
    if s.chars().count() <= MAX_CELL_WIDTH {
        return s;
    }
    let mut t: String = s.chars().take(MAX_CELL_WIDTH - 1).collect();
    t.push('…');
    t
}

fn pad(text: &str, width: usize, right: bool) -> String {
    let gap = width.saturating_sub(UnicodeWidthStr::width(text));
    if right {
        format!("{}{}", " ".repeat(gap), text)
    } else {
        format!("{}{}", text, " ".repeat(gap))
    }
}

/// Header cell text: `name (type)` when the type is known.
fn header_parts(name: &str, meta: Option<&ColumnMeta>) -> (String, Option<String>) {
    let ty = meta
        .and_then(|m| m.type_name.as_deref())
        .map(|t| format!(" ({})", clean(t)));
    (clean(name), ty)
}

fn paint(text: &str, style: Option<Style>) -> String {
    match style {
        Some(s) => s.paint(text).to_string(),
        None => text.to_string(),
    }
}

fn value_style(v: &Value, meta: Option<&ColumnMeta>) -> Option<Style> {
    if matches!(v, Value::Null) {
        return Some(Style::new().italic().fg(Color::DarkGray));
    }
    if v.is_numeric() && meta.is_none_or(|m| !m.primary_key && !m.boolean) {
        return Some(Style::new().fg(Color::Magenta));
    }
    let m = meta?;
    if m.primary_key {
        Some(Style::new().fg(Color::Fixed(214)))
    } else if m.boolean {
        Some(Style::new().fg(Color::Yellow))
    } else {
        None
    }
}

/// The default table, in the style of the `sqlite3 -box`-like grid:
///
/// ```text
/// ┌─────────────┬───────┐
/// │ id (integer)│ name  │
/// ╞═════════════╪═══════╡
/// │           1 │ Alice │
/// ├─────────────┼───────┤
/// ```
///
/// With `colors`, header names are bold light blue with a plain type, and
/// values are colored by kind (key orange, boolean yellow, number magenta,
/// NULL dim italic); text stays neutral. Cells are cut at 60 characters.
pub fn render_ascii(rs: &ResultSet, colors: bool, out: &mut dyn Write) -> io::Result<()> {
    let meta = |i: usize| rs.meta.get(i).filter(|_| rs.meta.len() == rs.columns.len());
    let header: Vec<(String, Option<String>)> = rs
        .columns
        .iter()
        .enumerate()
        .map(|(i, c)| header_parts(c, meta(i)))
        .collect();
    let rows: Vec<Vec<(String, Option<Style>, bool)>> = rs
        .rows
        .iter()
        .map(|r| {
            r.iter()
                .enumerate()
                .map(|(i, v)| {
                    (
                        truncate(clean(&display(v))),
                        value_style(v, meta(i)),
                        v.is_numeric(),
                    )
                })
                .collect()
        })
        .collect();
    let head_w = |h: &(String, Option<String>)| {
        UnicodeWidthStr::width(h.0.as_str()) + h.1.as_deref().map_or(0, UnicodeWidthStr::width)
    };
    let mut widths: Vec<usize> = header.iter().map(head_w).collect();
    for row in &rows {
        for (i, c) in row.iter().enumerate() {
            widths[i] = widths[i].max(UnicodeWidthStr::width(c.0.as_str()));
        }
    }
    let right: Vec<bool> = (0..header.len())
        .map(|i| !rows.is_empty() && rows.iter().all(|r| r[i].2))
        .collect();

    let rule = |fill: &str, l: &str, m: &str, r: &str| {
        let cols = widths
            .iter()
            .map(|w| fill.repeat(w + 2))
            .collect::<Vec<_>>();
        format!("{l}{}{r}\n", cols.join(m))
    };
    let top = rule("─", "┌", "┬", "┐");
    let head_rule = rule("═", "╞", "╪", "╡");
    let row_rule = rule("─", "├", "┼", "┤");
    let bottom = rule("─", "└", "┴", "┘");

    write!(out, "{top}")?;
    let cells: Vec<String> = header
        .iter()
        .zip(&widths)
        .map(|(h, w)| {
            let gap = " ".repeat(w.saturating_sub(head_w(h)));
            let (name_style, type_style) = if colors {
                (Some(Style::new().bold().fg(Color::LightBlue)), None)
            } else {
                (None, None)
            };
            format!(
                "{}{}{gap}",
                paint(&h.0, name_style),
                h.1.as_deref()
                    .map_or(String::new(), |t| paint(t, type_style))
            )
        })
        .collect();
    writeln!(out, "│ {} │", cells.join(" │ "))?;
    if rows.is_empty() {
        return write!(out, "{bottom}");
    }
    write!(out, "{head_rule}")?;
    for (n, row) in rows.iter().enumerate() {
        if n > 0 {
            write!(out, "{row_rule}")?;
        }
        let cells: Vec<String> = row
            .iter()
            .zip(&widths)
            .zip(&right)
            .map(|(((text, style, _), w), r)| {
                let gap = " ".repeat(w.saturating_sub(UnicodeWidthStr::width(text.as_str())));
                let text = paint(text, if colors { *style } else { None });
                if *r {
                    format!("{gap}{text}")
                } else {
                    format!("{text}{gap}")
                }
            })
            .collect();
        writeln!(out, "│ {} │", cells.join(" │ "))?;
    }
    write!(out, "{bottom}")
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
        Format::Ascii => unreachable!("rendered by render_ascii"),
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
            ..Default::default()
        }
    }

    #[test]
    fn ascii_output() {
        let r = rs("n", vec![Value::Integer(1), Value::Integer(22)]);
        assert_eq!(
            render_to_string(&r, Format::Ascii),
            "┌────┐\n│ n  │\n╞════╡\n│  1 │\n├────┤\n│ 22 │\n└────┘\n"
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
            "┌──────┐\n│ c    │\n╞══════╡\n│ 日本 │\n├──────┤\n│ ab   │\n└──────┘\n"
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
        assert_eq!(render_to_string(&r, Format::Ascii), "┌───┐\n│ a │\n└───┘\n");
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
            ..Default::default()
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

    fn typed() -> ResultSet {
        use crate::client::ColumnMeta;
        let m = |t: &str, pk, boolean| ColumnMeta {
            type_name: Some(t.into()),
            primary_key: pk,
            boolean,
        };
        ResultSet {
            columns: vec!["id".into(), "name".into(), "ok".into()],
            rows: vec![
                vec![
                    Value::Integer(1),
                    Value::Text("Alice".into()),
                    Value::Text("t".into()),
                ],
                vec![Value::Integer(2), Value::Null, Value::Text("f".into())],
            ],
            meta: vec![
                m("int4", true, false),
                m("text", false, false),
                m("bool", false, true),
            ],
        }
    }

    #[test]
    fn ascii_shows_types_in_header() {
        assert_eq!(
            render_to_string(&typed(), Format::Ascii),
            "┌───────────┬─────────────┬───────────┐\n\
             │ id (int4) │ name (text) │ ok (bool) │\n\
             ╞═══════════╪═════════════╪═══════════╡\n\
             │         1 │ Alice       │ t         │\n\
             ├───────────┼─────────────┼───────────┤\n\
             │         2 │ <null>      │ f         │\n\
             └───────────┴─────────────┴───────────┘\n"
        );
    }

    #[test]
    fn colors_header_pk_boolean_number() {
        let s = crate::render::render_to_string_with(&typed(), Format::Ascii, true);
        assert!(s.contains("\x1b[1;94mid\x1b[0m (int4)"), "{s:?}");
        assert!(s.contains("\x1b[38;5;214m1\x1b[0m"), "pk orange: {s:?}");
        assert!(s.contains("\x1b[33mt\x1b[0m"), "bool yellow: {s:?}");
        assert!(
            s.contains("\x1b[3;90m<null>\x1b[0m"),
            "null dim italic: {s:?}"
        );
        assert!(
            !s.contains("Alice\x1b") && !s.contains("\x1b[0mAlice"),
            "text neutral"
        );
        let plain = crate::render::render_to_string_with(&typed(), Format::Ascii, false);
        assert!(!plain.contains('\x1b'));
    }

    #[test]
    fn numbers_are_magenta_without_pk() {
        let mut r = typed();
        r.meta[0].primary_key = false;
        let s = crate::render::render_to_string_with(&r, Format::Ascii, true);
        assert!(s.contains("\x1b[35m1\x1b[0m"), "{s:?}");
    }

    #[test]
    fn control_characters_never_reach_the_terminal() {
        let r = rs("c", vec![Value::Text("a\x1b[2Jb\x08c".into())]);
        let s = render_to_string(&r, Format::Ascii);
        assert!(!s.contains('\x1b') && !s.contains('\x08'), "{s:?}");
    }

    #[test]
    fn long_text_is_truncated() {
        let r = rs("c", vec![Value::Text("x".repeat(100))]);
        let s = render_to_string(&r, Format::Ascii);
        let cell = format!("{}…", "x".repeat(59));
        assert!(s.contains(&cell) && !s.contains(&"x".repeat(60)), "{s}");
    }
}

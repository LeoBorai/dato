//! Result renderers for the supported output formats.

mod delimited;
mod json;
mod table;
mod vertical;

use std::fmt;
use std::io::{self, Write};
use std::str::FromStr;

use crate::client::{ResultSet, Value};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Format {
    #[default]
    Ascii,
    Psql,
    Markdown,
    Csv,
    Tsv,
    Json,
    Vertical,
}

impl Format {
    pub const NAMES: [&'static str; 7] = [
        "ascii", "psql", "markdown", "csv", "tsv", "json", "vertical",
    ];

    pub fn name(self) -> &'static str {
        Self::NAMES[self as usize]
    }
}

impl fmt::Display for Format {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.name())
    }
}

impl FromStr for Format {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, String> {
        match s.to_ascii_lowercase().as_str() {
            "ascii" => Ok(Format::Ascii),
            "psql" => Ok(Format::Psql),
            "markdown" => Ok(Format::Markdown),
            "csv" => Ok(Format::Csv),
            "tsv" => Ok(Format::Tsv),
            "json" => Ok(Format::Json),
            "vertical" => Ok(Format::Vertical),
            _ => Err(format!(
                "unknown format '{s}'; valid formats: {}",
                Format::NAMES.join(", ")
            )),
        }
    }
}

pub fn render(rs: &ResultSet, format: Format, out: &mut dyn Write) -> io::Result<()> {
    match format {
        Format::Ascii | Format::Psql | Format::Markdown => table::render(rs, format, out),
        Format::Csv => delimited::render(rs, ',', out),
        Format::Tsv => delimited::render(rs, '\t', out),
        Format::Json => json::render(rs, out),
        Format::Vertical => vertical::render(rs, out),
    }
}

pub fn render_to_string(rs: &ResultSet, format: Format) -> String {
    let mut buf = Vec::new();
    render(rs, format, &mut buf).expect("writing to a Vec cannot fail");
    String::from_utf8_lossy(&buf).into_owned()
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

/// Text for a value in the delimited formats.
fn plain(v: &Value) -> String {
    match v {
        Value::Null => String::new(),
        Value::Integer(i) => i.to_string(),
        Value::Real(r) => format!("{r:?}"),
        Value::Text(t) => t.clone(),
        Value::Blob(b) => format!("x'{}'", hex(b)),
    }
}

/// Text for a value in the table and vertical formats.
fn display(v: &Value) -> String {
    match v {
        Value::Null => "<null>".into(),
        Value::Blob(b) => format!("<blob {} bytes>", b.len()),
        other => plain(other),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    pub(crate) fn sample() -> ResultSet {
        ResultSet {
            columns: vec!["n".into()],
            rows: vec![vec![Value::Integer(1)], vec![Value::Integer(22)]],
        }
    }

    #[test]
    fn format_parsing() {
        assert_eq!("Markdown".parse::<Format>().unwrap(), Format::Markdown);
        for n in Format::NAMES {
            assert_eq!(n.parse::<Format>().unwrap().to_string(), n);
        }
        let err = "bogus".parse::<Format>().unwrap_err();
        assert!(err.contains("ascii") && err.contains("vertical"));
    }

    #[test]
    fn value_display() {
        assert_eq!(display(&Value::Null), "<null>");
        assert_eq!(display(&Value::Blob(vec![1, 2, 3])), "<blob 3 bytes>");
        assert_eq!(display(&Value::Real(1.0)), "1.0");
        assert_eq!(plain(&Value::Null), "");
    }

    #[test]
    fn sample_renders_in_every_format() {
        for n in Format::NAMES {
            let s = render_to_string(&sample(), n.parse().unwrap());
            assert!(s.contains("22"), "{n}");
        }
    }
}

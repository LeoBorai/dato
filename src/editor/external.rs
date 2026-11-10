//! Editing the current input in an external editor (`\e`, Ctrl-O).

use std::io::{self, Write};
use std::process::Command;

/// `$VISUAL`, then `$EDITOR`, then `vi`.
pub fn editor_command() -> String {
    ["VISUAL", "EDITOR"]
        .iter()
        .filter_map(|v| std::env::var(v).ok())
        .find(|v| !v.trim().is_empty())
        .unwrap_or_else(|| "vi".to_string())
}

/// Opens `initial` in `editor` (a program, optionally with arguments) and returns the saved text.
pub fn edit_with(editor: &str, initial: &str) -> io::Result<String> {
    let mut file = tempfile::Builder::new().suffix(".sql").tempfile()?;
    file.write_all(initial.as_bytes())?;
    file.flush()?;
    let mut parts = editor.split_whitespace();
    let program = parts
        .next()
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "empty editor command"))?;
    let status = Command::new(program)
        .args(parts)
        .arg(file.path())
        .status()?;
    if !status.success() {
        return Err(io::Error::other(format!("editor exited with {status}")));
    }
    let text = std::fs::read_to_string(file.path())?;
    Ok(text.trim_end_matches(['\n', '\r']).to_string())
}

pub fn edit(initial: &str) -> io::Result<String> {
    edit_with(&editor_command(), initial)
}

/// The input without a trailing `\e`, if it has one.
pub fn strip_edit_suffix(input: &str) -> Option<&str> {
    input.trim_end().strip_suffix("\\e")
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;
    use std::os::unix::fs::PermissionsExt;

    fn script(dir: &std::path::Path, body: &str) -> String {
        let p = dir.join("ed.sh");
        std::fs::write(&p, format!("#!/bin/sh\n{body}\n")).unwrap();
        std::fs::set_permissions(&p, std::fs::Permissions::from_mode(0o755)).unwrap();
        p.to_str().unwrap().to_string()
    }

    #[test]
    fn returns_saved_text() {
        let dir = tempfile::tempdir().unwrap();
        let ed = script(dir.path(), "printf 'select 2;\\n' > \"$1\"");
        assert_eq!(edit_with(&ed, "select 1").unwrap(), "select 2;");
    }

    #[test]
    fn editor_sees_initial_text() {
        let dir = tempfile::tempdir().unwrap();
        let ed = script(dir.path(), "cat \"$1\" > \"$1.out\"; cp \"$1.out\" \"$1\"");
        assert_eq!(edit_with(&ed, "select 1").unwrap(), "select 1");
    }

    #[test]
    fn failing_editor_is_an_error() {
        let dir = tempfile::tempdir().unwrap();
        let ed = script(dir.path(), "exit 3");
        assert!(edit_with(&ed, "x").is_err());
    }

    #[test]
    fn suffix_detection() {
        assert_eq!(strip_edit_suffix("select 1 \\e"), Some("select 1 "));
        assert_eq!(strip_edit_suffix("select 1"), None);
    }
}

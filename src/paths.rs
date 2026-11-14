//! Where dato keeps its files: everything lives in `~/.dato` (the user's home
//! directory on every platform).

use std::io;
use std::path::{Path, PathBuf};

/// Name of the application directory inside the home directory.
pub const APP_DIR: &str = ".dato";

/// `<home>/.dato`.
pub fn app_dir_in(home: &Path) -> PathBuf {
    home.join(APP_DIR)
}

/// `~/.dato`, or `None` when the home directory is unknown.
pub fn app_dir() -> Option<PathBuf> {
    dirs::home_dir().map(|h| app_dir_in(&h))
}

/// Shell history: `~/.dato/history`.
pub fn history_file() -> Option<PathBuf> {
    app_dir().map(|d| d.join("history"))
}

/// Saved connections: `~/.dato/connections.json`.
pub fn connections_file() -> Option<PathBuf> {
    app_dir().map(|d| d.join("connections.json"))
}

/// Buffer the external editor opens for Ctrl-O: `~/.dato/edit.sql`.
pub fn edit_buffer() -> Option<PathBuf> {
    app_dir().map(|d| d.join("edit.sql"))
}

/// Replaces a leading `~` or `~/` with the home directory, as a shell would.
/// Anything else, or an unknown home directory, is returned unchanged.
pub fn expand_tilde(path: &str) -> PathBuf {
    let rest = if path == "~" {
        Some("")
    } else {
        path.strip_prefix("~/")
    };
    match (rest, dirs::home_dir()) {
        (Some(rest), Some(home)) => home.join(rest),
        _ => PathBuf::from(path),
    }
}

/// Shows a path under the home directory as `~/...`; the inverse of
/// [`expand_tilde`]. Other paths are returned unchanged.
pub fn collapse_home(path: &str) -> String {
    let Some(home) = dirs::home_dir() else {
        return path.to_string();
    };
    match std::path::Path::new(path).strip_prefix(&home) {
        Ok(rest) if rest.as_os_str().is_empty() => "~".into(),
        Ok(rest) => format!("~/{}", rest.display()),
        Err(_) => path.to_string(),
    }
}

/// Creates `dir` if missing. On Unix it is private to the user (`0700`): history
/// and edit buffers can hold queries with sensitive data.
pub fn ensure_dir(dir: &Path) -> io::Result<()> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::DirBuilderExt;
        std::fs::DirBuilder::new()
            .recursive(true)
            .mode(0o700)
            .create(dir)
    }
    #[cfg(not(unix))]
    {
        std::fs::create_dir_all(dir)
    }
}

/// Creates `~/.dato` if missing and returns it.
pub fn ensure_app_dir() -> io::Result<PathBuf> {
    let dir = app_dir().ok_or_else(|| io::Error::other("cannot find the home directory"))?;
    ensure_dir(&dir)?;
    Ok(dir)
}

/// History location used before `~/.dato` (`<data dir>/dato/history`).
fn legacy_history_file() -> Option<PathBuf> {
    dirs::data_dir().map(|d| d.join("dato").join("history"))
}

/// Copies history from `legacy` to `target` when `target` does not exist yet.
/// The old file is left in place. Returns whether anything was copied.
pub fn migrate_history(legacy: &Path, target: &Path) -> io::Result<bool> {
    if target.exists() || !legacy.is_file() {
        return Ok(false);
    }
    std::fs::copy(legacy, target)?;
    Ok(true)
}

/// Copies history from the old platform data directory into `~/.dato`, once.
pub fn migrate_legacy_history() {
    if let (Some(legacy), Some(target)) = (legacy_history_file(), history_file()) {
        let _ = migrate_history(&legacy, &target);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tilde_expands_to_home() {
        let Some(home) = dirs::home_dir() else { return };
        assert_eq!(expand_tilde("~/a/b.db"), home.join("a/b.db"));
        assert_eq!(expand_tilde("~"), home);
        assert_eq!(expand_tilde("~other/x"), Path::new("~other/x"));
        assert_eq!(expand_tilde("a/~/b"), Path::new("a/~/b"));
        assert_eq!(expand_tilde("/abs"), Path::new("/abs"));
    }

    #[test]
    fn home_collapses_to_tilde() {
        let Some(home) = dirs::home_dir() else { return };
        let h = home.display();
        assert_eq!(collapse_home(&format!("{h}/a/b.db")), "~/a/b.db");
        assert_eq!(collapse_home(&h.to_string()), "~");
        assert_eq!(collapse_home(&format!("{h}x/a.db")), format!("{h}x/a.db"));
        assert_eq!(collapse_home("/etc/a.db"), "/etc/a.db");
        assert_eq!(collapse_home("rel.db"), "rel.db");
        assert_eq!(
            expand_tilde(&collapse_home(&format!("{h}/a.db"))),
            home.join("a.db")
        );
    }

    #[test]
    fn everything_lives_in_dot_dato() {
        let home = Path::new("/home/u");
        assert_eq!(app_dir_in(home), Path::new("/home/u/.dato"));
        if let (Some(dir), Some(h), Some(e)) = (app_dir(), history_file(), edit_buffer()) {
            assert!(dir.ends_with(".dato"));
            assert_eq!(h.parent(), Some(dir.as_path()));
            assert_eq!(e.parent(), Some(dir.as_path()));
            assert!(h.ends_with(".dato/history"));
        }
    }

    #[cfg(unix)]
    #[test]
    fn app_dir_is_private() {
        use std::os::unix::fs::PermissionsExt;
        let home = tempfile::tempdir().unwrap();
        let dir = app_dir_in(home.path());
        ensure_dir(&dir).unwrap();
        ensure_dir(&dir).unwrap();
        let mode = std::fs::metadata(&dir).unwrap().permissions().mode();
        assert_eq!(mode & 0o777, 0o700);
    }

    #[test]
    fn history_migrates_once_without_overwriting() {
        let tmp = tempfile::tempdir().unwrap();
        let legacy = tmp.path().join("old");
        let target = tmp.path().join("history");
        assert!(!migrate_history(&legacy, &target).unwrap());
        std::fs::write(&legacy, "select 1;\n").unwrap();
        assert!(migrate_history(&legacy, &target).unwrap());
        assert_eq!(std::fs::read_to_string(&target).unwrap(), "select 1;\n");
        std::fs::write(&legacy, "select 2;\n").unwrap();
        assert!(!migrate_history(&legacy, &target).unwrap());
        assert_eq!(std::fs::read_to_string(&target).unwrap(), "select 1;\n");
        assert!(legacy.exists());
    }
}

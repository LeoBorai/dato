//! User settings, read on startup from `~/.dato/settings.toml`.

use std::io;
use std::path::Path;

use crate::paths;

const DEFAULT_FILE: &str = "\
# dato settings

# Color table output: header, numbers, booleans and primary keys.
colors = true
";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Settings {
    pub colors: bool,
}

impl Default for Settings {
    fn default() -> Self {
        Self { colors: true }
    }
}

impl Settings {
    /// Reads `key = value` lines; unknown keys and bad values are ignored.
    pub fn parse(text: &str) -> Self {
        let mut s = Self::default();
        for line in text.lines() {
            let line = line.split('#').next().unwrap_or("").trim();
            if let Some((key, value)) = line.split_once('=')
                && key.trim() == "colors"
                && let Ok(b) = value.trim().parse::<bool>()
            {
                s.colors = b;
            }
        }
        s
    }

    /// Loads `path`, creating it with the defaults when missing. Failures
    /// fall back to the defaults.
    pub fn load_or_init_at(path: &Path) -> Self {
        match std::fs::read_to_string(path) {
            Ok(text) => Self::parse(&text),
            Err(e) if e.kind() == io::ErrorKind::NotFound => {
                let _ = write_default(path);
                Self::default()
            }
            Err(_) => Self::default(),
        }
    }

    /// Loads `~/.dato/settings.toml`, creating it with the defaults when missing.
    pub fn load_or_init() -> Self {
        paths::settings_file().map_or_else(Self::default, |p| Self::load_or_init_at(&p))
    }
}

fn write_default(path: &Path) -> io::Result<()> {
    if let Some(dir) = path.parent() {
        paths::ensure_dir(dir)?;
    }
    std::fs::write(path, DEFAULT_FILE)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn colors_default_on() {
        assert!(Settings::parse("").colors);
        assert!(Settings::parse("colors = maybe").colors);
    }

    #[test]
    fn colors_can_be_disabled() {
        assert!(!Settings::parse("colors = false # no\n").colors);
        assert!(Settings::parse("# colors = false\n").colors);
    }

    #[test]
    fn missing_file_is_created_with_defaults() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join(".dato").join("settings.toml");
        assert!(Settings::load_or_init_at(&path).colors);
        assert_eq!(std::fs::read_to_string(&path).unwrap(), DEFAULT_FILE);
        std::fs::write(&path, "colors = false\n").unwrap();
        assert!(!Settings::load_or_init_at(&path).colors);
    }
}

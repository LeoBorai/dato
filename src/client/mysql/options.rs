//! MySQL login parameters.

/// From `dato mysql`, `\connect mysql` or the wizard.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MySqlOptions {
    pub host: String,
    pub port: u16,
    pub user: String,
    pub password: Option<String>,
    /// `-p` was given: ask for the password before connecting.
    pub password_required: bool,
    pub socket: Option<String>,
    pub database: Option<String>,
}

impl Default for MySqlOptions {
    fn default() -> Self {
        Self {
            host: "localhost".into(),
            port: 3306,
            user: String::new(),
            password: None,
            password_required: false,
            socket: None,
            database: None,
        }
    }
}

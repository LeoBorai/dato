//! Decides whether Enter submits the input or inserts a newline.

use std::sync::{Arc, RwLock};

use reedline::{ValidationResult, Validator};

use crate::dialect::Dialect;
use crate::server::Server;
use crate::sql::is_complete_with;

/// Uses the statement rules of the session's current server.
pub struct SqlValidator {
    dialect: Arc<RwLock<Server>>,
}

impl SqlValidator {
    pub fn new(dialect: Arc<RwLock<Server>>) -> Self {
        Self { dialect }
    }
}

impl Validator for SqlValidator {
    fn validate(&self, line: &str) -> ValidationResult {
        let server = *self.dialect.read().unwrap();
        if is_complete_with(line, &server.lex_rules()) {
            ValidationResult::Complete
        } else {
            ValidationResult::Incomplete
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn maps_completeness() {
        let v = SqlValidator::new(Arc::new(RwLock::new(Server::Sqlite)));
        assert!(matches!(
            v.validate("select 1;"),
            ValidationResult::Complete
        ));
        assert!(matches!(
            v.validate("select 1"),
            ValidationResult::Incomplete
        ));
        assert!(matches!(v.validate(".tables"), ValidationResult::Complete));
    }

    #[test]
    fn follows_the_session_dialect() {
        let dialect = Arc::new(RwLock::new(Server::Sqlite));
        let v = SqlValidator::new(Arc::clone(&dialect));
        // On SQLite `#` is not a comment, so `# x` starts an unfinished statement.
        assert!(matches!(
            v.validate("select 1; # x"),
            ValidationResult::Incomplete
        ));
        *dialect.write().unwrap() = Server::MySql;
        assert!(matches!(
            v.validate("select 1; # x"),
            ValidationResult::Complete
        ));
        assert!(matches!(
            v.validate("select 'it\\'s;'"),
            ValidationResult::Incomplete
        ));
    }
}

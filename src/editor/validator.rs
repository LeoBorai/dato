//! Decides whether Enter submits the input or inserts a newline.

use reedline::{ValidationResult, Validator};

use crate::sql::is_complete;

pub struct SqlValidator;

impl Validator for SqlValidator {
    fn validate(&self, line: &str) -> ValidationResult {
        if is_complete(line) {
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
        let v = SqlValidator;
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
}

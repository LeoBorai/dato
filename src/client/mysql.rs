//! MySQL client: stub. The target is recognised so the CLI can report a clear error.

use super::{Client, ClientError};

pub fn open(_url: &str) -> Result<Box<dyn Client>, ClientError> {
    Err(ClientError::Unsupported(
        "MySQL backend is not available (not implemented yet)".into(),
    ))
}

//! Ways to get database credentials other than typing a password.
//!
//! Each method lives in its own submodule and knows nothing about database
//! clients or the terminal UI: it takes its own configuration and returns a
//! user name and password, which `crate::client` passes on to the server.

pub mod vault;

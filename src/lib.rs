//! `dato`: a database CLI with completion, syntax highlighting and table formats.
//!
//! The crate is split into backend-agnostic building blocks (`editor`, `render`,
//! `special`, `tui`) and per-server clients under `client`.

pub mod client;
pub mod render;
pub mod sql;

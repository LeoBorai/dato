//! `dato`: a database CLI with completion, syntax highlighting and table formats.
//!
//! The crate is split into backend-agnostic building blocks (`editor`, `render`,
//! `special`, `tui`) and per-server clients under `client`.

pub mod cli;
pub mod client;
pub mod editor;
pub mod render;
pub mod special;
pub mod sql;
pub mod tui;

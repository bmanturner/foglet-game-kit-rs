//! `fgk` library — internals shared between the binary and unit tests.
//!
//! Splitting the CLI into a `lib.rs` + `main.rs` pair lets us
//! integration-test scaffolding logic (templates, manifest emission,
//! packaging) without driving the binary through `assert_cmd` for every
//! assertion. The binary remains the public entry point; everything
//! reusable lives behind this crate.

#![forbid(unsafe_code)]
#![warn(missing_docs, rust_2018_idioms)]

pub mod emit_manifest;
pub mod package;
pub mod scaffold;
pub mod templates;
pub mod tick;

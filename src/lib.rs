//! Library crate for PDF Utility.
//!
//! Exists alongside `src/main.rs` so that the document-handling core is
//! exercisable both by the application binary and by integration tests
//! under `tests/`.

pub mod document;
pub mod form_fields;

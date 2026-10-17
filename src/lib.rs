//! Library crate for PDF Utility.
//!
//! Exists alongside `src/main.rs` so that the document-handling core is
//! exercisable both by the application binary and by integration tests
//! under `tests/`.

pub mod annotations;
pub mod apply;
pub mod document;
pub mod form_fields;
pub mod overlay;
pub mod pdf_structure;
pub mod save;
pub mod signature_image;
pub mod signatures;

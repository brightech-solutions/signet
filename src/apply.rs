//! Apply ("flatten"): bakes AcroForm form fields and annotations — the
//! app's own text box / signature overlay, once written onto the document
//! via [`crate::annotations::write_overlay_to_annotations`], plus anything
//! already present in the source PDF — directly into each page's content
//! stream via pdfium's native `FPDFPage_Flatten`, after which they're no
//! longer separate, editable objects.
//!
//! This is a one-way operation: pdfium has no "unflatten". That's exactly
//! why the app treats a document as locked (`applied` in `src/app.rs`)
//! immediately after a successful [`flatten`] — editing tools left live
//! against a flattened document would accept input that can never actually
//! take effect, silently losing it.
//!
//! **pdfium-render API note (verified against the vendored 0.9.4 source
//! under `~/.cargo/registry/src/.../pdfium-render-0.9.4/`):**
//! `PdfPage::flatten()` (`src/pdf/document/page.rs`) is a safe public
//! method, so no raw/unsafe FFI escape hatch is needed here. It has two
//! `cfg`-gated bodies, selected by pdfium-render's own `flatten` Cargo
//! feature:
//! - without that feature (this crate's `Cargo.toml` doesn't enable it):
//!   calls pdfium's own native `FPDFPage_Flatten` C API — exactly what this
//!   task asks for — via `self.bindings().FPDFPage_Flatten(...)` internally
//!   (still unsafe *inside* pdfium-render, but wrapped in a safe method we
//!   call normally).
//! - with that feature: substitutes a custom-written reimplementation
//!   instead of pdfium's native flatten (see
//!   <https://github.com/ajrcarey/pdfium-render/issues/140> for why
//!   pdfium-render added that alternative), which this project deliberately
//!   does not opt into.

use crate::document::{DocumentError, PdfDocumentState};

/// Flattens every page of `doc`: AcroForm fields and annotations are baked
/// into page content and removed as separate objects. Returns
/// `DocumentError::Pdfium` if any page fails to flatten — pdfium's
/// `FPDFPage_Flatten` reports every failure identically with no further
/// cause, so the resulting message is necessarily generic.
pub fn flatten(doc: &mut PdfDocumentState) -> Result<(), DocumentError> {
    let document = doc.inner_mut();
    let page_count = document.pages().len();

    for page_index in 0..page_count {
        let mut page = document
            .pages()
            .get(page_index)
            .map_err(|err| DocumentError::Pdfium(err.to_string()))?;

        page.flatten()
            .map_err(|err| DocumentError::Pdfium(err.to_string()))?;
    }

    Ok(())
}

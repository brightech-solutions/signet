//! Apply ("flatten"): bakes AcroForm form fields and annotations — the
//! app's own text box / signature overlay, written onto the document as
//! annotations first, plus anything already present in the source PDF —
//! directly into each page's content stream via pdfium's native
//! `FPDFPage_Flatten`, after which they're no longer separate, editable
//! objects.
//!
//! This is a one-way operation: pdfium has no "unflatten". That's exactly
//! why the app treats a document as locked (`applied` in `src/app.rs`)
//! immediately after a successful [`apply`] — editing tools left live
//! against a flattened document would accept input that can never actually
//! take effect, silently losing it.
//!
//! **Why an earlier version erased every addition (final review Critical
//! #2), and what makes this one work.** pdfium-render's
//! `PdfPage::flatten()` (vendored 0.9.4, `src/pdf/document/page.rs`, the
//! non-`flatten`-feature body this crate uses) calls `FPDFPage_Flatten`
//! with `FLAT_PRINT`. pdfium's flatten bakes an annotation by drawing its
//! normal appearance stream (`/AP /N`) into the page, then removes the
//! page's whole `/Annots` array. Two things therefore silently drop an
//! annotation instead of baking it:
//!
//! 1. **No print flag.** With `FLAT_PRINT`, only annotations whose `/F`
//!    has the Print bit are baked. Annotations created through
//!    `FPDFPage_CreateAnnot` have no flags, so the app's text boxes and
//!    signatures vanished. [`crate::annotations`] now sets the Print flag
//!    on everything it writes (also correct in its own right: they're
//!    content, and should print).
//! 2. **No appearance stream yet.** pdfium generates a `FreeText`
//!    annotation's or a form widget's `/AP` lazily, the first time the
//!    page is drawn, so anything added or changed since the page was last
//!    rendered had none to bake. [`flatten`] now renders each page that has
//!    annotations once (into a throwaway bitmap) right before flattening
//!    it, via [`PdfDocumentState::realize_appearance_streams`].
//!
//! (A probe against the real library confirmed both: with the print flag
//! set and the page rendered first, a text box went from 1471 ink pixels
//! before flatten to 1471 after, and stayed there across save + reopen;
//! without either, 0. The reviewer's hypothesis that pdfium-render's
//! post-flatten `regenerate_content()` was the culprit did not hold up —
//! `FPDFPage_GenerateContent` only rewrites streams holding dirty page
//! objects, and flatten dirties none.)
//!
//! A filled AcroForm field's own appearance is kept current by
//! [`crate::form_fields::set_field_value`]; flatten bakes exactly that.

use pdfium_render::prelude::PdfPageIndex;

use crate::annotations::{remove_overlay_annotations, write_overlay_to_annotations};
use crate::document::{DocumentError, PdfDocumentState};
use crate::overlay::OverlayLayer;

/// Bakes `overlay` and every form field / annotation already on `doc` into
/// page content: writes each overlay element onto its page as an
/// annotation (replacing any of the app's own annotations already there),
/// then [`flatten`]s the document.
///
/// If writing the overlay fails part-way, the app annotations written so
/// far are removed again before the error is returned, so a failed Apply
/// leaves the document unflattened and as it was.
pub fn apply(doc: &mut PdfDocumentState, overlay: &OverlayLayer) -> Result<(), DocumentError> {
    let page_count = doc.page_count();

    let write_result = (0..page_count).try_for_each(|page_index| {
        remove_overlay_annotations(doc, page_index)?;
        let elements: Vec<_> = overlay
            .elements
            .iter()
            .filter(|element| element.page_index() == page_index)
            .cloned()
            .collect();
        if elements.is_empty() {
            return Ok(());
        }
        write_overlay_to_annotations(doc, page_index, &elements)
    });

    if let Err(err) = write_result {
        for page_index in 0..page_count {
            let _ = remove_overlay_annotations(doc, page_index);
        }
        return Err(err);
    }

    flatten(doc)
}

/// Flattens every page of `doc`: AcroForm fields and annotations are baked
/// into page content and removed as separate objects. Returns
/// `DocumentError::Pdfium` if any page fails to flatten — pdfium's
/// `FPDFPage_Flatten` reports every failure identically with no further
/// cause, so the resulting message is necessarily generic.
///
/// Only annotations pdfium considers printable are baked (pdfium-render
/// hard-codes `FLAT_PRINT`); a non-printing annotation in a source PDF
/// (e.g. an on-screen-only button) is removed rather than baked, which
/// matches what printing the document would show.
pub fn flatten(doc: &mut PdfDocumentState) -> Result<(), DocumentError> {
    let page_count = doc.page_count();

    for page_index in 0..page_count {
        let has_annotations = doc
            .inner()
            .pages()
            .get(page_index as PdfPageIndex)
            .map(|page| !page.annotations().is_empty())
            .map_err(|err| DocumentError::Pdfium(err.to_string()))?;
        if !has_annotations {
            continue;
        }

        // See the module docs: flatten silently drops anything without an
        // appearance stream, and pdfium only creates those at render time.
        doc.realize_appearance_streams(page_index)?;

        let mut page = doc
            .inner_mut()
            .pages()
            .get(page_index as PdfPageIndex)
            .map_err(|err| DocumentError::Pdfium(err.to_string()))?;
        page.flatten()
            .map_err(|err| DocumentError::Pdfium(err.to_string()))?;
    }

    Ok(())
}

//! Saving the current document to disk, preserving editable annotations.
//!
//! A save has two parts: writing every current [`OverlayElement`] onto its
//! page as a real PDF annotation (via
//! [`crate::annotations::write_overlay_to_annotations`], grouped by
//! `page_index` since that function writes one page at a time), then asking
//! pdfium to write the whole document out to `path`. AcroForm field values
//! set via [`crate::form_fields::set_field_value`] are already live on the
//! in-memory [`PdfDocumentState`] by the time `save` is called, so they need
//! no separate step here — pdfium's document write persists them as part of
//! the same `FPDF_SaveAsCopy`/`FPDF_SaveWithVersion` call that writes
//! everything else.
//!
//! The result is always a standard, portable PDF: annotations are ordinary
//! PDF objects, so a file saved here (even one with un-applied overlay
//! elements still floating as annotations rather than flattened into page
//! content) opens correctly in any PDF viewer, not just this app.

use std::fs::File;
use std::path::{Path, PathBuf};

use crate::document::{DocumentError, PdfDocumentState};
use crate::overlay::OverlayLayer;

use crate::annotations::write_overlay_to_annotations;

/// Writes every element in `overlay` onto its page as an annotation, then
/// writes `doc` out to `path`.
///
/// Elements are grouped by `page_index` first because
/// [`write_overlay_to_annotations`] takes one page's elements at a time;
/// grouping here keeps `save` itself as the only place that needs to know
/// that.
pub fn save(
    doc: &mut PdfDocumentState,
    overlay: &OverlayLayer,
    path: &Path,
) -> Result<(), DocumentError> {
    let page_count = doc.page_count();

    for page_index in 0..page_count {
        let elements_on_page: Vec<_> = overlay
            .elements
            .iter()
            .filter(|element| element.page_index() == page_index)
            .cloned()
            .collect();

        if elements_on_page.is_empty() {
            continue;
        }

        write_overlay_to_annotations(doc, page_index, &elements_on_page)?;
    }

    write_document_atomically(doc, path)
}

/// Writes `doc` to `path` without ever truncating `path` itself mid-write.
///
/// pdfium loads documents lazily: the [`PdfDocumentState`] keeps reading
/// page objects from the file it was opened from for as long as it's open
/// (see `Pdfium::load_pdf_from_file`, which hands pdfium a live `File`).
/// pdfium-render's own `save_to_file` does `File::create(path)` first,
/// which — when `path` is that same open file, i.e. a plain "Save" —
/// truncates it to zero bytes *before* pdfium has read the objects it's
/// about to write out, destroying every page it hadn't touched yet.
///
/// Instead, the document is written to a fresh temporary file in the same
/// directory as `path` and only then `rename`d over it. `rename` within one
/// directory is atomic on macOS/APFS, and the still-open source `File`
/// keeps referring to the original (now unlinked) inode, so pdfium's lazy
/// reads keep working against the original bytes for the rest of the
/// session. On any failure the temporary file is removed and `path` is
/// left exactly as it was.
fn write_document_atomically(doc: &PdfDocumentState, path: &Path) -> Result<(), DocumentError> {
    // Write next to the real file, not next to a symlink pointing at it,
    // so the final rename replaces the file's contents rather than the
    // link — and so temp + target are guaranteed to be on one filesystem.
    let target = std::fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf());
    let dir = match target.parent() {
        Some(dir) if !dir.as_os_str().is_empty() => dir.to_path_buf(),
        _ => PathBuf::from("."),
    };
    let file_name = target
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_else(|| "document.pdf".to_string());
    let temp_path = dir.join(format!(".{file_name}.{}.tmp", uuid::Uuid::new_v4()));

    let result = (|| {
        let mut file = File::create(&temp_path).map_err(DocumentError::Io)?;
        doc.inner()
            .save_to_writer(&mut file)
            .map_err(|err| DocumentError::Pdfium(err.to_string()))?;
        file.sync_all().map_err(DocumentError::Io)?;

        // Keep an existing file's permissions (e.g. a 0600 private
        // document) rather than silently resetting them to the umask
        // default a brand-new file gets.
        if let Ok(metadata) = std::fs::metadata(&target) {
            std::fs::set_permissions(&temp_path, metadata.permissions())
                .map_err(DocumentError::Io)?;
        }

        std::fs::rename(&temp_path, &target).map_err(DocumentError::Io)
    })();

    if result.is_err() {
        let _ = std::fs::remove_file(&temp_path);
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::overlay::{OverlayElement, TextBoxElement};
    use uuid::Uuid;

    #[test]
    fn save_with_empty_overlay_still_writes_the_document() {
        let tmp = tempfile::tempdir().unwrap();
        let out_path = tmp.path().join("out.pdf");

        let mut doc =
            PdfDocumentState::open(Path::new("tests/fixtures/sample_plain.pdf")).unwrap();
        let overlay = OverlayLayer::default();

        save(&mut doc, &overlay, &out_path).unwrap();

        let reopened = PdfDocumentState::open(&out_path).unwrap();
        assert_eq!(reopened.page_count(), doc.page_count());
    }

    #[test]
    fn save_only_writes_elements_for_pages_they_belong_to() {
        let tmp = tempfile::tempdir().unwrap();
        let out_path = tmp.path().join("out.pdf");

        let mut doc =
            PdfDocumentState::open(Path::new("tests/fixtures/sample_plain.pdf")).unwrap();
        let mut overlay = OverlayLayer::default();
        overlay.add(OverlayElement::TextBox(TextBoxElement {
            id: Uuid::new_v4(),
            page_index: 0,
            rect: egui::Rect::from_min_size(egui::pos2(10.0, 10.0), egui::vec2(50.0, 20.0)),
            text: "On page 0".into(),
        }));

        save(&mut doc, &overlay, &out_path).unwrap();

        let reopened = PdfDocumentState::open(&out_path).unwrap();
        let elements = crate::annotations::read_overlay_from_annotations(&reopened, 0);
        assert_eq!(elements.len(), 1);
    }
}

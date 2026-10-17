//! Low-level PDF object edits that pdfium-render's safe API can't express.
//!
//! pdfium-render 0.9.4 exposes no way to remove or regenerate a form
//! widget's appearance stream (`/AP`): pdfium's own `FPDFAnnot_SetAP` and
//! the `FORM_*` focus/replace-selection calls that would regenerate it are
//! bound in `PdfiumLibraryBindings` but unreachable from outside the crate
//! (the bindings accessor and every raw `FPDF_*` handle getter are
//! `pub(crate)` — verified against the vendored
//! `pdfium-render-0.9.4/src/pdfium.rs` and `.../page/object/ownership.rs`).
//!
//! So the one structural edit this app needs — "forget this field's stale
//! appearance" — is done here on the serialized bytes, with `lopdf`, as an
//! **incremental update**: pdfium's own output is kept byte-for-byte and
//! only the edited widget/field dictionaries are appended after it, so an
//! object `lopdf` happens to parse imperfectly is never rewritten. The
//! caller ([`crate::form_fields::set_field_value`]) then reloads pdfium
//! from the result; pdfium regenerates a missing widget appearance from
//! the field's current `/V` the next time the page is drawn (verified
//! empirically: see `tests/form_fields_test.rs`).

use lopdf::{Dictionary, Document, IncrementalDocument, ObjectId};

use crate::document::DocumentError;

/// The result of [`invalidate_field_appearances`].
pub struct FieldAppearanceEdit {
    /// The updated document bytes.
    pub bytes: Vec<u8>,
    /// 0-based indices of the pages holding at least one of the edited
    /// widgets, in ascending order without duplicates.
    pub page_indices: Vec<usize>,
}

/// Removes the appearance stream (`/AP`) from every widget of the form
/// field whose fully-qualified name is `field_name`, and makes sure the
/// widget's current `/V` is also the value of its terminal field
/// dictionary (pdfium-render's `set_value` writes `/V` onto the widget
/// annotation's own dictionary, which for a widget that is a `/Kids` entry
/// of a separate field dictionary is not where pdfium reads the value
/// from when it regenerates the appearance).
///
/// Returns `Ok(None)` if no widget with that name could be found.
pub fn invalidate_field_appearances(
    bytes: Vec<u8>,
    field_name: &str,
) -> Result<Option<FieldAppearanceEdit>, DocumentError> {
    let mut incremental = IncrementalDocument::load_from(bytes.as_slice()).map_err(lopdf_error)?;

    let mut widget_ids: Vec<ObjectId> = Vec::new();
    let mut page_indices: Vec<usize> = Vec::new();
    {
        let prev = incremental.get_prev_documents();
        for (page_number, page_id) in prev.get_pages() {
            for widget_id in widget_annotation_ids(prev, page_id) {
                let Ok(widget) = prev.get_dictionary(widget_id) else {
                    continue;
                };
                if fully_qualified_name(prev, widget).as_deref() == Some(field_name) {
                    widget_ids.push(widget_id);
                    let page_index = page_number.saturating_sub(1) as usize;
                    if !page_indices.contains(&page_index) {
                        page_indices.push(page_index);
                    }
                }
            }
        }
    }

    if widget_ids.is_empty() {
        return Ok(None);
    }
    page_indices.sort_unstable();

    for widget_id in widget_ids {
        let (value, terminal_id) = {
            let prev = incremental.get_prev_documents();
            let widget = prev.get_dictionary(widget_id).map_err(lopdf_error)?;
            (widget.get(b"V").ok().cloned(), terminal_field_id(prev, widget_id))
        };

        incremental
            .opt_clone_object_to_new_document(widget_id)
            .map_err(lopdf_error)?;
        incremental
            .new_document
            .get_dictionary_mut(widget_id)
            .map_err(lopdf_error)?
            .remove(b"AP");

        if let (Some(value), Some(terminal_id)) = (value, terminal_id)
            && terminal_id != widget_id
        {
            incremental
                .opt_clone_object_to_new_document(terminal_id)
                .map_err(lopdf_error)?;
            incremental
                .new_document
                .get_dictionary_mut(terminal_id)
                .map_err(lopdf_error)?
                .set("V", value);
        }
    }

    let mut out = Vec::with_capacity(bytes.len() + 1024);
    incremental.save_to(&mut out).map_err(DocumentError::Io)?;

    Ok(Some(FieldAppearanceEdit {
        bytes: out,
        page_indices,
    }))
}

/// The object ids of every indirect `/Subtype /Widget` annotation listed in
/// page `page_id`'s `/Annots`. (Widgets are always indirect objects in
/// practice, since the AcroForm's `/Fields` tree must reference them.)
fn widget_annotation_ids(doc: &Document, page_id: ObjectId) -> Vec<ObjectId> {
    let Ok(page) = doc.get_dictionary(page_id) else {
        return Vec::new();
    };
    let Ok(annots) = page.get(b"Annots") else {
        return Vec::new();
    };
    let Ok((_, annots)) = doc.dereference(annots) else {
        return Vec::new();
    };
    let Ok(annots) = annots.as_array() else {
        return Vec::new();
    };

    annots
        .iter()
        .filter_map(|entry| entry.as_reference().ok())
        .filter(|id| {
            doc.get_dictionary(*id)
                .ok()
                .and_then(|dict| dict.get(b"Subtype").ok())
                .and_then(|subtype| subtype.as_name().ok())
                == Some(b"Widget".as_slice())
        })
        .collect()
}

/// The field's fully-qualified name (ISO 32000 12.7.3.2): the `/T` partial
/// names of the dictionary and its `/Parent` ancestors, root first, joined
/// with `.` — the same name pdfium reports via `FPDFAnnot_GetFormFieldName`.
fn fully_qualified_name(doc: &Document, widget: &Dictionary) -> Option<String> {
    let mut parts = Vec::new();
    let mut current = Some(widget);
    let mut depth = 0;

    while let Some(dict) = current {
        if let Ok(partial) = dict.get(b"T")
            && let Ok(partial) = lopdf::decode_text_string(partial)
        {
            parts.push(partial);
        }
        depth += 1;
        if depth > 64 {
            // A cyclic /Parent chain in a malformed file.
            return None;
        }
        current = dict
            .get(b"Parent")
            .ok()
            .and_then(|parent| parent.as_reference().ok())
            .and_then(|id| doc.get_dictionary(id).ok());
    }

    if parts.is_empty() {
        return None;
    }
    parts.reverse();
    Some(parts.join("."))
}

/// The id of the terminal field dictionary for widget `widget_id`: the
/// widget itself if it carries its own `/T` (a merged field/widget
/// dictionary), otherwise its nearest `/Parent` ancestor that does.
fn terminal_field_id(doc: &Document, widget_id: ObjectId) -> Option<ObjectId> {
    let mut current = widget_id;
    for _ in 0..64 {
        let dict = doc.get_dictionary(current).ok()?;
        if dict.has(b"T") {
            return Some(current);
        }
        current = dict.get(b"Parent").ok()?.as_reference().ok()?;
    }
    None
}

fn lopdf_error(err: lopdf::Error) -> DocumentError {
    DocumentError::Pdfium(format!("PDF structure edit failed: {err}"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use lopdf::Object;

    #[test]
    fn unknown_field_name_is_reported_as_none() {
        let bytes = std::fs::read("tests/fixtures/sample_form.pdf").unwrap();
        assert!(
            invalidate_field_appearances(bytes, "no_such_field")
                .unwrap()
                .is_none()
        );
    }

    fn only_widget(doc: &Document) -> &Dictionary {
        doc.objects
            .values()
            .filter_map(|object| object.as_dict().ok())
            .find(|dict| dict.get(b"Subtype").and_then(Object::as_name).ok() == Some(b"Widget"))
            .unwrap()
    }

    #[test]
    fn edit_appends_an_incremental_update_and_strips_ap() {
        // Rendering once makes pdfium generate (and keep) an appearance
        // stream for the prefilled value — the same state a real form
        // authored elsewhere arrives in.
        let doc = crate::document::PdfDocumentState::open(std::path::Path::new(
            "tests/fixtures/sample_form_prefilled.pdf",
        ))
        .unwrap();
        doc.render_page(0, 1.0).unwrap();
        let original = doc.inner().save_to_bytes().unwrap();
        assert!(only_widget(&Document::load_mem(&original).unwrap()).has(b"AP"));

        let edit = invalidate_field_appearances(original.clone(), "applicant_name")
            .unwrap()
            .unwrap();

        assert_eq!(edit.page_indices, vec![0]);
        // Incremental: the original bytes are kept verbatim as a prefix.
        assert!(edit.bytes.starts_with(&original));
        assert!(edit.bytes.len() > original.len());
        assert!(!only_widget(&Document::load_mem(&edit.bytes).unwrap()).has(b"AP"));
    }
}

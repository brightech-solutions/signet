//! AcroForm field enumeration, hit-testing, and filling.
//!
//! Form fields in pdfium are exposed as page annotations of type `Widget`
//! (or `XfaWidget`); this module enumerates those annotations on a page,
//! converts their rects into the same page-space coordinate system used by
//! [`crate::document::PdfDocumentState::render_page`] (PDF points, origin
//! top-left), and provides a simple linear hit-test plus a way to set a text
//! field's value by name.
//!
//! Only text fields are supported for editing (this task's scope); every
//! other field type (checkbox, radio, dropdown, etc.) is reported as
//! [`FieldKind::Other`] so the UI can safely ignore clicks on it rather than
//! guessing at a field type it doesn't support.

use pdfium_render::prelude::*;

use crate::document::{DocumentError, PdfDocumentState};

/// The kind of a form field, as far as this application cares: whether it's
/// a fillable text field, or anything else.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FieldKind {
    Text,
    Other,
}

/// A single AcroForm field found on a page, with its rect in page space
/// (PDF points, origin top-left, matching [`crate::document::PdfDocumentState::render_page`]
/// and [`crate::ui::canvas::page_to_screen`]).
#[derive(Debug, Clone)]
pub struct FormFieldHit {
    pub name: String,
    pub page_rect: egui::Rect,
    pub kind: FieldKind,
}

/// Enumerates the AcroForm fields on `page_index`.
///
/// Returns an empty `Vec` if the page index is out of bounds, the document
/// has no embedded form, or a field annotation has no name (unnamed fields
/// cannot be targeted by [`set_field_value`], so there is no use exposing
/// them as hit-testable).
pub fn fields_on_page(doc: &PdfDocumentState, page_index: usize) -> Vec<FormFieldHit> {
    let document = doc.inner();

    let page = match document.pages().get(page_index as PdfPageIndex) {
        Ok(page) => page,
        Err(_) => return Vec::new(),
    };

    let page_height = page.height().value;
    let annotations = page.annotations();

    let mut fields = Vec::with_capacity(annotations.len());

    for annotation in annotations.iter() {
        let Some(field) = annotation.as_form_field() else {
            continue;
        };

        let Some(name) = field.name() else {
            continue;
        };

        let Ok(bounds) = annotation.bounds() else {
            continue;
        };

        let kind = match field.field_type() {
            PdfFormFieldType::Text => FieldKind::Text,
            _ => FieldKind::Other,
        };

        // pdfium rects are in PDF space (origin bottom-left, y increasing
        // up); flip to the top-left-origin page space the rest of the app
        // uses by subtracting from the page height.
        let top_left = egui::pos2(bounds.left().value, page_height - bounds.top().value);
        let bottom_right = egui::pos2(bounds.right().value, page_height - bounds.bottom().value);

        fields.push(FormFieldHit {
            name,
            page_rect: egui::Rect::from_two_pos(top_left, bottom_right),
            kind,
        });
    }

    fields
}

/// Returns the first field in `fields` whose rect contains `page_point`, if
/// any. A plain linear scan: form fields per page are few enough (dozens at
/// most) that this doesn't need to be a spatial index.
// The brief's signature spells out `'a` explicitly; keeping it rather than
// letting it be elided, per clippy's `needless_lifetimes` suggestion.
#[allow(clippy::needless_lifetimes)]
pub fn hit_test<'a>(
    fields: &'a [FormFieldHit],
    page_point: egui::Pos2,
) -> Option<&'a FormFieldHit> {
    fields
        .iter()
        .find(|field| field.page_rect.contains(page_point))
}

/// Returns the current value of the text field named `name`, searching
/// every page of `doc`, or `None` if no field with that name exists, it
/// exists but isn't a text field, or it exists and is a text field but has
/// no value set.
///
/// Callers that seed an editable buffer from a field (e.g. the inline
/// text-field editor in `app.rs`) must use this rather than defaulting to
/// an empty string, since a text field opened from a pre-filled template or
/// a document re-opened after partial filling may already carry real
/// content that must not be silently discarded on the first edit.
pub fn get_field_value(doc: &PdfDocumentState, name: &str) -> Option<String> {
    let document = doc.inner();

    for page_index in 0..document.pages().len() {
        let page = document.pages().get(page_index).ok()?;
        let annotations = page.annotations();

        for annotation in annotations.iter() {
            let Some(field) = annotation.as_form_field() else {
                continue;
            };

            if field.name().as_deref() != Some(name) {
                continue;
            }

            return field
                .as_text_field()
                .and_then(|text_field| text_field.value());
        }
    }

    None
}

/// Sets the value of the text field named `name`, searching every page of
/// `doc`.
///
/// Returns `DocumentError::Pdfium` if no field with that name exists, or if
/// it exists but isn't a text field.
pub fn set_field_value(
    doc: &mut PdfDocumentState,
    name: &str,
    value: &str,
) -> Result<(), DocumentError> {
    let document = doc.inner_mut();
    let page_count = document.pages().len();

    for page_index in 0..page_count {
        let page = document
            .pages()
            .get(page_index)
            .map_err(|err| DocumentError::Pdfium(err.to_string()))?;

        let annotations = page.annotations();

        for annotation_index in annotations.as_range() {
            let mut annotation = match annotations.get(annotation_index) {
                Ok(annotation) => annotation,
                Err(_) => continue,
            };

            let is_match = annotation
                .as_form_field()
                .and_then(|field| field.name())
                .as_deref()
                == Some(name);

            if !is_match {
                continue;
            }

            return match annotation
                .as_form_field_mut()
                .and_then(|field| field.as_text_field_mut())
            {
                Some(text_field) => text_field
                    .set_value(value)
                    .map_err(|err| DocumentError::Pdfium(err.to_string())),
                None => Err(DocumentError::Pdfium(format!(
                    "field '{name}' exists but is not a text field"
                ))),
            };
        }
    }

    Err(DocumentError::Pdfium(format!(
        "no field named '{name}' found"
    )))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;

    #[test]
    fn hit_test_empty_fields_returns_none() {
        assert!(hit_test(&[], egui::pos2(0.0, 0.0)).is_none());
    }

    #[test]
    fn fields_on_page_returns_empty_for_out_of_bounds_page() {
        let doc = PdfDocumentState::open(Path::new("tests/fixtures/sample_form.pdf")).unwrap();
        assert!(fields_on_page(&doc, 99).is_empty());
    }

    #[test]
    fn set_field_value_errors_for_unknown_field() {
        let mut doc = PdfDocumentState::open(Path::new("tests/fixtures/sample_form.pdf")).unwrap();
        assert!(set_field_value(&mut doc, "does_not_exist", "x").is_err());
    }
}

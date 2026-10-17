//! Integration test for `pdf_utility::apply::flatten`: flattening must
//! actually remove both AcroForm fields and the app's own overlay
//! annotations as separate, editable objects — not just report success.

use std::path::Path;

use pdf_utility::annotations::{read_overlay_from_annotations, write_overlay_to_annotations};
use pdf_utility::apply::flatten;
use pdf_utility::document::PdfDocumentState;
use pdf_utility::form_fields::{fields_on_page, set_field_value};
use pdf_utility::overlay::{OverlayElement, OverlayLayer, TextBoxElement};
use uuid::Uuid;

#[test]
fn flatten_removes_fields_and_app_annotations() {
    let mut doc = PdfDocumentState::open(Path::new("tests/fixtures/sample_form.pdf")).unwrap();
    set_field_value(&mut doc, "applicant_name", "Jane Doe").unwrap();
    let mut overlay = OverlayLayer { elements: vec![] };
    overlay.add(OverlayElement::TextBox(TextBoxElement {
        id: Uuid::new_v4(),
        page_index: 0,
        rect: egui::Rect::from_min_size(egui::pos2(20.0, 30.0), egui::vec2(120.0, 18.0)),
        text: "Note".into(),
    }));
    write_overlay_to_annotations(&mut doc, 0, &overlay.elements).unwrap();

    flatten(&mut doc).unwrap();

    assert!(fields_on_page(&doc, 0).is_empty());
    assert!(read_overlay_from_annotations(&doc, 0).is_empty());
}

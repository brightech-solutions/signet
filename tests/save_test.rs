use std::path::Path;

use pdf_utility::annotations::read_overlay_from_annotations;
use pdf_utility::document::PdfDocumentState;
use pdf_utility::form_fields::{fields_on_page, get_field_value, set_field_value};
use pdf_utility::overlay::{OverlayElement, OverlayLayer, TextBoxElement};
use pdf_utility::save::save;
use uuid::Uuid;

#[test]
fn saved_field_value_and_text_box_are_both_readable_after_reopen() {
    let tmp = tempfile::tempdir().unwrap();
    let out_path = tmp.path().join("out.pdf");

    let mut doc = PdfDocumentState::open(Path::new("tests/fixtures/sample_form.pdf")).unwrap();
    set_field_value(&mut doc, "applicant_name", "Jane Doe").unwrap();
    let mut overlay = OverlayLayer { elements: vec![] };
    overlay.add(OverlayElement::TextBox(TextBoxElement {
        id: Uuid::new_v4(),
        page_index: 0,
        rect: egui::Rect::from_min_size(egui::pos2(20.0, 30.0), egui::vec2(120.0, 18.0)),
        text: "Note".into(),
    }));
    save(&mut doc, &overlay, &out_path).unwrap();

    let reopened = PdfDocumentState::open(&out_path).unwrap();
    let fields = fields_on_page(&reopened, 0);
    assert!(fields.iter().any(|f| f.name == "applicant_name"));
    // Beyond the brief's literal assertion (field still present), confirm
    // the *value* itself survived the save/reopen round trip, not just the
    // field's existence.
    assert_eq!(
        get_field_value(&reopened, "applicant_name"),
        Some("Jane Doe".to_string())
    );
    let elements = read_overlay_from_annotations(&reopened, 0);
    assert!(elements.iter().any(|e| matches!(e, OverlayElement::TextBox(tb) if tb.text == "Note")));
}

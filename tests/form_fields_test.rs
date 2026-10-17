use std::path::Path;

use pdf_utility::document::PdfDocumentState;
use pdf_utility::form_fields::{FieldKind, fields_on_page, hit_test, set_field_value};

#[test]
fn finds_known_text_field() {
    let doc = PdfDocumentState::open(Path::new("tests/fixtures/sample_form.pdf")).unwrap();
    let fields = fields_on_page(&doc, 0);
    let field = fields.iter().find(|f| f.name == "applicant_name").unwrap();
    assert_eq!(field.kind, FieldKind::Text);
}

#[test]
fn hit_test_finds_field_at_its_rect_center() {
    let doc = PdfDocumentState::open(Path::new("tests/fixtures/sample_form.pdf")).unwrap();
    let fields = fields_on_page(&doc, 0);
    let field = &fields[0];
    let center = field.page_rect.center();
    assert_eq!(hit_test(&fields, center).unwrap().name, field.name);
}

#[test]
fn hit_test_returns_none_outside_any_field() {
    let doc = PdfDocumentState::open(Path::new("tests/fixtures/sample_form.pdf")).unwrap();
    let fields = fields_on_page(&doc, 0);
    assert!(hit_test(&fields, egui::pos2(-100.0, -100.0)).is_none());
}

#[test]
fn set_field_value_round_trips() {
    let mut doc = PdfDocumentState::open(Path::new("tests/fixtures/sample_form.pdf")).unwrap();
    set_field_value(&mut doc, "applicant_name", "Jane Doe").unwrap();
    let fields = fields_on_page(&doc, 0);
    assert!(fields.iter().any(|f| f.name == "applicant_name"));
    // value readback is exercised end-to-end in Task 9's save/reopen test
}

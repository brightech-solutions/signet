use std::path::Path;

use pdf_utility::document::PdfDocumentState;
use pdf_utility::form_fields::{
    FieldKind, fields_on_page, get_field_value, hit_test, set_field_value,
};

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
    assert_eq!(
        get_field_value(&doc, "applicant_name"),
        Some("Jane Doe".to_string())
    );
}

/// Finding 1 (fix round 1): a text field that already has a non-empty `/V`
/// before the application ever touches it (a pre-filled template, or a
/// document re-opened after partial filling by another tool) must have its
/// real value readable via `get_field_value`, so callers can seed an inline
/// editor from it instead of defaulting to an empty buffer and silently
/// discarding the existing content on first keystroke. `sample_form.pdf`'s
/// `applicant_name` field has `/V ()` (empty), which would not catch a
/// regression here, so this test uses a second fixture whose field starts
/// with a non-empty `/V (Jane Smith)`.
#[test]
fn get_field_value_reads_existing_non_empty_value() {
    let doc =
        PdfDocumentState::open(Path::new("tests/fixtures/sample_form_prefilled.pdf")).unwrap();
    assert_eq!(
        get_field_value(&doc, "applicant_name"),
        Some("Jane Smith".to_string())
    );
}

#[test]
fn get_field_value_returns_none_for_unknown_field() {
    let doc = PdfDocumentState::open(Path::new("tests/fixtures/sample_form.pdf")).unwrap();
    assert_eq!(get_field_value(&doc, "does_not_exist"), None);
}

/// Finding 2 (fix round 1): pins the absolute PDF-to-page coordinate flip
/// in `fields_on_page` against the fixture's real `/Rect [100 650 300 680]`
/// and its MediaBox height of 792, rather than only exercising it via a
/// flip-invariant hit test (clicking a rect's own center hits it regardless
/// of flip direction, so `hit_test_finds_field_at_its_rect_center` could not
/// catch a sign-flip regression here).
///
/// PDF rect space is bottom-left-origin: left=100, bottom=650, right=300,
/// top=680. Flipping to the top-left-origin page space
/// `fields_on_page`/`render_page`/`page_to_screen` use:
///   top    = page_height - rect_top_pdf    = 792 - 680 = 112
///   bottom = page_height - rect_bottom_pdf = 792 - 650 = 142
/// left/right are unaffected by the vertical flip.
#[test]
fn fields_on_page_flips_rect_to_top_left_origin() {
    let doc = PdfDocumentState::open(Path::new("tests/fixtures/sample_form.pdf")).unwrap();
    let fields = fields_on_page(&doc, 0);
    let field = fields.iter().find(|f| f.name == "applicant_name").unwrap();

    let expected = egui::Rect::from_min_max(egui::pos2(100.0, 112.0), egui::pos2(300.0, 142.0));

    let epsilon = 0.01;
    assert!(
        (field.page_rect.min.x - expected.min.x).abs() < epsilon
            && (field.page_rect.min.y - expected.min.y).abs() < epsilon
            && (field.page_rect.max.x - expected.max.x).abs() < epsilon
            && (field.page_rect.max.y - expected.max.y).abs() < epsilon,
        "expected page_rect {:?}, got {:?}",
        expected,
        field.page_rect
    );
}

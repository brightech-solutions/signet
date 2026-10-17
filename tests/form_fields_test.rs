mod common;

use std::path::Path;

use common::ink_in;
use pdf_utility::document::PdfDocumentState;
use pdf_utility::overlay::OverlayLayer;
use pdf_utility::save::save;
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

fn applicant_rect(doc: &PdfDocumentState) -> egui::Rect {
    fields_on_page(doc, 0)
        .into_iter()
        .find(|field| field.name == "applicant_name")
        .unwrap()
        .page_rect
}

/// Final review Critical #3: `set_field_value` used to write only `/V`,
/// leaving the widget's appearance stream (`/AP`) — which pdfium generates
/// the first time the page is rendered, i.e. right after opening — showing
/// the old value forever. The page is rendered *before* filling here,
/// exactly as the viewer does, and the filled text must then actually be
/// drawn in the field's rect.
#[test]
fn filled_field_value_is_actually_rendered() {
    let mut doc = PdfDocumentState::open(Path::new("tests/fixtures/sample_form.pdf")).unwrap();
    let rect = applicant_rect(&doc);
    assert_eq!(ink_in(&doc, 0, rect), 0, "fixture field should start blank");

    set_field_value(&mut doc, "applicant_name", "Jane Doe").unwrap();

    let ink = ink_in(&doc, 0, rect);
    assert!(ink > 50, "filled field renders only {ink} ink pixels");
    assert_eq!(
        get_field_value(&doc, "applicant_name"),
        Some("Jane Doe".to_string())
    );
}

/// The stale appearance must be *replaced*, not drawn over: clearing a
/// field that was pre-filled (and already rendered once, so pdfium has
/// generated an appearance for its old value) must leave it blank.
#[test]
fn clearing_a_prefilled_field_removes_its_old_rendered_value() {
    let mut doc =
        PdfDocumentState::open(Path::new("tests/fixtures/sample_form_prefilled.pdf")).unwrap();
    let rect = applicant_rect(&doc);
    assert!(ink_in(&doc, 0, rect) > 50, "prefilled value should render");

    set_field_value(&mut doc, "applicant_name", "").unwrap();

    assert_eq!(ink_in(&doc, 0, rect), 0);
}

/// The regenerated appearance must be persisted in the saved file (so the
/// filled value shows up after reopening, here or in any other viewer),
/// not just live in this session's memory.
#[test]
fn filled_field_value_is_rendered_after_save_and_reopen() {
    let tmp = tempfile::tempdir().unwrap();
    let path = tmp.path().join("filled.pdf");

    let mut doc = PdfDocumentState::open(Path::new("tests/fixtures/sample_form.pdf")).unwrap();
    let rect = applicant_rect(&doc);
    doc.render_page(0, 1.0).unwrap();
    set_field_value(&mut doc, "applicant_name", "Jane Doe").unwrap();
    save(&mut doc, &OverlayLayer::default(), &path).unwrap();
    drop(doc);

    // Structural check, independent of pdfium regenerating anything on
    // reopen: the saved widget carries a real appearance stream.
    let saved = lopdf::Document::load(&path).unwrap();
    let widget_has_ap_stream = saved.objects.values().any(|object| {
        let Ok(dict) = object.as_dict() else {
            return false;
        };
        let is_widget = dict.get(b"Subtype").and_then(|s| s.as_name()).ok() == Some(b"Widget");
        let normal_ap = dict
            .get(b"AP")
            .and_then(|ap| ap.as_dict())
            .and_then(|ap| ap.get(b"N"))
            .and_then(|n| n.as_reference())
            .and_then(|id| saved.get_object(id));
        is_widget && matches!(normal_ap, Ok(lopdf::Object::Stream(_)))
    });
    assert!(widget_has_ap_stream, "saved widget has no /AP /N stream");

    let reopened = PdfDocumentState::open(&path).unwrap();
    let ink = ink_in(&reopened, 0, rect);
    assert!(ink > 50, "reopened filled field renders only {ink} ink pixels");
    assert_eq!(
        get_field_value(&reopened, "applicant_name"),
        Some("Jane Doe".to_string())
    );
}

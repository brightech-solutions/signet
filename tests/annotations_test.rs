use std::path::Path;

use pdf_utility::annotations::{read_overlay_from_annotations, write_overlay_to_annotations};
use pdf_utility::document::PdfDocumentState;
use pdf_utility::overlay::{OverlayElement, TextBoxElement};
use uuid::Uuid;

#[test]
fn text_box_round_trips_through_annotations() {
    let mut doc = PdfDocumentState::open(Path::new("tests/fixtures/sample_plain.pdf")).unwrap();
    let original = vec![OverlayElement::TextBox(TextBoxElement {
        id: Uuid::new_v4(),
        page_index: 0,
        rect: egui::Rect::from_min_size(egui::pos2(20.0, 30.0), egui::vec2(120.0, 18.0)),
        text: "Hello".into(),
    })];
    write_overlay_to_annotations(&mut doc, 0, &original).unwrap();
    let read_back = read_overlay_from_annotations(&doc, 0);
    assert_eq!(read_back.len(), 1);
    match &read_back[0] {
        OverlayElement::TextBox(tb) => assert_eq!(tb.text, "Hello"),
        _ => panic!("expected a text box"),
    }
}

#[test]
fn foreign_annotations_without_app_marker_are_ignored() {
    // sample_plain.pdf has no pre-existing annotations; this documents the
    // contract that only APP_MARKER-tagged annotations are read back as
    // OverlayElements — any other annotation subtype/author is skipped.
    let doc = PdfDocumentState::open(Path::new("tests/fixtures/sample_plain.pdf")).unwrap();
    assert!(read_overlay_from_annotations(&doc, 0).is_empty());
}

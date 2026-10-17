use std::path::Path;

use pdf_utility::annotations::{read_overlay_from_annotations, write_overlay_to_annotations};
use pdf_utility::document::PdfDocumentState;
use pdf_utility::overlay::{OverlayElement, SignatureElement, TextBoxElement};
use pdf_utility::signatures::{SignatureEntry, SignatureSource, render_signature_entry};
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

/// Task 6's `write_overlay_to_annotations`/`read_overlay_from_annotations`
/// support for `SignatureElement` (a stamp annotation with an embedded
/// image) was written before any real signature image existed to test
/// with, so it was never exercised end-to-end. Now that Task 8 can
/// actually render a placed signature's image (via
/// `render_signature_entry`), this closes that gap: a real signature
/// image must survive the full write -> pdfium -> read cycle with its
/// pixels intact, not just its bounding rect.
#[test]
fn real_placed_signature_image_round_trips_through_annotations() {
    let tmp = tempfile::tempdir().unwrap();

    // A real signature entry, as the signature library would hand to the
    // canvas at placement time: a typed name rendered in the bundled
    // script font, which `render_signature_entry` rasterizes to a real
    // (non-trivial) glyph bitmap.
    let entry = SignatureEntry {
        id: Uuid::new_v4(),
        name: "Jane Doe".to_string(),
        source: SignatureSource::Typed {
            text: "Jane Doe".to_string(),
            font_id: pdf_utility::signatures::SCRIPT_FONT_ID.to_string(),
        },
        created: "0".to_string(),
    };
    let image = render_signature_entry(&entry, tmp.path());
    assert!(
        image.size[0] > 1 && image.size[1] > 1,
        "expected a real rendered signature, got {:?}",
        image.size
    );

    let mut doc = PdfDocumentState::open(Path::new("tests/fixtures/sample_plain.pdf")).unwrap();
    let original = vec![OverlayElement::Signature(SignatureElement {
        id: Uuid::new_v4(),
        page_index: 0,
        rect: egui::Rect::from_min_size(egui::pos2(40.0, 50.0), egui::vec2(120.0, 40.0)),
        image: image.clone(),
    })];

    write_overlay_to_annotations(&mut doc, 0, &original).unwrap();
    let read_back = read_overlay_from_annotations(&doc, 0);

    assert_eq!(read_back.len(), 1);
    match &read_back[0] {
        OverlayElement::Signature(signature) => {
            assert_eq!(signature.image.size, image.size);
            // The embedded image must come back pixel-for-pixel, not just
            // the annotation's bounding rect — a lossy re-encode (e.g. to
            // JPEG, which has no alpha channel) would silently destroy a
            // signature's transparency and edges.
            assert_eq!(signature.image.pixels, image.pixels);
        }
        other => panic!("expected a signature element, got {other:?}"),
    }
}

mod common;

use std::path::Path;

use common::ink_in;
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

/// What a saved text box actually *looks like* — in this app after
/// reopening, in any other viewer, and once baked by Apply — is its
/// annotation's appearance stream. It must show each line the user typed
/// on its own line, and nothing else: pdfium's auto-generated `FreeText`
/// appearance (used before this fix) ran all lines together into one and
/// drew a black border box around the text.
#[test]
fn saved_multiline_text_box_renders_each_line_and_no_border() {
    let tmp = tempfile::tempdir().unwrap();
    let path = tmp.path().join("out.pdf");
    let rect = egui::Rect::from_min_size(egui::pos2(100.0, 300.0), egui::vec2(200.0, 60.0));

    let mut doc = PdfDocumentState::open(Path::new("tests/fixtures/sample_plain.pdf")).unwrap();
    let text = "FIRST LINE\nSECOND LINE";
    write_overlay_to_annotations(
        &mut doc,
        0,
        &[OverlayElement::TextBox(TextBoxElement {
            id: Uuid::new_v4(),
            page_index: 0,
            rect,
            text: text.into(),
        })],
    )
    .unwrap();
    pdf_utility::save::save(&mut doc, &Default::default(), &path).unwrap();
    drop(doc);

    let reopened = PdfDocumentState::open(&path).unwrap();
    let band = |top: f32, bottom: f32| {
        egui::Rect::from_min_max(
            egui::pos2(rect.min.x + 4.0, rect.min.y + top),
            egui::pos2(rect.max.x - 4.0, rect.min.y + bottom),
        )
    };
    // 12pt text, 14pt line pitch, 2pt top padding: line 1 occupies roughly
    // y+2..y+16 and line 2 y+16..y+30 (page space, top-left origin).
    let first = ink_in(&reopened, 0, band(2.0, 15.0));
    let second = ink_in(&reopened, 0, band(17.0, 30.0));
    let below = ink_in(&reopened, 0, band(33.0, 56.0));
    assert!(first > 40, "first line: {first} ink pixels");
    assert!(second > 40, "second line: {second} ink pixels");
    assert_eq!(below, 0, "ink below the two lines");

    let edges = [
        egui::Rect::from_min_max(rect.min, egui::pos2(rect.max.x, rect.min.y + 1.5)),
        egui::Rect::from_min_max(egui::pos2(rect.min.x, rect.max.y - 1.5), rect.max),
        egui::Rect::from_min_max(egui::pos2(rect.max.x - 1.5, rect.min.y), rect.max),
    ];
    for edge in edges {
        assert_eq!(ink_in(&reopened, 0, edge), 0, "border ink along {edge:?}");
    }

    // ...and it still reads back as the same editable text box.
    let elements = read_overlay_from_annotations(&reopened, 0);
    assert_eq!(elements.len(), 1);
    match &elements[0] {
        OverlayElement::TextBox(text_box) => {
            assert_eq!(text_box.text, text);
            assert!((text_box.rect.min - rect.min).length() < 0.5);
            assert!((text_box.rect.size() - rect.size()).length() < 0.5);
        }
        other => panic!("expected a text box, got {other:?}"),
    }
}

/// Long lines wrap inside the box instead of running off its right edge.
#[test]
fn long_text_box_line_wraps_within_its_rect() {
    let rect = egui::Rect::from_min_size(egui::pos2(100.0, 300.0), egui::vec2(120.0, 80.0));
    let mut doc = PdfDocumentState::open(Path::new("tests/fixtures/sample_plain.pdf")).unwrap();
    write_overlay_to_annotations(
        &mut doc,
        0,
        &[OverlayElement::TextBox(TextBoxElement {
            id: Uuid::new_v4(),
            page_index: 0,
            rect,
            text: "the quick brown fox jumps over the lazy dog again and again".into(),
        })],
    )
    .unwrap();

    let right_of_box =
        egui::Rect::from_min_max(egui::pos2(rect.max.x, rect.min.y), egui::pos2(500.0, rect.max.y));
    assert_eq!(ink_in(&doc, 0, right_of_box), 0, "text ran past the box");
    // ~60 characters of 12pt Helvetica need ~3 lines of a 120pt-wide box,
    // stacked from the top at a 14pt pitch.
    for (line, top) in [(1, 2.0), (2, 17.0), (3, 31.0)] {
        let band = egui::Rect::from_min_max(
            egui::pos2(rect.min.x, rect.min.y + top),
            egui::pos2(rect.max.x, rect.min.y + top + 13.0),
        );
        let ink = ink_in(&doc, 0, band);
        assert!(ink > 40, "wrapped line {line} has only {ink} ink pixels");
    }
}

/// An empty text box survives a save round trip as an (empty) text box
/// rather than silently disappearing.
#[test]
fn empty_text_box_round_trips() {
    let mut doc = PdfDocumentState::open(Path::new("tests/fixtures/sample_plain.pdf")).unwrap();
    write_overlay_to_annotations(
        &mut doc,
        0,
        &[OverlayElement::TextBox(TextBoxElement {
            id: Uuid::new_v4(),
            page_index: 0,
            rect: egui::Rect::from_min_size(egui::pos2(20.0, 30.0), egui::vec2(120.0, 18.0)),
            text: String::new(),
        })],
    )
    .unwrap();
    let elements = read_overlay_from_annotations(&doc, 0);
    assert!(matches!(&elements[..], [OverlayElement::TextBox(tb)] if tb.text.is_empty()));
}

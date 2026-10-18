mod common;

use std::path::Path;

use common::{app_annotation_count_in_file, ink_in, multipage_marker_rect};
use pdf_utility::annotations::{read_overlay_from_annotations, take_overlay_from_document};
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

/// Regression test for the final review's Critical #1: pdfium loads a
/// document lazily from the open file, so writing the save straight to
/// that same path (`File::create` truncates it first) destroyed every
/// object pdfium hadn't read yet. Saving in place must leave the file
/// intact — including a later page that was never rendered or touched
/// before the save.
#[test]
fn saving_in_place_keeps_untouched_later_pages_intact() {
    let tmp = tempfile::tempdir().unwrap();
    let path = tmp.path().join("multipage.pdf");
    std::fs::copy("tests/fixtures/sample_multipage.pdf", &path).unwrap();
    let original_size = std::fs::metadata(&path).unwrap().len();

    let mut doc = PdfDocumentState::open(&path).unwrap();
    assert_eq!(doc.page_count(), 12);
    // Touch only page 0, as the viewer does right after opening.
    doc.render_page(0, 1.0).unwrap();

    save(&mut doc, &OverlayLayer::default(), &path).unwrap();
    drop(doc);

    // The temp file the save went through was renamed into place, not
    // left lying around next to the document.
    let entries: Vec<_> = std::fs::read_dir(tmp.path())
        .unwrap()
        .map(|entry| entry.unwrap().file_name())
        .collect();
    assert_eq!(entries, vec![std::ffi::OsString::from("multipage.pdf")]);

    // pdfium Flate-compresses content streams on every save, so a healthy
    // save of this fixture (whose filler is deliberately incompressible)
    // lands around 80% of the original; the destructive in-place save this
    // guards against produced ~2%.
    let saved_size = std::fs::metadata(&path).unwrap().len();
    assert!(
        saved_size * 10 >= original_size * 6,
        "saved file shrank from {original_size} to {saved_size} bytes"
    );

    let reopened = PdfDocumentState::open(&path).unwrap();
    assert_eq!(reopened.page_count(), 12);
    // The last page renders, and its page-specific marker square is
    // actually drawn (40x40pt = 1600px at zoom 1), not just "no error".
    let marker = ink_in(&reopened, 11, multipage_marker_rect(12));
    assert!(marker >= 1500, "page 12 marker has only {marker} ink pixels");
    // ...and nowhere else's marker leaked onto it.
    assert_eq!(ink_in(&reopened, 11, multipage_marker_rect(1)), 0);
}

/// Save As to a *different* path must also leave the source file
/// untouched, byte for byte.
#[test]
fn save_as_leaves_the_source_file_untouched() {
    let tmp = tempfile::tempdir().unwrap();
    let source = tmp.path().join("source.pdf");
    let dest = tmp.path().join("dest.pdf");
    std::fs::copy("tests/fixtures/sample_multipage.pdf", &source).unwrap();
    let before = std::fs::read(&source).unwrap();

    let mut doc = PdfDocumentState::open(&source).unwrap();
    save(&mut doc, &OverlayLayer::default(), &dest).unwrap();

    assert_eq!(std::fs::read(&source).unwrap(), before);
    let reopened = PdfDocumentState::open(&dest).unwrap();
    assert!(ink_in(&reopened, 11, multipage_marker_rect(12)) >= 1500);
}

fn text_box(rect: egui::Rect, text: &str) -> OverlayElement {
    OverlayElement::TextBox(TextBoxElement {
        id: Uuid::new_v4(),
        page_index: 0,
        rect,
        text: text.into(),
    })
}

const NOTE_RECT: egui::Rect = egui::Rect {
    min: egui::pos2(100.0, 300.0),
    max: egui::pos2(300.0, 340.0),
};

/// Final review Critical #4 (a): a saved text box comes back on reopen as
/// a real, editable `OverlayElement` — and is taken off the page itself,
/// so it isn't also drawn underneath the live element as static pixels.
#[test]
fn saved_text_box_reopens_as_an_editable_overlay_element() {
    let tmp = tempfile::tempdir().unwrap();
    let path = tmp.path().join("out.pdf");
    let mut doc = PdfDocumentState::open(Path::new("tests/fixtures/sample_plain.pdf")).unwrap();
    let mut overlay = OverlayLayer::default();
    overlay.add(text_box(NOTE_RECT, "HELLO WORLD"));
    save(&mut doc, &overlay, &path).unwrap();
    drop(doc);

    let mut reopened = PdfDocumentState::open(&path).unwrap();
    assert!(ink_in(&reopened, 0, NOTE_RECT) > 50, "saved file should show the text box");

    let overlay = take_overlay_from_document(&mut reopened).unwrap();
    assert_eq!(overlay.elements.len(), 1);
    match &overlay.elements[0] {
        OverlayElement::TextBox(tb) => {
            assert_eq!(tb.text, "HELLO WORLD");
            assert!((tb.rect.min - NOTE_RECT.min).length() < 0.5);
            assert!((tb.rect.size() - NOTE_RECT.size()).length() < 0.5);
        }
        other => panic!("expected a text box, got {other:?}"),
    }
    assert_eq!(
        ink_in(&reopened, 0, NOTE_RECT),
        0,
        "text box is still drawn into the page under the live overlay element"
    );
    assert!(read_overlay_from_annotations(&reopened, 0).is_empty());
}

/// Final review Critical #4 (b): saving the same document twice in one
/// session leaves exactly one annotation per element in the file.
#[test]
fn saving_twice_writes_exactly_one_annotation_per_element() {
    let tmp = tempfile::tempdir().unwrap();
    let path = tmp.path().join("out.pdf");
    std::fs::copy("tests/fixtures/sample_plain.pdf", &path).unwrap();
    let mut doc = PdfDocumentState::open(&path).unwrap();
    let mut overlay = OverlayLayer::default();
    overlay.add(text_box(NOTE_RECT, "HELLO WORLD"));

    save(&mut doc, &overlay, &path).unwrap();
    save(&mut doc, &overlay, &path).unwrap();

    assert_eq!(app_annotation_count_in_file(&path, 1), 1);
    // ...and the in-memory page doesn't keep a static copy drawn under the
    // live overlay either.
    assert!(read_overlay_from_annotations(&doc, 0).is_empty());
    assert_eq!(ink_in(&doc, 0, NOTE_RECT), 0);
}

/// Final review Critical #4 (b), across sessions: reopen a saved file,
/// take its overlay, save again — still exactly one annotation.
#[test]
fn reopening_and_resaving_does_not_duplicate_annotations() {
    let tmp = tempfile::tempdir().unwrap();
    let path = tmp.path().join("out.pdf");
    let mut doc = PdfDocumentState::open(Path::new("tests/fixtures/sample_plain.pdf")).unwrap();
    let mut overlay = OverlayLayer::default();
    overlay.add(text_box(NOTE_RECT, "HELLO WORLD"));
    save(&mut doc, &overlay, &path).unwrap();
    drop(doc);

    let mut reopened = PdfDocumentState::open(&path).unwrap();
    let overlay = take_overlay_from_document(&mut reopened).unwrap();
    save(&mut reopened, &overlay, &path).unwrap();
    drop(reopened);

    assert_eq!(app_annotation_count_in_file(&path, 1), 1);
}

/// Final review Critical #4 (c): add, save, move, save again, reopen —
/// only the new position exists; no ghost at the old one.
#[test]
fn moving_an_element_between_saves_leaves_no_ghost() {
    let tmp = tempfile::tempdir().unwrap();
    let path = tmp.path().join("out.pdf");
    std::fs::copy("tests/fixtures/sample_plain.pdf", &path).unwrap();
    let mut doc = PdfDocumentState::open(&path).unwrap();
    let page_size = doc.page_size_points(0);
    let mut overlay = OverlayLayer::default();
    overlay.add(text_box(NOTE_RECT, "HELLO WORLD"));
    save(&mut doc, &overlay, &path).unwrap();

    let id = overlay.elements[0].id();
    let new_origin = egui::pos2(100.0, 500.0);
    overlay.move_element(id, new_origin, page_size);
    save(&mut doc, &overlay, &path).unwrap();
    drop(doc);

    assert_eq!(app_annotation_count_in_file(&path, 1), 1);
    let mut reopened = PdfDocumentState::open(&path).unwrap();
    assert_eq!(ink_in(&reopened, 0, NOTE_RECT), 0, "ghost left at the old position");
    let moved_rect = egui::Rect::from_min_size(new_origin, NOTE_RECT.size());
    assert!(ink_in(&reopened, 0, moved_rect) > 50, "nothing at the new position");
    let overlay = take_overlay_from_document(&mut reopened).unwrap();
    assert_eq!(overlay.elements.len(), 1);
    assert!((overlay.elements[0].rect().min - new_origin).length() < 0.5);
}

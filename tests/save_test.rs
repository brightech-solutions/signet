mod common;

use std::path::Path;

use common::{ink_in, multipage_marker_rect};
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

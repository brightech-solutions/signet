//! Integration tests for Apply: every addition (filled fields, text boxes,
//! signatures) must end up genuinely *drawn* in the page content — checked
//! by counting rendered ink pixels in each element's rect — and must stop
//! being separate, editable objects. Checking only that the
//! annotations/fields are gone is what let a version that silently erased
//! every addition pass before.

mod common;

use std::path::Path;

use common::ink_in;
use signet::annotations::read_overlay_from_annotations;
use signet::apply::{apply, flatten};
use signet::document::PdfDocumentState;
use signet::form_fields::{fields_on_page, set_field_value};
use signet::overlay::{OverlayElement, OverlayLayer, SignatureElement, TextBoxElement};
use signet::save::save;
use uuid::Uuid;

const TEXT_RECT: egui::Rect = egui::Rect {
    min: egui::pos2(100.0, 300.0),
    max: egui::pos2(300.0, 340.0),
};
const SIGNATURE_RECT: egui::Rect = egui::Rect {
    min: egui::pos2(100.0, 400.0),
    max: egui::pos2(260.0, 460.0),
};

/// A signature-like image: transparent, with an opaque dark stroke band
/// through the middle third — so a correct bake shows ink there, and a
/// bake that lost the alpha channel would show a solid block instead.
fn signature_image() -> egui::ColorImage {
    let (width, height) = (160, 60);
    let mut image = egui::ColorImage::filled([width, height], egui::Color32::TRANSPARENT);
    for y in 20..40 {
        for x in 0..width {
            image.pixels[y * width + x] = egui::Color32::from_rgb(10, 10, 60);
        }
    }
    image
}

fn overlay_with_text_box_and_signature() -> OverlayLayer {
    let mut overlay = OverlayLayer::default();
    overlay.add(OverlayElement::TextBox(TextBoxElement {
        id: Uuid::new_v4(),
        page_index: 0,
        rect: TEXT_RECT,
        text: "HELLO WORLD".into(),
    }));
    overlay.add(OverlayElement::Signature(SignatureElement {
        id: Uuid::new_v4(),
        page_index: 0,
        rect: SIGNATURE_RECT,
        image: signature_image(),
    }));
    overlay
}

fn field_rect(doc: &PdfDocumentState) -> egui::Rect {
    fields_on_page(doc, 0)
        .into_iter()
        .find(|field| field.name == "applicant_name")
        .unwrap()
        .page_rect
}

/// Final review Critical #2 (and the Apply half of #3): text box,
/// signature and filled field are all visibly baked into the page, both
/// immediately and after Save + reopen, and none of them is editable any
/// more.
#[test]
fn apply_bakes_text_box_signature_and_filled_field_into_page_content() {
    let tmp = tempfile::tempdir().unwrap();
    let out_path = tmp.path().join("applied.pdf");

    let mut doc = PdfDocumentState::open(Path::new("tests/fixtures/sample_form.pdf")).unwrap();
    let field = field_rect(&doc);
    // The viewer renders the page as soon as it opens.
    doc.render_page(0, 1.0).unwrap();
    for rect in [field, TEXT_RECT, SIGNATURE_RECT] {
        assert_eq!(ink_in(&doc, 0, rect), 0, "fixture region {rect:?} should start blank");
    }

    set_field_value(&mut doc, "applicant_name", "Jane Doe").unwrap();
    apply(&mut doc, &overlay_with_text_box_and_signature()).unwrap();

    let check = |doc: &PdfDocumentState, when: &str| {
        let field_ink = ink_in(doc, 0, field);
        let text_ink = ink_in(doc, 0, TEXT_RECT);
        let signature_ink = ink_in(doc, 0, SIGNATURE_RECT);
        eprintln!("{when}: field={field_ink} text={text_ink} signature={signature_ink}");
        assert!(field_ink > 50, "{when}: field has {field_ink} ink pixels");
        assert!(text_ink > 50, "{when}: text box has {text_ink} ink pixels");
        // The stroke band is 1/3 of the 160x60pt rect (3200px); the
        // transparent rest must stay transparent (no solid block).
        assert!(
            (2500..=4500).contains(&signature_ink),
            "{when}: signature has {signature_ink} ink pixels"
        );
        assert!(fields_on_page(doc, 0).is_empty(), "{when}: field still editable");
        assert!(
            read_overlay_from_annotations(doc, 0).is_empty(),
            "{when}: overlay annotations still present"
        );
    };

    check(&doc, "after apply");

    save(&mut doc, &OverlayLayer::default(), &out_path).unwrap();
    drop(doc);
    let reopened = PdfDocumentState::open(&out_path).unwrap();
    check(&reopened, "after save + reopen");
}

/// The lower-level `flatten` must bake what's already on the document
/// even when nothing has been rendered yet (pdfium generates missing
/// appearance streams lazily, at render time, and its native flatten
/// silently drops annotations that don't have one).
#[test]
fn flatten_bakes_a_never_rendered_prefilled_field() {
    let mut doc =
        PdfDocumentState::open(Path::new("tests/fixtures/sample_form_prefilled.pdf")).unwrap();
    let field = field_rect(&doc);

    flatten(&mut doc).unwrap();

    assert!(fields_on_page(&doc, 0).is_empty());
    let ink = ink_in(&doc, 0, field);
    assert!(ink > 50, "prefilled value baked as only {ink} ink pixels");
}

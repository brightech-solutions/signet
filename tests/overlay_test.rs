use pdf_utility::overlay::{
    OverlayElement, OverlayLayer, SignatureElement, TextBoxElement, clamp_to_page,
};
use uuid::Uuid;

#[test]
fn clamp_keeps_fully_inside_page_when_dropped_at_edge() {
    let page_size = egui::vec2(600.0, 800.0);
    let rect = egui::Rect::from_min_size(egui::pos2(590.0, 790.0), egui::vec2(100.0, 40.0));
    let clamped = clamp_to_page(rect, page_size);
    assert!(clamped.min.x >= 0.0 && clamped.min.y >= 0.0);
    assert!(clamped.max.x <= page_size.x && clamped.max.y <= page_size.y);
}

#[test]
fn clamp_is_noop_when_already_inside() {
    let page_size = egui::vec2(600.0, 800.0);
    let rect = egui::Rect::from_min_size(egui::pos2(50.0, 50.0), egui::vec2(100.0, 40.0));
    assert_eq!(clamp_to_page(rect, page_size), rect);
}

#[test]
fn clamp_preserves_size_even_when_shifted() {
    let page_size = egui::vec2(600.0, 800.0);
    let rect = egui::Rect::from_min_size(egui::pos2(590.0, 790.0), egui::vec2(100.0, 40.0));
    let clamped = clamp_to_page(rect, page_size);
    assert!((clamped.width() - 100.0).abs() < f32::EPSILON);
    assert!((clamped.height() - 40.0).abs() < f32::EPSILON);
}

#[test]
fn clamp_anchors_to_origin_when_element_larger_than_page() {
    let page_size = egui::vec2(600.0, 800.0);
    // Wider than the page itself: min.x must clamp to 0 rather than going
    // negative or leaving max.x unbounded.
    let rect = egui::Rect::from_min_size(egui::pos2(-50.0, 10.0), egui::vec2(800.0, 40.0));
    let clamped = clamp_to_page(rect, page_size);
    assert_eq!(clamped.min.x, 0.0);
}

#[test]
fn move_element_clamps_result() {
    let mut layer = OverlayLayer { elements: vec![] };
    let id = Uuid::new_v4();
    layer.add(OverlayElement::TextBox(TextBoxElement {
        id,
        page_index: 0,
        rect: egui::Rect::from_min_size(egui::pos2(10.0, 10.0), egui::vec2(100.0, 40.0)),
        text: String::new(),
    }));
    layer.move_element(id, egui::pos2(590.0, 790.0), egui::vec2(600.0, 800.0));
    let rect = layer.elements[0].rect();
    assert!(rect.max.x <= 600.0 && rect.max.y <= 800.0);
}

#[test]
fn resize_element_clamps_result() {
    let mut layer = OverlayLayer { elements: vec![] };
    let id = Uuid::new_v4();
    layer.add(OverlayElement::TextBox(TextBoxElement {
        id,
        page_index: 0,
        rect: egui::Rect::from_min_size(egui::pos2(10.0, 10.0), egui::vec2(100.0, 40.0)),
        text: String::new(),
    }));
    // Resize to a rect that overshoots the page on both axes.
    let oversized = egui::Rect::from_min_size(egui::pos2(500.0, 700.0), egui::vec2(300.0, 300.0));
    layer.resize_element(id, oversized, egui::vec2(600.0, 800.0));
    let rect = layer.elements[0].rect();
    assert!(rect.min.x >= 0.0 && rect.min.y >= 0.0);
    assert!(rect.max.x <= 600.0 && rect.max.y <= 800.0);
}

#[test]
fn remove_deletes_element() {
    let mut layer = OverlayLayer { elements: vec![] };
    let id = Uuid::new_v4();
    layer.add(OverlayElement::TextBox(TextBoxElement {
        id,
        page_index: 0,
        rect: egui::Rect::from_min_size(egui::pos2(10.0, 10.0), egui::vec2(100.0, 40.0)),
        text: String::new(),
    }));
    assert_eq!(layer.elements.len(), 1);
    layer.remove(id);
    assert!(layer.elements.is_empty());
}

/// The Review Focus requirement, exercised for a placed signature rather
/// than a text box: `OverlayLayer::move_element` must clamp a
/// `SignatureElement` exactly like a `TextBoxElement` — both go through
/// the same `clamp_to_page` call, with no signature-specific bypass.
#[test]
fn placed_signature_is_clamped_like_a_text_box() {
    let mut layer = OverlayLayer { elements: vec![] };
    let id = Uuid::new_v4();
    let img = egui::ColorImage::filled([40, 20], egui::Color32::TRANSPARENT);
    layer.add(OverlayElement::Signature(SignatureElement {
        id,
        page_index: 0,
        rect: egui::Rect::from_min_size(egui::pos2(590.0, 790.0), egui::vec2(40.0, 20.0)),
        image: img,
    }));
    layer.move_element(id, egui::pos2(590.0, 790.0), egui::vec2(600.0, 800.0));
    let rect = layer.elements[0].rect();
    assert!(rect.max.x <= 600.0 && rect.max.y <= 800.0);
}

#[test]
fn move_element_on_unknown_id_is_a_noop() {
    let mut layer = OverlayLayer { elements: vec![] };
    layer.move_element(
        Uuid::new_v4(),
        egui::pos2(1.0, 1.0),
        egui::vec2(600.0, 800.0),
    );
    assert!(layer.elements.is_empty());
}

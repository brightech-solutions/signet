//! Free-form overlay elements — text boxes, and (from Task 8) placed
//! signature images — that live on top of a page independently of any
//! AcroForm field, plus the generic selection/move/resize/delete mechanics
//! shared by both.
//!
//! The one hard rule here (the Review Focus item for this task): an element
//! dropped or dragged near a page edge must end up fully inside the page.
//! [`clamp_to_page`] is the single place that enforces it, and both
//! [`OverlayLayer::move_element`] and [`OverlayLayer::resize_element`] call
//! it before storing a new rect — a caller cannot bypass clamping by going
//! through either path.
//!
//! There is deliberately no undo/redo here: only direct manipulation
//! (move/resize/delete before saving) is in scope for this application.

use uuid::Uuid;

/// A free-form text box placed on a page, independent of any AcroForm
/// field. `rect` is in the same page-space coordinate system as
/// [`crate::form_fields::FormFieldHit::page_rect`] (PDF points, origin
/// top-left).
#[derive(Debug, Clone)]
pub struct TextBoxElement {
    pub id: Uuid,
    pub page_index: usize,
    pub rect: egui::Rect,
    pub text: String,
}

/// A placed signature image: either an uploaded photo/scan or a rasterized
/// typed name (see `crate::signatures::render_signature_entry`), already
/// rendered to pixels by the time it becomes a `SignatureElement` — this
/// struct itself doesn't care which. `rect` is in the same page-space
/// coordinate system as [`TextBoxElement::rect`], and shares the exact
/// same selection/move/resize/delete mechanics via [`OverlayLayer`] below;
/// nothing about those mechanics is specific to either variant.
#[derive(Debug, Clone)]
pub struct SignatureElement {
    pub id: Uuid,
    pub page_index: usize,
    pub rect: egui::Rect,
    pub image: egui::ColorImage,
}

/// Any element that can live on the overlay layer.
#[derive(Debug, Clone)]
pub enum OverlayElement {
    TextBox(TextBoxElement),
    Signature(SignatureElement),
}

impl OverlayElement {
    /// The element's identity, stable for its lifetime on the layer.
    pub fn id(&self) -> Uuid {
        match self {
            OverlayElement::TextBox(element) => element.id,
            OverlayElement::Signature(element) => element.id,
        }
    }

    /// The 0-based index of the page the element is placed on.
    pub fn page_index(&self) -> usize {
        match self {
            OverlayElement::TextBox(element) => element.page_index,
            OverlayElement::Signature(element) => element.page_index,
        }
    }

    /// The element's current rect, in page space.
    pub fn rect(&self) -> egui::Rect {
        match self {
            OverlayElement::TextBox(element) => element.rect,
            OverlayElement::Signature(element) => element.rect,
        }
    }

    /// Crate-internal: used by [`OverlayLayer::move_element`] and
    /// [`OverlayLayer::resize_element`], which always clamp the rect via
    /// [`clamp_to_page`] before calling this, so there is no public setter
    /// that would let a caller store an unclamped rect directly.
    fn set_rect(&mut self, rect: egui::Rect) {
        match self {
            OverlayElement::TextBox(element) => element.rect = rect,
            OverlayElement::Signature(element) => element.rect = rect,
        }
    }
}

/// Clamps `rect` so both `rect.min` and `rect.max` fall within
/// `[0, page_size]` on each axis, preserving `rect`'s width and height —
/// it only ever shifts the origin, never resizes.
///
/// If the rect is itself larger than the page on an axis (wider or taller
/// than the page), that axis's `min` is clamped to `0` rather than left
/// negative or computed from a negative `page_extent - size`.
pub fn clamp_to_page(rect: egui::Rect, page_size: egui::Vec2) -> egui::Rect {
    let size = rect.size();

    let clamp_origin = |min: f32, size: f32, page_extent: f32| -> f32 {
        if size >= page_extent {
            0.0
        } else {
            min.clamp(0.0, page_extent - size)
        }
    };

    let min = egui::pos2(
        clamp_origin(rect.min.x, size.x, page_size.x),
        clamp_origin(rect.min.y, size.y, page_size.y),
    );

    egui::Rect::from_min_size(min, size)
}

/// The set of overlay elements for the currently-open document. One layer
/// covers every page; each element records its own `page_index`.
#[derive(Debug, Default)]
pub struct OverlayLayer {
    pub elements: Vec<OverlayElement>,
}

impl OverlayLayer {
    /// Adds a new element to the layer.
    pub fn add(&mut self, element: OverlayElement) {
        self.elements.push(element);
    }

    /// Removes the element with the given `id`, if present. A no-op if no
    /// element with that id exists (e.g. it was already removed).
    pub fn remove(&mut self, id: Uuid) {
        self.elements.retain(|element| element.id() != id);
    }

    /// Returns the element with the given `id`, if present.
    pub fn get(&self, id: Uuid) -> Option<&OverlayElement> {
        self.elements.iter().find(|element| element.id() == id)
    }

    /// Returns a mutable reference to the element with the given `id`, if
    /// present. Used by callers that need to mutate an element's contents
    /// directly (e.g. a text box's `text` while it's being edited) rather
    /// than its rect, which must only ever change via [`Self::move_element`]
    /// or [`Self::resize_element`] so it stays clamped.
    pub fn get_mut(&mut self, id: Uuid) -> Option<&mut OverlayElement> {
        self.elements.iter_mut().find(|element| element.id() == id)
    }

    /// Moves the element `id` so its rect's top-left corner is at
    /// `new_origin` (page space), preserving its current size. The result
    /// is always passed through [`clamp_to_page`] before being stored, so
    /// the element can never end up partly or fully off the page. A no-op
    /// if no element with `id` exists.
    pub fn move_element(&mut self, id: Uuid, new_origin: egui::Pos2, page_size: egui::Vec2) {
        let Some(element) = self.get_mut(id) else {
            return;
        };
        let size = element.rect().size();
        let desired = egui::Rect::from_min_size(new_origin, size);
        element.set_rect(clamp_to_page(desired, page_size));
    }

    /// Resizes the element `id` to `new_rect` (page space). The result is
    /// always passed through [`clamp_to_page`] before being stored, so the
    /// element can never end up partly or fully off the page. A no-op if no
    /// element with `id` exists.
    pub fn resize_element(&mut self, id: Uuid, new_rect: egui::Rect, page_size: egui::Vec2) {
        let Some(element) = self.get_mut(id) else {
            return;
        };
        element.set_rect(clamp_to_page(new_rect, page_size));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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
}

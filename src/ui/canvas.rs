//! The scrollable page canvas: renders the current page's texture and maps
//! between PDF page coordinates and on-screen coordinates.
//!
//! The coordinate transform here is what later tasks (form field overlays,
//! signature placement) use to position widgets over the rendered page.

/// Maps a point in page space (PDF points, origin top-left, matching the
/// rendered page image) to on-screen coordinates, given the current zoom
/// level and the canvas's scroll offset.
///
/// `screen = page_point * zoom - scroll_offset`.
///
/// First called from `PdfUtilityApp` as of Task 4, to position the inline
/// text-field editor over a clicked AcroForm field; `scroll_offset` is
/// `-image_response.rect.min.to_vec2()`, i.e. the negative of where the
/// rendered page image's page-space origin currently sits on screen (which
/// already accounts for the `ScrollArea`'s current scroll position).
pub fn page_to_screen(page_point: egui::Pos2, zoom: f32, scroll_offset: egui::Vec2) -> egui::Pos2 {
    egui::pos2(page_point.x * zoom, page_point.y * zoom) - scroll_offset
}

/// Maps a rect in page space to its on-screen rect, applying
/// [`page_to_screen`] to both corners.
pub fn page_rect_to_screen(
    page_rect: egui::Rect,
    zoom: f32,
    scroll_offset: egui::Vec2,
) -> egui::Rect {
    egui::Rect::from_two_pos(
        page_to_screen(page_rect.min, zoom, scroll_offset),
        page_to_screen(page_rect.max, zoom, scroll_offset),
    )
}

/// Maps a screen-space delta (e.g. an `egui::Response::drag_delta()`) to
/// the equivalent delta in page space, inverting the scale
/// [`page_to_screen`] applies. Used by Task 5's overlay dragging/resizing
/// (and, from Task 8, signature dragging/resizing) to turn a mouse-drag
/// delta into a page-space move for `OverlayLayer::move_element` /
/// `resize_element`. Scroll offset doesn't need inverting here since a
/// *delta* between two frames is unaffected by a constant offset.
pub fn screen_delta_to_page(screen_delta: egui::Vec2, zoom: f32) -> egui::Vec2 {
    screen_delta / zoom
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn page_point_to_screen_accounts_for_zoom_and_scroll() {
        let screen = page_to_screen(
            egui::pos2(100.0, 50.0),
            /*zoom*/ 2.0,
            /*scroll_offset*/ egui::vec2(10.0, 0.0),
        );
        assert_eq!(screen, egui::pos2(190.0, 100.0));
    }

    #[test]
    fn page_rect_to_screen_maps_both_corners() {
        let screen = page_rect_to_screen(
            egui::Rect::from_min_max(egui::pos2(10.0, 20.0), egui::pos2(30.0, 40.0)),
            /*zoom*/ 2.0,
            /*scroll_offset*/ egui::vec2(5.0, 0.0),
        );
        assert_eq!(
            screen,
            egui::Rect::from_min_max(egui::pos2(15.0, 40.0), egui::pos2(55.0, 80.0))
        );
    }

    #[test]
    fn screen_delta_to_page_inverts_zoom_scale() {
        let delta = screen_delta_to_page(egui::vec2(20.0, 10.0), /*zoom*/ 2.0);
        assert_eq!(delta, egui::vec2(10.0, 5.0));
    }
}

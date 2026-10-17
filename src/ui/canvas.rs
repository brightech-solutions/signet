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
/// Not yet called from `PdfUtilityApp` (Task 3 renders the whole page as one
/// texture and doesn't position anything over it); later tasks' form-field
/// and signature overlays are the first consumers.
#[allow(dead_code)]
pub fn page_to_screen(page_point: egui::Pos2, zoom: f32, scroll_offset: egui::Vec2) -> egui::Pos2 {
    egui::pos2(page_point.x * zoom, page_point.y * zoom) - scroll_offset
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
}

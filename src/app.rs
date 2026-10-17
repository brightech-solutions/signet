//! The root `eframe::App` for PDF Utility.
//!
//! Owns the currently open document (if any), the viewer's page/zoom state,
//! and the rendered-page texture. File → Open and the zoom/page controls all
//! live here; the page/screen coordinate transform used to position overlays
//! lives in [`crate::ui::canvas`].

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use pdf_utility::document::{DocumentError, PdfDocumentState};
use pdf_utility::form_fields::{self, FieldKind};
use pdf_utility::overlay::{self, OverlayElement, OverlayLayer, TextBoxElement};
use uuid::Uuid;

use crate::ui::canvas;

/// Smallest and largest zoom factors selectable via the zoom buttons.
const MIN_ZOOM: f32 = 0.25;
const MAX_ZOOM: f32 = 4.0;
const ZOOM_STEP: f32 = 0.25;

/// Default size (page points) for a newly-placed free-form text box.
const DEFAULT_TEXT_BOX_SIZE: egui::Vec2 = egui::vec2(160.0, 40.0);
/// Smallest size (page points) a text box can be resized down to — a floor
/// so dragging a corner past its opposite edge can't collapse or invert
/// the box.
const MIN_TEXT_BOX_SIZE: egui::Vec2 = egui::vec2(24.0, 16.0);
/// On-screen size (pixels) of a selected element's corner resize handles.
const HANDLE_SIZE: f32 = 8.0;

/// The toolbar's current placement tool. `Select` is the default: clicks
/// hit-test existing overlay elements and AcroForm fields but don't create
/// anything. `AddTextBox` is toggled on via the "Add text box" button.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
enum ToolMode {
    #[default]
    Select,
    AddTextBox,
}

/// Which corner of a rect a resize handle is anchored to, i.e. which edges
/// move when that handle is dragged.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
enum Corner {
    TopLeft,
    TopRight,
    BottomLeft,
    BottomRight,
}

impl Corner {
    const ALL: [Corner; 4] = [
        Corner::TopLeft,
        Corner::TopRight,
        Corner::BottomLeft,
        Corner::BottomRight,
    ];

    fn moves_left_edge(self) -> bool {
        matches!(self, Corner::TopLeft | Corner::BottomLeft)
    }

    fn moves_top_edge(self) -> bool {
        matches!(self, Corner::TopLeft | Corner::TopRight)
    }

    fn point_on(self, rect: egui::Rect) -> egui::Pos2 {
        match self {
            Corner::TopLeft => rect.left_top(),
            Corner::TopRight => rect.right_top(),
            Corner::BottomLeft => rect.left_bottom(),
            Corner::BottomRight => rect.right_bottom(),
        }
    }
}

/// Applies a page-space drag `delta` on handle `corner` to `rect`, moving
/// only the edges that corner owns, then enforces [`MIN_TEXT_BOX_SIZE`] on
/// each axis so the opposite edge can't be dragged past (which would
/// collapse or invert the rect). The result still needs to go through
/// [`overlay::clamp_to_page`] (via `OverlayLayer::resize_element`) to stay
/// on the page; this function only keeps the rect itself sane.
fn resize_rect_for_corner(rect: egui::Rect, corner: Corner, delta: egui::Vec2) -> egui::Rect {
    let mut min = rect.min;
    let mut max = rect.max;

    if corner.moves_left_edge() {
        min.x += delta.x;
    } else {
        max.x += delta.x;
    }
    if corner.moves_top_edge() {
        min.y += delta.y;
    } else {
        max.y += delta.y;
    }

    if max.x - min.x < MIN_TEXT_BOX_SIZE.x {
        if corner.moves_left_edge() {
            min.x = max.x - MIN_TEXT_BOX_SIZE.x;
        } else {
            max.x = min.x + MIN_TEXT_BOX_SIZE.x;
        }
    }
    if max.y - min.y < MIN_TEXT_BOX_SIZE.y {
        if corner.moves_top_edge() {
            min.y = max.y - MIN_TEXT_BOX_SIZE.y;
        } else {
            max.y = min.y + MIN_TEXT_BOX_SIZE.y;
        }
    }

    egui::Rect::from_min_max(min, max)
}

/// The viewer's page/zoom state.
///
/// Read by later tasks' overlay/canvas code (together with
/// [`crate::ui::canvas::page_to_screen`]) to map page coordinates to screen
/// coordinates.
pub struct ViewerState {
    pub current_page: usize,
    pub zoom: f32,
}

impl Default for ViewerState {
    fn default() -> Self {
        Self {
            current_page: 0,
            zoom: 1.0,
        }
    }
}

/// Root application state.
pub struct PdfUtilityApp {
    document: Option<PdfDocumentState>,
    viewer: ViewerState,
    /// Set when the most recent open attempt failed; cleared when the user
    /// dismisses the error modal or a later open succeeds.
    error_message: Option<String>,
    /// The texture for the currently-rendered page, plus the (page, zoom)
    /// it was rendered for, so we only re-rasterize when something changed.
    texture: Option<egui::TextureHandle>,
    texture_key: Option<(usize, u32)>,
    /// The name of the AcroForm text field currently shown as an inline
    /// editor, if the user has clicked one. `None` means no editor is shown.
    focused_field: Option<String>,
    /// This session's in-progress values for text fields, keyed by field
    /// name. The first time a field is focused (in `handle_canvas_click`),
    /// its entry is seeded from the document's real current value via
    /// `form_fields::get_field_value` (falling back to empty only if
    /// pdfium reports none) — never unconditionally defaulted to empty —
    /// so a field that already has content (a pre-filled template, or a
    /// document re-opened after partial filling by another tool) doesn't
    /// get silently blanked out by the inline editor. After that, this map
    /// just tracks the editor's live buffer so switching focus away from a
    /// field and back doesn't lose what was typed into it.
    field_values: HashMap<String, String>,
    /// The current document's free-form overlay elements (text boxes, and
    /// — from Task 8 — placed signatures), covering every page.
    overlay: OverlayLayer,
    /// The toolbar's current placement tool; see [`ToolMode`].
    tool_mode: ToolMode,
    /// The overlay element currently selected (shown with a selection
    /// outline and, while not being edited, corner resize handles), if any.
    selected_element: Option<Uuid>,
    /// The text box currently showing its inline `egui::TextEdit`, if any.
    /// A subset of `selected_element`: editing implies selected, but a
    /// selected element need not be the one being edited.
    editing_text_box: Option<Uuid>,
    /// Set for exactly one frame after a text box starts being edited (on
    /// creation, or when the user re-opens an existing one), so
    /// `draw_overlay` can call `response.request_focus()` once rather than
    /// re-requesting focus on every frame it's drawn.
    request_focus_for: Option<Uuid>,
}

impl PdfUtilityApp {
    /// Creates the app, optionally opening `initial_path` immediately (used
    /// for launch-arg / Finder double-click open).
    pub fn new(initial_path: Option<PathBuf>) -> Self {
        let mut app = Self {
            document: None,
            viewer: ViewerState::default(),
            error_message: None,
            texture: None,
            texture_key: None,
            focused_field: None,
            field_values: HashMap::new(),
            overlay: OverlayLayer::default(),
            tool_mode: ToolMode::default(),
            selected_element: None,
            editing_text_box: None,
            request_focus_for: None,
        };

        if let Some(path) = initial_path {
            app.open_path(&path);
        }

        app
    }

    /// Opens the PDF at `path`, replacing any currently open document.
    ///
    /// On success, resets the viewer to page 0 at 100% zoom and clears any
    /// previous error. On failure, leaves no document open and records a
    /// user-facing error message.
    fn open_path(&mut self, path: &Path) {
        match PdfDocumentState::open(path) {
            Ok(document) => {
                self.document = Some(document);
                self.viewer = ViewerState::default();
                self.error_message = None;
                self.texture = None;
                self.texture_key = None;
                self.focused_field = None;
                self.field_values.clear();
                self.overlay = OverlayLayer::default();
                self.tool_mode = ToolMode::default();
                self.selected_element = None;
                self.editing_text_box = None;
                self.request_focus_for = None;
            }
            Err(err) => {
                self.document = None;
                self.texture = None;
                self.texture_key = None;
                self.focused_field = None;
                self.field_values.clear();
                self.overlay = OverlayLayer::default();
                self.tool_mode = ToolMode::default();
                self.selected_element = None;
                self.editing_text_box = None;
                self.request_focus_for = None;
                self.error_message = Some(error_message_for(&err));
            }
        }
    }

    /// Re-rasterizes the current page into `self.texture` if the page or
    /// zoom changed since the last render (or nothing has been rendered
    /// yet). A render failure is reported the same way an open failure is.
    fn ensure_texture(&mut self, ctx: &egui::Context) {
        let Some(document) = &self.document else {
            self.texture = None;
            self.texture_key = None;
            return;
        };

        let key = (self.viewer.current_page, self.viewer.zoom.to_bits());
        if self.texture_key == Some(key) {
            return;
        }

        match document.render_page(self.viewer.current_page, self.viewer.zoom) {
            Ok(image) => {
                self.texture =
                    Some(ctx.load_texture("pdf-page", image, egui::TextureOptions::default()));
                self.texture_key = Some(key);
            }
            Err(err) => {
                self.texture = None;
                self.texture_key = None;
                self.error_message = Some(error_message_for(&err));
            }
        }
    }

    /// Draws the File menu, handling "Open...".
    fn menu_bar(&mut self, ui: &mut egui::Ui) {
        egui::MenuBar::new().ui(ui, |ui| {
            ui.menu_button("File", |ui| {
                if ui.button("Open…").clicked() {
                    ui.close();
                    if let Some(path) = rfd::FileDialog::new()
                        .add_filter("PDF", &["pdf"])
                        .pick_file()
                    {
                        self.open_path(&path);
                    }
                }
            });
        });
    }

    /// Draws the page navigation and zoom controls. Disabled (not shown)
    /// when no document is open.
    fn toolbar(&mut self, ui: &mut egui::Ui) {
        let Some(document) = &self.document else {
            return;
        };
        let page_count = document.page_count();

        ui.horizontal(|ui| {
            if ui
                .add_enabled(self.viewer.current_page > 0, egui::Button::new("◀ Prev"))
                .clicked()
            {
                self.viewer.current_page -= 1;
            }

            ui.label(format!(
                "Page {} of {}",
                self.viewer.current_page + 1,
                page_count
            ));

            if ui
                .add_enabled(
                    self.viewer.current_page + 1 < page_count,
                    egui::Button::new("Next ▶"),
                )
                .clicked()
            {
                self.viewer.current_page += 1;
            }

            ui.separator();

            if ui
                .add_enabled(self.viewer.zoom > MIN_ZOOM, egui::Button::new("Zoom −"))
                .clicked()
            {
                self.viewer.zoom = (self.viewer.zoom - ZOOM_STEP).max(MIN_ZOOM);
            }

            ui.label(format!("{:.0}%", self.viewer.zoom * 100.0));

            if ui
                .add_enabled(self.viewer.zoom < MAX_ZOOM, egui::Button::new("Zoom +"))
                .clicked()
            {
                self.viewer.zoom = (self.viewer.zoom + ZOOM_STEP).min(MAX_ZOOM);
            }

            // `current_page` is always kept `< page_count` by the Prev/Next
            // handlers above, but `page_size_points` returns a silent
            // `Vec2::ZERO` for an out-of-bounds index rather than a
            // `Result`, so this bound is checked explicitly rather than
            // trusted.
            if self.viewer.current_page < page_count {
                let size = document.page_size_points(self.viewer.current_page);
                ui.separator();
                ui.label(format!("{:.0} × {:.0} pt", size.x, size.y));
            }

            ui.separator();

            let add_text_box_active = self.tool_mode == ToolMode::AddTextBox;
            if ui
                .selectable_label(add_text_box_active, "Add text box")
                .clicked()
            {
                self.tool_mode = if add_text_box_active {
                    ToolMode::Select
                } else {
                    ToolMode::AddTextBox
                };
            }
        });
    }

    /// Handles a click on the rendered page image at `click_pos` (absolute
    /// screen coordinates), given the image's on-screen rect. Converts the
    /// click to a page-space point and routes it, in priority order:
    ///
    /// 1. A hit on an existing overlay element (free-form text box, or —
    ///    from Task 8 — a placed signature) on the current page selects it.
    ///    This takes priority over AcroForm fields and empty-space
    ///    placement so an element drawn on top of a field, or stacked on
    ///    top of where a new box would otherwise be created, is what gets
    ///    selected.
    /// 2. A hit on an AcroForm text field focuses it for inline editing.
    /// 3. A hit on any other field kind (checkbox, radio, dropdown, ...) is
    ///    a deliberate no-op: full support for those types isn't in scope
    ///    yet, but clicking one must never crash the app.
    /// 4. No hit at all: with the "Add text box" tool active
    ///    (`self.tool_mode`), drops a new free-form text box at the click
    ///    and switches straight to editing it; otherwise just deselects
    ///    whatever was selected.
    fn handle_canvas_click(&mut self, click_pos: egui::Pos2, image_rect: egui::Rect) {
        let Some(document) = &self.document else {
            return;
        };
        if self.viewer.current_page >= document.page_count() {
            return;
        }

        let zoom = self.viewer.zoom;
        let page_point = egui::pos2(
            (click_pos.x - image_rect.min.x) / zoom,
            (click_pos.y - image_rect.min.y) / zoom,
        );

        if let Some(id) = self.hit_test_overlay(page_point) {
            self.selected_element = Some(id);
            self.focused_field = None;
            // A plain click always lands in "selected" (outline + resize
            // handles), never "editing" — per the doc comment, editing
            // starts only on creation or via a double-click in
            // `draw_overlay`. Clearing this unconditionally also means
            // clicking a *different* element always exits whatever else
            // was being edited.
            self.editing_text_box = None;
            return;
        }

        let fields = form_fields::fields_on_page(document, self.viewer.current_page);
        match form_fields::hit_test(&fields, page_point) {
            Some(field) if field.kind == FieldKind::Text => {
                let name = field.name.clone();
                self.focused_field = Some(name.clone());
                // Keep `selected_element`/`editing_text_box` mutually
                // exclusive with `focused_field`: without this, a
                // previously-selected overlay element would stay
                // "selected" while this field's editor has keyboard
                // focus, and `draw_overlay`'s Delete-key handler (which
                // only checks `editing_text_box`, not `focused_field`)
                // could then delete that element while the user is really
                // just trying to delete a character in the field.
                self.selected_element = None;
                self.editing_text_box = None;
                // Seed the editor's buffer from the field's real current
                // value the first time it's focused in this session, rather
                // than unconditionally defaulting to empty — a text field
                // opened from a pre-filled template, or a document
                // re-opened after partial filling by another tool, may
                // already carry real content, and unconditionally starting
                // from an empty buffer would silently discard it on the
                // first keystroke. Once a field has an entry in
                // `field_values` (even an empty one, e.g. because the user
                // cleared it), that entry is left alone so in-session edits
                // aren't clobbered by re-focusing.
                self.field_values.entry(name).or_insert_with(|| {
                    form_fields::get_field_value(document, &field.name).unwrap_or_default()
                });
            }
            Some(_) => {
                // FieldKind::Other: deliberate no-op, per the Review Focus
                // requirement (see doc comment).
            }
            None => {
                self.focused_field = None;
                if self.tool_mode == ToolMode::AddTextBox {
                    let page_size = document.page_size_points(self.viewer.current_page);
                    let rect = overlay::clamp_to_page(
                        egui::Rect::from_min_size(page_point, DEFAULT_TEXT_BOX_SIZE),
                        page_size,
                    );
                    let id = Uuid::new_v4();
                    self.overlay.add(OverlayElement::TextBox(TextBoxElement {
                        id,
                        page_index: self.viewer.current_page,
                        rect,
                        text: String::new(),
                    }));
                    self.selected_element = Some(id);
                    self.editing_text_box = Some(id);
                    self.request_focus_for = Some(id);
                } else {
                    self.selected_element = None;
                    self.editing_text_box = None;
                }
            }
        }
    }

    /// Returns the id of the topmost overlay element on the current page
    /// whose rect contains `page_point`, if any. "Topmost" = last added,
    /// matching the natural draw order in `draw_overlay` (later elements
    /// are drawn on top of earlier ones).
    fn hit_test_overlay(&self, page_point: egui::Pos2) -> Option<Uuid> {
        let current_page = self.viewer.current_page;
        self.overlay
            .elements
            .iter()
            .rev()
            .find(|element| {
                element.page_index() == current_page && element.rect().contains(page_point)
            })
            .map(|element| element.id())
    }

    /// Draws the inline text editor for the currently-focused field (if any
    /// and if it's still present on the current page), positioned over its
    /// rect in the rendered page image. Edits are written straight through
    /// to the document via `form_fields::set_field_value`.
    fn draw_field_editor(&mut self, ui: &mut egui::Ui, image_rect: egui::Rect) {
        let Some(name) = self.focused_field.clone() else {
            return;
        };

        let field_rect = self.document.as_ref().and_then(|document| {
            let fields = form_fields::fields_on_page(document, self.viewer.current_page);
            fields
                .into_iter()
                .find(|field| field.name == name)
                .map(|field| field.page_rect)
        });

        let Some(field_rect) = field_rect else {
            return;
        };

        let zoom = self.viewer.zoom;
        let scroll_offset = -image_rect.min.to_vec2();
        let screen_rect = canvas::page_rect_to_screen(field_rect, zoom, scroll_offset);

        // `handle_canvas_click` always seeds `field_values[name]` (from the
        // field's real current value, falling back to empty only if pdfium
        // reports none) before setting `focused_field`, so this entry
        // already exists by the time we get here; `.or_default()` is just a
        // defensive fallback, not the normal seeding path.
        let buffer = self.field_values.entry(name.clone()).or_default();
        let response = ui.put(screen_rect, egui::TextEdit::singleline(buffer));
        let changed_value = response.changed().then(|| buffer.clone());

        if let Some(value) = changed_value
            && let Some(document) = &mut self.document
            && let Err(err) = form_fields::set_field_value(document, &name, &value)
        {
            self.error_message = Some(error_message_for(&err));
        }
    }

    /// Draws every overlay element on the current page, and handles their
    /// selection, drag-move, corner-handle resize, and delete.
    ///
    /// For each element: the selected one being actively edited (a text
    /// box, via `editing_text_box`) is shown as a live `egui::TextEdit`;
    /// every other element is shown as a static framed box (its text, for a
    /// text box) that senses clicks and drags — a click selects it, a
    /// double-click on a text box also re-opens it for editing, and a drag
    /// moves it via `OverlayLayer::move_element`. The selected element also
    /// gets four corner resize handles (drag one to resize via
    /// `OverlayLayer::resize_element`) and a "×" delete button. Pressing
    /// Delete while an element is selected (and not being edited) removes
    /// it via `OverlayLayer::remove`.
    ///
    /// Both `move_element` and `resize_element` clamp the result to the
    /// page via `overlay::clamp_to_page`, so nothing dragged here can end
    /// up partly or fully off the page — see the Review Focus requirement
    /// in `overlay.rs`.
    fn draw_overlay(&mut self, ui: &mut egui::Ui, image_rect: egui::Rect) {
        let Some(document) = &self.document else {
            return;
        };
        let current_page = self.viewer.current_page;
        if current_page >= document.page_count() {
            return;
        }

        let zoom = self.viewer.zoom;
        let page_size = document.page_size_points(current_page);
        let scroll_offset = -image_rect.min.to_vec2();

        // Snapshot the ids on this page before the loop: the loop below
        // mutates `self.overlay` (move/resize), so it can't hold a live
        // borrow of `self.overlay.elements` across iterations.
        let ids_on_page: Vec<Uuid> = self
            .overlay
            .elements
            .iter()
            .filter(|element| element.page_index() == current_page)
            .map(|element| element.id())
            .collect();

        let mut delete_requested: Option<Uuid> = None;

        for id in ids_on_page {
            let Some(element) = self.overlay.get(id) else {
                continue;
            };
            let page_rect = element.rect();
            let screen_rect = canvas::page_rect_to_screen(page_rect, zoom, scroll_offset);
            let is_selected = self.selected_element == Some(id);
            let is_editing = self.editing_text_box == Some(id);
            let is_text_box = matches!(element, OverlayElement::TextBox(_));

            if is_editing {
                if let Some(OverlayElement::TextBox(text_box)) = self.overlay.get_mut(id) {
                    let response = ui.put(
                        screen_rect,
                        egui::TextEdit::multiline(&mut text_box.text).id_salt(id),
                    );
                    if self.request_focus_for == Some(id) {
                        response.request_focus();
                        self.request_focus_for = None;
                    }
                    if response.lost_focus() {
                        self.editing_text_box = None;
                    }
                }
            } else {
                let body_id = egui::Id::unique(("overlay_body", id));
                let body_response =
                    ui.interact(screen_rect, body_id, egui::Sense::click_and_drag());

                let stroke_color = if is_selected {
                    egui::Color32::from_rgb(0, 120, 215)
                } else {
                    egui::Color32::GRAY
                };
                ui.painter().rect_stroke(
                    screen_rect,
                    2.0,
                    egui::Stroke::new(1.5, stroke_color),
                    egui::StrokeKind::Inside,
                );
                if let OverlayElement::TextBox(text_box) = element {
                    ui.painter().text(
                        screen_rect.left_top() + egui::vec2(4.0, 2.0),
                        egui::Align2::LEFT_TOP,
                        &text_box.text,
                        egui::FontId::default(),
                        ui.visuals().text_color(),
                    );
                }

                if body_response.double_clicked() {
                    self.selected_element = Some(id);
                    if is_text_box {
                        self.editing_text_box = Some(id);
                        self.request_focus_for = Some(id);
                    }
                } else if body_response.clicked() {
                    self.selected_element = Some(id);
                }

                if body_response.dragged() {
                    self.selected_element = Some(id);
                    let delta_page = canvas::screen_delta_to_page(body_response.drag_delta(), zoom);
                    self.overlay
                        .move_element(id, page_rect.min + delta_page, page_size);
                }
            }

            if is_selected && !is_editing {
                for corner in Corner::ALL {
                    // Re-read the element's current rect each handle: an
                    // earlier handle this same frame (or the body drag
                    // above) may already have moved/resized it.
                    let Some(current_rect) = self.overlay.get(id).map(|element| element.rect())
                    else {
                        break;
                    };
                    let corner_screen =
                        canvas::page_to_screen(corner.point_on(current_rect), zoom, scroll_offset);
                    let handle_rect =
                        egui::Rect::from_center_size(corner_screen, egui::Vec2::splat(HANDLE_SIZE));
                    let handle_id = egui::Id::unique(("overlay_handle", id, corner));
                    let handle_response = ui.interact(handle_rect, handle_id, egui::Sense::drag());
                    ui.painter().rect_filled(
                        handle_rect,
                        1.0,
                        egui::Color32::from_rgb(0, 120, 215),
                    );

                    if handle_response.dragged() {
                        let delta_page =
                            canvas::screen_delta_to_page(handle_response.drag_delta(), zoom);
                        let new_rect = resize_rect_for_corner(current_rect, corner, delta_page);
                        self.overlay.resize_element(id, new_rect, page_size);
                    }
                }

                let delete_center = screen_rect.right_top() + egui::vec2(-HANDLE_SIZE, HANDLE_SIZE);
                let delete_rect = egui::Rect::from_center_size(
                    delete_center,
                    egui::Vec2::splat(HANDLE_SIZE * 2.0),
                );
                let delete_id = egui::Id::unique(("overlay_delete", id));
                let delete_response = ui.interact(delete_rect, delete_id, egui::Sense::click());
                ui.painter()
                    .rect_filled(delete_rect, 1.0, egui::Color32::from_rgb(200, 40, 40));
                ui.painter().text(
                    delete_rect.center(),
                    egui::Align2::CENTER_CENTER,
                    "×",
                    egui::FontId::default(),
                    egui::Color32::WHITE,
                );
                if delete_response.clicked() {
                    delete_requested = Some(id);
                }
            }
        }

        if let Some(id) = delete_requested {
            self.overlay.remove(id);
            if self.selected_element == Some(id) {
                self.selected_element = None;
            }
            if self.editing_text_box == Some(id) {
                self.editing_text_box = None;
            }
        }

        // Delete key removes the selected element, but only while it isn't
        // the one currently capturing keyboard input for text editing
        // (`handle_canvas_click`/this method keep `focused_field` and
        // `selected_element` mutually exclusive, so this can't fire while
        // an AcroForm field's editor has focus either).
        if let Some(id) = self.selected_element
            && self.editing_text_box != Some(id)
            && ui.ctx().input(|input| input.key_pressed(egui::Key::Delete))
        {
            self.overlay.remove(id);
            self.selected_element = None;
        }
    }

    /// Draws the error modal, if an error is currently set. Dismissible via
    /// its OK button, clicking outside it, or Escape.
    fn error_modal(&mut self, ctx: &egui::Context) {
        let Some(message) = self.error_message.clone() else {
            return;
        };

        let response = egui::Modal::new(egui::Id::unique("document_error_modal")).show(ctx, |ui| {
            ui.set_max_width(320.0);
            ui.label(message);
            ui.add_space(8.0);
            ui.vertical_centered(|ui| {
                if ui.button("OK").clicked() {
                    ui.close();
                }
            });
        });

        if response.should_close() {
            self.error_message = None;
        }
    }
}

/// Maps a [`DocumentError`] to the message shown to the user. Corrupt and
/// password-protected documents get clear, distinct, non-technical
/// messages; the remaining variants fall back to their `Display` text.
fn error_message_for(err: &DocumentError) -> String {
    match err {
        DocumentError::Corrupt => "This file isn't a valid PDF.".to_string(),
        DocumentError::PasswordProtected => {
            "This PDF is password-protected, which isn't supported yet.".to_string()
        }
        DocumentError::Io(_) | DocumentError::Pdfium(_) => {
            format!("Couldn't open this file: {err}")
        }
    }
}

impl eframe::App for PdfUtilityApp {
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        let ctx = ui.ctx().clone();

        egui::Panel::top("menu_bar_panel").show(ui, |ui| {
            self.menu_bar(ui);
        });

        egui::CentralPanel::default().show(ui, |ui| {
            self.toolbar(ui);
            ui.separator();

            self.ensure_texture(&ctx);

            egui::ScrollArea::both()
                .auto_shrink([false, false])
                .show(ui, |ui| {
                    if let Some(texture) = &self.texture {
                        let image_response =
                            ui.add(egui::Image::new(texture).sense(egui::Sense::click()));

                        if image_response.clicked()
                            && let Some(click_pos) = image_response.interact_pointer_pos()
                        {
                            self.handle_canvas_click(click_pos, image_response.rect);
                        }

                        self.draw_field_editor(ui, image_response.rect);
                        self.draw_overlay(ui, image_response.rect);
                    } else if self.document.is_none() {
                        ui.label("Open a PDF via File → Open… to get started.");
                    }
                });
        });

        self.error_modal(&ctx);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn app_with_plain_pdf() -> PdfUtilityApp {
        PdfUtilityApp::new(Some(PathBuf::from("tests/fixtures/sample_plain.pdf")))
    }

    fn app_with_form_pdf() -> PdfUtilityApp {
        PdfUtilityApp::new(Some(PathBuf::from("tests/fixtures/sample_form.pdf")))
    }

    /// `handle_canvas_click` doesn't take an `egui::Ui`, so its routing
    /// logic (overlay hit > field hit > tool-mode-gated creation) is
    /// directly unit-testable against a real document, without needing a
    /// live egui frame.
    #[test]
    fn add_text_box_tool_creates_and_selects_box_on_empty_click() {
        let mut app = app_with_plain_pdf();
        app.tool_mode = ToolMode::AddTextBox;
        let image_rect =
            egui::Rect::from_min_size(egui::pos2(0.0, 0.0), egui::vec2(1000.0, 1000.0));

        app.handle_canvas_click(egui::pos2(50.0, 60.0), image_rect);

        assert_eq!(app.overlay.elements.len(), 1);
        let element = &app.overlay.elements[0];
        assert_eq!(element.page_index(), 0);
        assert_eq!(app.selected_element, Some(element.id()));
        assert_eq!(app.editing_text_box, Some(element.id()));
        assert_eq!(app.request_focus_for, Some(element.id()));
    }

    #[test]
    fn select_tool_does_not_create_a_box_on_empty_click() {
        let mut app = app_with_plain_pdf();
        let image_rect =
            egui::Rect::from_min_size(egui::pos2(0.0, 0.0), egui::vec2(1000.0, 1000.0));

        app.handle_canvas_click(egui::pos2(50.0, 60.0), image_rect);

        assert!(app.overlay.elements.is_empty());
    }

    /// Clicking inside an already-placed box selects it rather than
    /// stacking a second box underneath it, even with the "Add text box"
    /// tool still active.
    #[test]
    fn clicking_an_existing_box_selects_it_without_duplicating() {
        let mut app = app_with_plain_pdf();
        app.tool_mode = ToolMode::AddTextBox;
        let image_rect =
            egui::Rect::from_min_size(egui::pos2(0.0, 0.0), egui::vec2(1000.0, 1000.0));

        app.handle_canvas_click(egui::pos2(50.0, 60.0), image_rect);
        let id = app.overlay.elements[0].id();
        let rect = app.overlay.elements[0].rect();

        app.handle_canvas_click(rect.center(), image_rect);

        assert_eq!(app.overlay.elements.len(), 1);
        assert_eq!(app.selected_element, Some(id));
    }

    /// The Review Focus requirement, exercised through the real click path
    /// rather than just `clamp_to_page` directly: dropping a box with a
    /// click right at the page's bottom-right corner must not leave any
    /// part of it off the page.
    #[test]
    fn text_box_dropped_at_page_edge_is_clamped_to_page() {
        let mut app = app_with_plain_pdf();
        app.tool_mode = ToolMode::AddTextBox;
        let page_size = app.document.as_ref().unwrap().page_size_points(0);
        let image_rect =
            egui::Rect::from_min_size(egui::pos2(0.0, 0.0), egui::vec2(2000.0, 2000.0));

        app.handle_canvas_click(egui::pos2(page_size.x - 1.0, page_size.y - 1.0), image_rect);

        let rect = app.overlay.elements[0].rect();
        assert!(rect.min.x >= 0.0 && rect.min.y >= 0.0);
        assert!(rect.max.x <= page_size.x && rect.max.y <= page_size.y);
    }

    /// Regression test for a real invariant gap found in self-review:
    /// `selected_element`/`editing_text_box` must stay mutually exclusive
    /// with `focused_field`, or `draw_overlay`'s Delete-key handler could
    /// delete a stale "selected" overlay element while the user is really
    /// just typing into (and might press Delete/Backspace inside) an
    /// AcroForm field's editor.
    #[test]
    fn focusing_a_field_clears_overlay_selection_and_editing() {
        let mut app = app_with_form_pdf();
        let image_rect =
            egui::Rect::from_min_size(egui::pos2(0.0, 0.0), egui::vec2(1000.0, 1000.0));

        // Place and start editing a text box in the page's top-left
        // corner, away from the fixture's AcroForm field.
        app.tool_mode = ToolMode::AddTextBox;
        app.handle_canvas_click(egui::pos2(5.0, 5.0), image_rect);
        let box_id = app.overlay.elements[0].id();
        assert_eq!(app.selected_element, Some(box_id));
        assert_eq!(app.editing_text_box, Some(box_id));

        let field_center = {
            let document = app.document.as_ref().unwrap();
            let fields = form_fields::fields_on_page(document, 0);
            fields
                .iter()
                .find(|field| field.name == "applicant_name")
                .unwrap()
                .page_rect
                .center()
        };

        app.tool_mode = ToolMode::Select;
        app.handle_canvas_click(field_center, image_rect);

        assert_eq!(app.focused_field, Some("applicant_name".to_string()));
        assert_eq!(app.selected_element, None);
        assert_eq!(app.editing_text_box, None);
    }

    #[test]
    fn resize_from_bottom_right_moves_only_the_max_corner() {
        let rect = egui::Rect::from_min_size(egui::pos2(10.0, 10.0), egui::vec2(50.0, 50.0));
        let resized = resize_rect_for_corner(rect, Corner::BottomRight, egui::vec2(5.0, -3.0));
        assert_eq!(resized.min, egui::pos2(10.0, 10.0));
        assert_eq!(resized.max, egui::pos2(65.0, 57.0));
    }

    #[test]
    fn resize_from_top_left_moves_only_the_min_corner() {
        let rect = egui::Rect::from_min_size(egui::pos2(10.0, 10.0), egui::vec2(50.0, 50.0));
        let resized = resize_rect_for_corner(rect, Corner::TopLeft, egui::vec2(4.0, 6.0));
        assert_eq!(resized.min, egui::pos2(14.0, 16.0));
        assert_eq!(resized.max, egui::pos2(60.0, 60.0));
    }

    /// Dragging a corner far enough to try to collapse or invert the box
    /// must not succeed — the opposite edge holds at `MIN_TEXT_BOX_SIZE`.
    #[test]
    fn resize_cannot_collapse_below_minimum_size() {
        let rect = egui::Rect::from_min_size(egui::pos2(10.0, 10.0), egui::vec2(50.0, 50.0));
        let resized =
            resize_rect_for_corner(rect, Corner::BottomRight, egui::vec2(-1000.0, -1000.0));
        assert!(resized.width() >= MIN_TEXT_BOX_SIZE.x - f32::EPSILON);
        assert!(resized.height() >= MIN_TEXT_BOX_SIZE.y - f32::EPSILON);
        assert!(resized.width() > 0.0 && resized.height() > 0.0);
    }
}

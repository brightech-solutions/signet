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

use crate::ui::canvas;

/// Smallest and largest zoom factors selectable via the zoom buttons.
const MIN_ZOOM: f32 = 0.25;
const MAX_ZOOM: f32 = 4.0;
const ZOOM_STEP: f32 = 0.25;

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
            }
            Err(err) => {
                self.document = None;
                self.texture = None;
                self.texture_key = None;
                self.focused_field = None;
                self.field_values.clear();
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
        });
    }

    /// Handles a click on the rendered page image at `click_pos` (absolute
    /// screen coordinates), given the image's on-screen rect. Converts the
    /// click to a page-space point and hit-tests it against the current
    /// page's AcroForm fields:
    ///
    /// - A hit on a text field focuses it for inline editing.
    /// - A hit on any other field kind (checkbox, radio, dropdown, ...) is a
    ///   deliberate no-op: full support for those types isn't in scope yet,
    ///   but clicking one must never crash the app.
    /// - No hit leaves the current focus untouched (free-form text box
    ///   placement for this case is Task 5's concern).
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

        let fields = form_fields::fields_on_page(document, self.viewer.current_page);
        if let Some(field) = form_fields::hit_test(&fields, page_point)
            && field.kind == FieldKind::Text
        {
            let name = field.name.clone();
            self.focused_field = Some(name.clone());
            // Seed the editor's buffer from the field's real current value
            // the first time it's focused in this session, rather than
            // unconditionally defaulting to empty — a text field opened
            // from a pre-filled template, or a document re-opened after
            // partial filling by another tool, may already carry real
            // content, and unconditionally starting from an empty buffer
            // would silently discard it on the first keystroke. Once a
            // field has an entry in `field_values` (even an empty one,
            // e.g. because the user cleared it), that entry is left alone
            // so in-session edits aren't clobbered by re-focusing.
            self.field_values.entry(name).or_insert_with(|| {
                form_fields::get_field_value(document, &field.name).unwrap_or_default()
            });
        }
        // FieldKind::Other, or no field at all: no-op, per the Review Focus
        // requirement and Task 5's scope, respectively (see doc comment).
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
                    } else if self.document.is_none() {
                        ui.label("Open a PDF via File → Open… to get started.");
                    }
                });
        });

        self.error_modal(&ctx);
    }
}

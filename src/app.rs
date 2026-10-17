//! The root `eframe::App` for PDF Utility.
//!
//! Owns the currently open document (if any), the viewer's page/zoom state,
//! and the rendered-page texture. File → Open and the zoom/page controls all
//! live here; the page/screen coordinate transform used to position overlays
//! lives in [`crate::ui::canvas`].

use std::path::{Path, PathBuf};

use pdf_utility::document::{DocumentError, PdfDocumentState};

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
            }
            Err(err) => {
                self.document = None;
                self.texture = None;
                self.texture_key = None;
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
                        ui.image(texture);
                    } else if self.document.is_none() {
                        ui.label("Open a PDF via File → Open… to get started.");
                    }
                });
        });

        self.error_modal(&ctx);
    }
}

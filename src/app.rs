//! The root `eframe::App` for PDF Utility.
//!
//! Owns the currently open document (if any), the viewer's page/zoom state,
//! and the rendered-page texture. File → Open and the zoom/page controls all
//! live here; the page/screen coordinate transform used to position overlays
//! lives in [`crate::ui::canvas`].

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use pdf_utility::annotations;
use pdf_utility::apply;
use pdf_utility::document::{DocumentError, PdfDocumentState};
use pdf_utility::form_fields::{self, FieldKind};
use pdf_utility::overlay::{self, OverlayElement, OverlayLayer, SignatureElement, TextBoxElement};
use pdf_utility::save;
use pdf_utility::signatures::{self, SignatureLibrary};
use uuid::Uuid;

use crate::ui::canvas;
use crate::ui::signature_panel::SignaturePanelState;

/// Smallest and largest zoom factors selectable via the zoom buttons.
const MIN_ZOOM: f32 = 0.25;
const MAX_ZOOM: f32 = 4.0;
const ZOOM_STEP: f32 = 0.25;

/// Default size (page points) for a newly-placed free-form text box.
const DEFAULT_TEXT_BOX_SIZE: egui::Vec2 = egui::vec2(160.0, 40.0);
/// Default width (page points) for a newly-placed signature; its height
/// follows from the rendered image's aspect ratio (see
/// `default_signature_rect`), so a wide scan and a short typed name both
/// land at a sensible page size rather than their raw pixel dimensions.
const DEFAULT_SIGNATURE_WIDTH: f32 = 160.0;
/// Smallest size (page points) an overlay element (text box or signature)
/// can be resized down to — a floor so dragging a corner past its opposite
/// edge can't collapse or invert the element.
const MIN_OVERLAY_ELEMENT_SIZE: egui::Vec2 = egui::vec2(24.0, 16.0);
/// On-screen size (pixels) of a selected element's corner resize handles.
const HANDLE_SIZE: f32 = 8.0;

/// The toolbar's current placement tool. `Select` is the default: clicks
/// hit-test existing overlay elements and AcroForm fields but don't create
/// anything. `AddTextBox` is toggled on via the "Add text box" button.
/// `PlaceSignature` is entered by clicking "Place on page" in the
/// signature panel for the carried library entry id, and is consumed by
/// the very next canvas click (whether it lands on empty space and places
/// the signature, or not) — it's a one-shot mode, not a toggle.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
enum ToolMode {
    #[default]
    Select,
    AddTextBox,
    PlaceSignature(Uuid),
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
/// only the edges that corner owns, then enforces [`MIN_OVERLAY_ELEMENT_SIZE`] on
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

    if max.x - min.x < MIN_OVERLAY_ELEMENT_SIZE.x {
        if corner.moves_left_edge() {
            min.x = max.x - MIN_OVERLAY_ELEMENT_SIZE.x;
        } else {
            max.x = min.x + MIN_OVERLAY_ELEMENT_SIZE.x;
        }
    }
    if max.y - min.y < MIN_OVERLAY_ELEMENT_SIZE.y {
        if corner.moves_top_edge() {
            min.y = max.y - MIN_OVERLAY_ELEMENT_SIZE.y;
        } else {
            max.y = min.y + MIN_OVERLAY_ELEMENT_SIZE.y;
        }
    }

    egui::Rect::from_min_max(min, max)
}

/// The default page-space rect for a newly-placed signature, anchored
/// with its top-left corner at `origin`: width is fixed at
/// [`DEFAULT_SIGNATURE_WIDTH`] and height follows from
/// `image_size`'s aspect ratio, so a wide scanned signature and a short
/// typed name both land at a sensible size on the page rather than their
/// raw pixel dimensions (which have no relation to page points). The
/// caller still runs the result through [`overlay::clamp_to_page`], same
/// as a text box.
fn default_signature_rect(origin: egui::Pos2, image_size: [usize; 2]) -> egui::Rect {
    let [width_px, height_px] = image_size;
    let aspect = height_px as f32 / (width_px.max(1) as f32);
    let size = egui::vec2(DEFAULT_SIGNATURE_WIDTH, DEFAULT_SIGNATURE_WIDTH * aspect);
    egui::Rect::from_min_size(origin, size)
}

/// Something the user asked for that would throw away unsaved changes, held
/// back while the "unsaved changes" confirmation is shown.
#[derive(Debug, Clone, PartialEq, Eq)]
enum PendingAction {
    /// File → Open picked this path.
    Open(PathBuf),
    /// The window's close button (or similar) was pressed.
    Close,
}

/// The user's answer in the "unsaved changes" confirmation.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum UnsavedChoice {
    /// Save first (Save, or Save As if there's no path yet), then continue
    /// only if that save succeeded.
    Save,
    /// Continue without saving, losing the changes.
    Discard,
    /// Don't continue; keep the current document open and edited.
    Cancel,
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
    /// The path the current document was opened from, or last saved to via
    /// "Save As" — i.e. the destination "Save" overwrites. Set on a
    /// successful open and updated after a successful "Save As"; cleared
    /// (along with the rest of the per-document state) when an open fails
    /// or when there's no document at all. "Save" is disabled whenever this
    /// is `None`, since there is then no destination path to overwrite.
    current_file_path: Option<PathBuf>,
    viewer: ViewerState,
    /// Set when the most recent open attempt failed; cleared when the user
    /// dismisses the error modal or a later open succeeds.
    error_message: Option<String>,
    /// A non-error confirmation to show the user, dismissed the same way as
    /// `error_message` but in its own modal (`info_modal`) so it's never
    /// confused with a failure. Set after a successful Apply.
    info_message: Option<String>,
    /// `true` once Apply has successfully flattened the current document —
    /// see `apply()`'s doc comment. There is no way to clear this back to
    /// `false` for an open document (pdfium's flatten is one-way); it's
    /// only ever reset by opening a different document. While `true`, every
    /// editing interaction (field click-to-edit, the "Add text box" tool
    /// and its placement, the signature panel's "Place on page", and all
    /// overlay selection/move/resize/delete) is disabled — the Review
    /// Focus requirement for this task — rather than silently accepted and
    /// then lost, since nothing entered after a flatten can ever be baked
    /// into the document's content again.
    applied: bool,
    /// `true` when the open document has changes not yet written to disk:
    /// set by every mutation (a committed field edit, adding / editing /
    /// moving / resizing / deleting an overlay element, a successful
    /// Apply), cleared by a successful Save / Save As and by opening a
    /// document. While `true`, File → Open and closing the window first ask
    /// for confirmation (see `pending_action`).
    dirty: bool,
    /// An Open or Close held back while the "unsaved changes" confirmation
    /// is shown; `None` when no confirmation is pending.
    pending_action: Option<PendingAction>,
    /// Set once the user has confirmed closing (by discarding or saving),
    /// so the close request that follows isn't intercepted again.
    close_confirmed: bool,
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
    /// Set for exactly one frame after a field is focused by a click, so
    /// `draw_field_editor` gives its `TextEdit` keyboard focus once (and a
    /// later focus loss reliably commits the edit).
    request_field_focus: bool,
    /// The current document's free-form overlay elements (text boxes, and
    /// — from Task 8 — placed signatures), covering every page.
    overlay: OverlayLayer,
    /// GPU textures for placed `SignatureElement`s, keyed by the
    /// element's own id (not the library entry it was placed from) — each
    /// one is uploaded lazily in `draw_overlay` the first time it's drawn,
    /// then reused every later frame, mirroring how
    /// `ui::signature_panel::SignaturePanelState` caches library
    /// thumbnails. Cleared alongside `overlay` in `open_path`, and an
    /// entry is dropped whenever its element is deleted, so this can't
    /// grow to outlive the elements it caches.
    signature_textures: HashMap<Uuid, egui::TextureHandle>,
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
    /// The personal signature library (Task 7): saved signatures, backed
    /// by a manifest + image files under `signature_dir`. Independent of
    /// any open document, so unlike the fields above it is never reset in
    /// `open_path`.
    signature_library: SignatureLibrary,
    /// Where `signature_library` is persisted — resolved once here, at
    /// startup, to `~/Library/Application Support/<bundle id>/signatures/`
    /// (see `signature_library_dir`).
    signature_dir: PathBuf,
    /// Transient UI state (in-progress "add typed" form, thumbnail texture
    /// cache) for the signature side panel.
    signature_panel: SignaturePanelState,
}

impl PdfUtilityApp {
    /// Creates the app, optionally opening `initial_path` immediately (used
    /// for launch-arg / Finder double-click open).
    pub fn new(initial_path: Option<PathBuf>) -> Self {
        let signature_dir = signature_library_dir();
        let signature_library = SignatureLibrary::load(&signature_dir);

        let mut app = Self {
            document: None,
            current_file_path: None,
            viewer: ViewerState::default(),
            error_message: None,
            info_message: None,
            applied: false,
            dirty: false,
            pending_action: None,
            close_confirmed: false,
            texture: None,
            texture_key: None,
            focused_field: None,
            field_values: HashMap::new(),
            request_field_focus: false,
            overlay: OverlayLayer::default(),
            signature_textures: HashMap::new(),
            tool_mode: ToolMode::default(),
            selected_element: None,
            editing_text_box: None,
            request_focus_for: None,
            signature_library,
            signature_dir,
            signature_panel: SignaturePanelState::new(),
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
        // Text boxes / signatures saved by an earlier session come back as
        // live, editable overlay elements (and are taken off the page
        // itself, so they aren't also drawn underneath as static pixels).
        let opened = PdfDocumentState::open(path).and_then(|mut document| {
            let overlay = annotations::take_overlay_from_document(&mut document)?;
            Ok((document, overlay))
        });
        match opened {
            Ok((document, overlay)) => {
                self.document = Some(document);
                self.current_file_path = Some(path.to_path_buf());
                self.viewer = ViewerState::default();
                self.error_message = None;
                self.info_message = None;
                self.applied = false;
                self.dirty = false;
                self.pending_action = None;
                self.texture = None;
                self.texture_key = None;
                self.focused_field = None;
                self.field_values.clear();
                self.overlay = overlay;
                self.signature_textures.clear();
                self.tool_mode = ToolMode::default();
                self.selected_element = None;
                self.editing_text_box = None;
                self.request_focus_for = None;
            }
            Err(err) => {
                self.document = None;
                self.current_file_path = None;
                self.info_message = None;
                self.applied = false;
                self.dirty = false;
                self.pending_action = None;
                self.texture = None;
                self.texture_key = None;
                self.focused_field = None;
                self.field_values.clear();
                self.overlay = OverlayLayer::default();
                self.signature_textures.clear();
                self.tool_mode = ToolMode::default();
                self.selected_element = None;
                self.editing_text_box = None;
                self.request_focus_for = None;
                self.error_message = Some(error_message_for(&err));
            }
        }
    }

    /// File → Open's entry point: opens `path` straight away if there's
    /// nothing unsaved, otherwise holds it back as a [`PendingAction`] so
    /// the "unsaved changes" confirmation is shown first. A field edit
    /// still sitting in its editor counts as a change.
    fn request_open(&mut self, path: PathBuf) {
        self.commit_field_edit();
        if self.dirty {
            self.pending_action = Some(PendingAction::Open(path));
        } else {
            self.open_path(&path);
        }
    }

    /// Called when the window has been asked to close. Returns `true` if
    /// the close must be cancelled (there are unsaved changes the user
    /// hasn't confirmed losing), in which case the confirmation is queued.
    fn intercept_close(&mut self) -> bool {
        if self.close_confirmed {
            return false;
        }
        self.commit_field_edit();
        if self.dirty {
            self.pending_action = Some(PendingAction::Close);
            true
        } else {
            false
        }
    }

    /// Acts on the user's answer to the "unsaved changes" confirmation for
    /// the pending action. Returns `true` if the window should now close.
    fn resolve_pending_action(&mut self, choice: UnsavedChoice) -> bool {
        let Some(action) = self.pending_action.take() else {
            return false;
        };

        match choice {
            UnsavedChoice::Cancel => return false,
            UnsavedChoice::Save => {
                if self.current_file_path.is_some() {
                    self.save();
                } else {
                    self.save_as();
                }
                if self.dirty {
                    // Save failed or was cancelled: keep everything as is.
                    return false;
                }
            }
            UnsavedChoice::Discard => {}
        }

        match action {
            PendingAction::Open(path) => {
                self.open_path(&path);
                false
            }
            PendingAction::Close => {
                self.close_confirmed = true;
                true
            }
        }
    }

    /// Draws the "unsaved changes" confirmation while an action is pending.
    fn unsaved_changes_modal(&mut self, ctx: &egui::Context) {
        let Some(action) = self.pending_action.clone() else {
            return;
        };
        let what = match action {
            PendingAction::Open(_) => "opening another file",
            PendingAction::Close => "closing",
        };

        let mut choice = None;
        let response = egui::Modal::new(egui::Id::unique("unsaved_changes_modal")).show(ctx, |ui| {
            ui.set_max_width(340.0);
            ui.label(format!(
                "This document has unsaved changes. Save them before {what}?"
            ));
            ui.add_space(8.0);
            ui.horizontal(|ui| {
                if ui.button("Save").clicked() {
                    choice = Some(UnsavedChoice::Save);
                }
                if ui.button("Don't Save").clicked() {
                    choice = Some(UnsavedChoice::Discard);
                }
                if ui.button("Cancel").clicked() {
                    choice = Some(UnsavedChoice::Cancel);
                }
            });
        });
        if choice.is_none() && response.should_close() {
            choice = Some(UnsavedChoice::Cancel);
        }

        if let Some(choice) = choice
            && self.resolve_pending_action(choice)
        {
            ctx.send_viewport_cmd(egui::ViewportCommand::Close);
        }
    }

    /// Writes the current document and overlay out to `path` via
    /// [`save::save`], reporting any failure the same way an open/render
    /// failure is reported. Returns whether the save succeeded, so callers
    /// (`save_as`) can decide whether to adopt `path` as the new
    /// `current_file_path`.
    fn save_to(&mut self, path: &Path) -> bool {
        // An edit still sitting in the field editor's buffer is part of
        // what the user expects to be saved.
        self.commit_field_edit();
        let Some(document) = &mut self.document else {
            return false;
        };

        match save::save(document, &self.overlay, path) {
            Ok(()) => {
                self.dirty = false;
                true
            }
            Err(err) => {
                self.error_message = Some(error_message_for(&err));
                false
            }
        }
    }

    /// "Save": overwrites `current_file_path`. A no-op if there is no
    /// current document or no path to overwrite — callers (the File menu)
    /// are expected to disable the button in that case, but this guards
    /// against it regardless.
    fn save(&mut self) {
        let Some(path) = self.current_file_path.clone() else {
            return;
        };
        self.save_to(&path);
    }

    /// "Save As": always opens a native save dialog, writes to the chosen
    /// path, and — only on success — updates `current_file_path` to it so a
    /// subsequent "Save" overwrites the new location rather than the old
    /// one.
    fn save_as(&mut self) {
        if self.document.is_none() {
            return;
        }
        let Some(path) = rfd::FileDialog::new()
            .add_filter("PDF", &["pdf"])
            .set_file_name("document.pdf")
            .save_file()
        else {
            return;
        };

        if self.save_to(&path) {
            self.current_file_path = Some(path);
        }
    }

    /// "Apply": via `apply::apply`, writes every current overlay element
    /// onto its page as an annotation (replacing any of the app's own
    /// annotations already there), then flattens the whole document so
    /// those annotations — and the AcroForm fields already filled via
    /// `form_fields::set_field_value` — become part of each page's content
    /// and stop being separate, editable objects.
    ///
    /// On success: sets `applied`, clears the now-baked-in overlay (leaving
    /// it populated would let a later `save` re-write those same elements
    /// as *new* annotations on top of the flattened content, duplicating
    /// them) and every selection/editing/tool-mode field, and shows a
    /// confirmation. A no-op if there's no current document. On failure
    /// (either step), reports the error and leaves the document unflattened
    /// and still editable — `applied` only ever becomes `true` once the
    /// document has actually been flattened.
    fn apply(&mut self) {
        if self.document.is_none() {
            return;
        }
        // Bake the field editor's pending value too, not the stale one.
        self.commit_field_edit();

        let document = self.document.as_mut().unwrap();
        let result = apply::apply(document, &self.overlay);
        // The page's rendered content changed (or, on a part-way failure,
        // may have): never keep showing the pre-Apply texture.
        self.invalidate_page_texture();
        match result {
            Ok(()) => {
                self.applied = true;
                self.dirty = true;
                self.overlay = OverlayLayer::default();
                self.signature_textures.clear();
                self.tool_mode = ToolMode::default();
                self.selected_element = None;
                self.editing_text_box = None;
                self.request_focus_for = None;
                self.focused_field = None;
                self.info_message =
                    Some("This document is now locked; Save or Save As to keep it.".to_string());
            }
            Err(err) => {
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
                        self.request_open(path);
                    }
                }

                ui.separator();

                // Disabled (greyed out) whenever there's no destination path
                // to overwrite — no document open, or one that was opened
                // but never came from/was saved to a real path.
                if ui
                    .add_enabled(
                        self.current_file_path.is_some(),
                        egui::Button::new("Save"),
                    )
                    .clicked()
                {
                    ui.close();
                    self.save();
                }

                if ui
                    .add_enabled(self.document.is_some(), egui::Button::new("Save As…"))
                    .clicked()
                {
                    ui.close();
                    self.save_as();
                }
            });
        });
    }

    /// Draws the "Apply" control (greyed out once the document is already
    /// `applied`, since pdfium's flatten is one-way and there's no "apply
    /// again") plus a persistent "Document locked" label while applied, so
    /// the lockout is visible even when the toolbar's other editing
    /// controls are also greyed out.
    fn draw_apply_control(&mut self, ui: &mut egui::Ui) {
        if ui
            .add_enabled(!self.applied, egui::Button::new("Apply"))
            .on_hover_text(
                "Flattens filled fields and overlay elements into the page, \
                 then locks the document from further editing.",
            )
            .clicked()
        {
            self.apply();
        }

        if self.applied {
            ui.separator();
            ui.colored_label(egui::Color32::from_rgb(160, 120, 0), "Document locked");
        }
    }

    /// Draws the page navigation and zoom controls. Disabled (not shown)
    /// when no document is open.
    fn toolbar(&mut self, ui: &mut egui::Ui) {
        let Some(document) = &self.document else {
            return;
        };
        let page_count = document.page_count();
        // Computed up front (rather than read from `document` inside the
        // closure below) so the immutable borrow of `self.document` ends
        // before the closure starts — it also calls `self.draw_apply_control`,
        // which needs `&mut self`.
        let current_page_size = (self.viewer.current_page < page_count)
            .then(|| document.page_size_points(self.viewer.current_page));

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
            // `Result`, so this bound is checked explicitly (above, via
            // `current_page_size`) rather than trusted.
            if let Some(size) = current_page_size {
                ui.separator();
                ui.label(format!("{:.0} × {:.0} pt", size.x, size.y));
            }

            ui.separator();

            let add_text_box_active = self.tool_mode == ToolMode::AddTextBox;
            if ui
                .add_enabled(
                    !self.applied,
                    egui::Button::selectable(add_text_box_active, "Add text box"),
                )
                .clicked()
            {
                self.tool_mode = if add_text_box_active {
                    ToolMode::Select
                } else {
                    ToolMode::AddTextBox
                };
            }

            // `applied` forces `tool_mode` back to `Select` as soon as it
            // becomes true (see `apply()`), so this can only be reached
            // pre-Apply; no `applied` guard needed here beyond that.
            if matches!(self.tool_mode, ToolMode::PlaceSignature(_)) {
                ui.separator();
                ui.label("Click the page to place the signature");
                if ui.button("Cancel").clicked() {
                    self.tool_mode = ToolMode::Select;
                }
            }

            ui.separator();
            self.draw_apply_control(ui);
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
    ///
    /// Once `self.applied` is `true` this is an unconditional no-op: every
    /// one of the above (overlay selection, field focus, text box/signature
    /// placement) is an editing interaction that the Review Focus
    /// requirement says must be inert post-Apply, not just absent because
    /// there happens to be nothing left to hit.
    fn handle_canvas_click(&mut self, click_pos: egui::Pos2, image_rect: egui::Rect) {
        if self.applied {
            return;
        }
        // Whatever this click lands on, an in-progress field edit is
        // finished by it — commit it before focus can move elsewhere (the
        // old field's editor won't be drawn again to notice its own focus
        // loss once `focused_field` points somewhere else).
        self.commit_field_edit();
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
                self.request_field_focus = true;
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
                match self.tool_mode {
                    ToolMode::AddTextBox => {
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
                        self.dirty = true;
                    }
                    ToolMode::PlaceSignature(entry_id) => {
                        self.place_signature_from_library(entry_id, page_point);
                        // One-shot, per `ToolMode`'s doc comment: consumed
                        // by this click whether or not the entry was still
                        // found (e.g. deleted from the panel after the
                        // mode was entered but before this click landed).
                        self.tool_mode = ToolMode::Select;
                    }
                    ToolMode::Select => {
                        self.selected_element = None;
                        self.editing_text_box = None;
                    }
                }
            }
        }
    }

    /// Renders library entry `entry_id` (via `signatures::render_signature_entry`)
    /// and adds it to the overlay as a new, selected `SignatureElement`
    /// anchored with its top-left corner at `origin` (page space), sized
    /// via [`default_signature_rect`] and clamped to the page exactly like
    /// a text box. A no-op (not even clearing the current selection) if
    /// `entry_id` no longer exists in the library.
    fn place_signature_from_library(&mut self, entry_id: Uuid, origin: egui::Pos2) {
        let Some(document) = &self.document else {
            return;
        };
        let Some(entry) = self
            .signature_library
            .entries
            .iter()
            .find(|entry| entry.id == entry_id)
        else {
            return;
        };

        let page_size = document.page_size_points(self.viewer.current_page);
        let image = signatures::render_signature_entry(entry, &self.signature_dir);
        let rect = overlay::clamp_to_page(default_signature_rect(origin, image.size), page_size);

        let id = Uuid::new_v4();
        self.overlay
            .add(OverlayElement::Signature(SignatureElement {
                id,
                page_index: self.viewer.current_page,
                rect,
                image,
            }));
        self.selected_element = Some(id);
        self.editing_text_box = None;
        self.dirty = true;
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
        // `apply()` always clears `focused_field` on success, so this is
        // belt-and-suspenders defense, consistent with the explicit
        // `applied` guards in `handle_canvas_click` and `draw_overlay`: a
        // field editor must never be interactable post-Apply, regardless of
        // what state happens to clear it first.
        if self.applied {
            return;
        }
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
        let response = ui.put(
            screen_rect,
            egui::TextEdit::singleline(buffer).id_salt(("field_editor", &name)),
        );
        if self.request_field_focus {
            response.request_focus();
            self.request_field_focus = false;
        }

        // Typing only edits the buffer; the value is written to the
        // document when the edit is committed (here on focus loss —
        // Enter, Tab, Escape or clicking elsewhere — and also before any
        // click is routed, Save, Apply, or Open; see `commit_field_edit`).
        // Committing is a whole-document round trip (it regenerates the
        // field's on-page appearance), too heavy to repeat per keystroke.
        if response.lost_focus() {
            self.commit_field_edit();
            self.focused_field = None;
        }
    }

    /// Writes the focused field's editor buffer to the document, if it
    /// differs from the field's current value, via
    /// `form_fields::set_field_value` (which also regenerates the field's
    /// on-page appearance) and drops the cached page texture so the canvas
    /// re-renders with the new value. A no-op if no field is focused.
    fn commit_field_edit(&mut self) {
        let Some(name) = self.focused_field.clone() else {
            return;
        };
        let Some(buffer) = self.field_values.get(&name).cloned() else {
            return;
        };
        let Some(document) = &mut self.document else {
            return;
        };
        if form_fields::get_field_value(document, &name).unwrap_or_default() == buffer {
            return;
        }

        let result = form_fields::set_field_value(document, &name, &buffer);
        // Invalidate (and mark dirty) even on failure: `set_field_value`
        // may already have changed `/V` before the appearance refresh
        // failed.
        self.invalidate_page_texture();
        self.dirty = true;
        if let Err(err) = result {
            self.error_message = Some(error_message_for(&err));
        }
    }

    /// Forces `ensure_texture` to re-render the current page on the next
    /// frame. Needed after anything that changes the document's rendered
    /// content without changing the (page, zoom) cache key — a committed
    /// field edit, or Apply.
    fn invalidate_page_texture(&mut self) {
        self.texture_key = None;
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
        // `apply()` clears the overlay on success, so this loop would
        // already have nothing to iterate post-Apply; the explicit guard
        // here is the real gate the Review Focus requirement asks for
        // (selection/move/resize/delete must be disabled, not just
        // incidentally absent), and also skips the Delete-key handler at
        // the bottom of this method.
        if self.applied {
            return;
        }
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
                    if response.changed() {
                        self.dirty = true;
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
                match element {
                    OverlayElement::TextBox(text_box) => {
                        ui.painter().text(
                            screen_rect.left_top() + egui::vec2(4.0, 2.0),
                            egui::Align2::LEFT_TOP,
                            &text_box.text,
                            egui::FontId::default(),
                            ui.visuals().text_color(),
                        );
                    }
                    OverlayElement::Signature(signature) => {
                        // Uploaded lazily, once per element id, then reused
                        // every later frame — see `signature_textures`'s
                        // doc comment.
                        let texture = self.signature_textures.entry(id).or_insert_with(|| {
                            ui.ctx().load_texture(
                                format!("overlay-signature-{id}"),
                                signature.image.clone(),
                                egui::TextureOptions::default(),
                            )
                        });
                        ui.painter().image(
                            texture.id(),
                            screen_rect,
                            egui::Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(1.0, 1.0)),
                            egui::Color32::WHITE,
                        );
                    }
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
                    if body_response.drag_delta() != egui::Vec2::ZERO {
                        self.dirty = true;
                    }
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
                        if handle_response.drag_delta() != egui::Vec2::ZERO {
                            self.dirty = true;
                        }
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
            self.remove_overlay_element(id);
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
            self.remove_overlay_element(id);
        }
    }

    /// Removes overlay element `id` and any state that refers to it:
    /// `selected_element`/`editing_text_box` if either currently points at
    /// it, and its cached GPU texture in `signature_textures` if it has
    /// one (a no-op for a text box, which never gets an entry there).
    fn remove_overlay_element(&mut self, id: Uuid) {
        if self.overlay.get(id).is_some() {
            self.dirty = true;
        }
        self.overlay.remove(id);
        self.signature_textures.remove(&id);
        if self.selected_element == Some(id) {
            self.selected_element = None;
        }
        if self.editing_text_box == Some(id) {
            self.editing_text_box = None;
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

    /// Draws the informational confirmation modal (currently only used for
    /// the post-Apply "document is now locked" message), if one is
    /// currently set. Dismissible the same way as `error_modal`, but kept
    /// as a separate modal/field so a successful confirmation is never
    /// mistaken for an error.
    fn info_modal(&mut self, ctx: &egui::Context) {
        let Some(message) = self.info_message.clone() else {
            return;
        };

        let response = egui::Modal::new(egui::Id::unique("apply_info_modal")).show(ctx, |ui| {
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
            self.info_message = None;
        }
    }
}

/// The bundle id from `packaging/Info.plist`'s `CFBundleIdentifier`, used
/// to namespace this app's folder under Application Support.
const BUNDLE_ID: &str = "com.brightech.pdfutility";

/// Resolves the signature library's storage directory:
/// `~/Library/Application Support/<bundle id>/signatures/`. Computed once,
/// here, at app startup (from [`PdfUtilityApp::new`]) rather than
/// recomputed wherever the library is touched, so every part of the app
/// agrees on where it lives for the life of the process — the directory
/// itself is created lazily by `SignatureLibrary::save`/`add_image`, not
/// here, so a fresh install with no saved signatures yet doesn't leave an
/// empty folder behind just from opening the app.
///
/// Falls back to the current directory if `$HOME` isn't set, which
/// shouldn't happen on a normal macOS login session but shouldn't be
/// allowed to panic the app if it somehow did.
fn signature_library_dir() -> PathBuf {
    let home = std::env::var_os("HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("."));
    home.join("Library/Application Support")
        .join(BUNDLE_ID)
        .join("signatures")
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

        let mut place_requested: Option<Uuid> = None;
        egui::Panel::right("signature_panel")
            .default_size(220.0)
            .show(ui, |ui| {
                egui::ScrollArea::vertical().show(ui, |ui| {
                    place_requested = self.signature_panel.ui(
                        ui,
                        &mut self.signature_library,
                        &self.signature_dir,
                        self.applied,
                    );
                });
            });
        // The panel's "Place on page" button is itself disabled while
        // `applied` (via the `locked` parameter above), so `place_requested`
        // should never be `Some` here when applied — this guard is
        // defense-in-depth, not the primary gate.
        if let Some(entry_id) = place_requested
            && !self.applied
        {
            // Entering placement mode always starts from a clean slate,
            // same as toggling on "Add text box" — this takes priority
            // over whatever was selected/being edited a moment ago.
            self.tool_mode = ToolMode::PlaceSignature(entry_id);
            self.selected_element = None;
            self.editing_text_box = None;
        }

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

        // Closing the window with unsaved changes: cancel the close this
        // frame (eframe only exits if no `CancelClose` was sent during the
        // frame that saw the request) and ask first.
        if ctx.input(|input| input.viewport().close_requested()) && self.intercept_close() {
            ctx.send_viewport_cmd(egui::ViewportCommand::CancelClose);
        }

        self.error_modal(&ctx);
        self.info_modal(&ctx);
        self.unsaved_changes_modal(&ctx);
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

    /// Counts opaque, darker-than-mid-grey pixels of page `page_index`
    /// (rendered at zoom 1) inside `page_rect` — i.e. whether something is
    /// actually drawn there, not whether some object merely exists.
    fn ink_in(document: &PdfDocumentState, page_index: usize, page_rect: egui::Rect) -> usize {
        let image = document.render_page(page_index, 1.0).unwrap();
        let [width, height] = image.size;
        let mut count = 0;
        for y in (page_rect.min.y.max(0.0) as usize)..(page_rect.max.y as usize).min(height) {
            for x in (page_rect.min.x.max(0.0) as usize)..(page_rect.max.x as usize).min(width) {
                let p = image.pixels[y * width + x];
                if p.a() > 128 && (p.r() as u32 + p.g() as u32 + p.b() as u32) < 384 {
                    count += 1;
                }
            }
        }
        count
    }

    fn applicant_field_rect(app: &PdfUtilityApp) -> egui::Rect {
        form_fields::fields_on_page(app.document.as_ref().unwrap(), 0)
            .into_iter()
            .find(|field| field.name == "applicant_name")
            .unwrap()
            .page_rect
    }

    fn big_image_rect() -> egui::Rect {
        egui::Rect::from_min_size(egui::pos2(0.0, 0.0), egui::vec2(1000.0, 1000.0))
    }

    /// Final review Critical #3, app side: a committed field edit must
    /// reach the document *visibly* and drop the cached page texture, so
    /// the canvas re-renders instead of showing the pre-edit page.
    #[test]
    fn committing_a_field_edit_renders_the_value_and_invalidates_the_page_texture() {
        let mut app = app_with_form_pdf();
        let ctx = egui::Context::default();
        app.ensure_texture(&ctx);
        assert!(app.texture_key.is_some());

        let rect = applicant_field_rect(&app);
        app.handle_canvas_click(rect.center(), big_image_rect());
        assert_eq!(app.focused_field.as_deref(), Some("applicant_name"));
        app.field_values
            .insert("applicant_name".to_string(), "Jane Doe".to_string());

        app.commit_field_edit();

        assert!(app.error_message.is_none(), "{:?}", app.error_message);
        assert!(app.texture_key.is_none(), "stale page texture kept");
        let document = app.document.as_ref().unwrap();
        assert_eq!(
            form_fields::get_field_value(document, "applicant_name"),
            Some("Jane Doe".to_string())
        );
        assert!(ink_in(document, 0, rect) > 50);
    }

    /// Clicking away from a field being edited (here: onto empty page
    /// space) commits what was typed, rather than dropping it.
    #[test]
    fn clicking_away_from_a_field_commits_its_edit() {
        let mut app = app_with_form_pdf();
        let rect = applicant_field_rect(&app);
        app.handle_canvas_click(rect.center(), big_image_rect());
        app.field_values
            .insert("applicant_name".to_string(), "Jane Doe".to_string());

        app.handle_canvas_click(egui::pos2(5.0, 5.0), big_image_rect());

        assert_eq!(app.focused_field, None);
        let document = app.document.as_ref().unwrap();
        assert_eq!(
            form_fields::get_field_value(document, "applicant_name"),
            Some("Jane Doe".to_string())
        );
        assert!(ink_in(document, 0, rect) > 50);
    }

    /// Opening a document sets `current_file_path` to it — this is what
    /// "Save" (as opposed to "Save As") overwrites, and what gates whether
    /// the Save menu item is enabled at all.
    #[test]
    fn opening_a_document_sets_current_file_path() {
        let app = app_with_plain_pdf();
        assert_eq!(
            app.current_file_path,
            Some(PathBuf::from("tests/fixtures/sample_plain.pdf"))
        );
    }

    /// A failed open must leave `current_file_path` cleared, not pointing at
    /// a path whose document failed to load — otherwise "Save" could be
    /// enabled with no real document behind it.
    #[test]
    fn failed_open_clears_current_file_path() {
        let mut app = app_with_plain_pdf();
        app.open_path(Path::new("tests/fixtures/corrupt.pdf"));
        assert!(app.current_file_path.is_none());
        assert!(app.document.is_none());
    }

    /// `save()` (the "Save" menu item's handler) with no current document is
    /// a harmless no-op — it must not panic even though the File menu is
    /// expected to disable "Save" in that state.
    #[test]
    fn save_with_no_document_is_a_noop() {
        let mut app = PdfUtilityApp::new(None);
        app.save();
        assert!(app.error_message.is_none());
    }

    /// `save()` writes the document (with its current field values and
    /// overlay) to `current_file_path`, overwriting it in place — exercising
    /// the real menu-wired path end-to-end, as opposed to `save_test.rs`,
    /// which calls `pdf_utility::save::save` directly.
    #[test]
    fn save_overwrites_current_file_path() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("doc.pdf");
        std::fs::copy("tests/fixtures/sample_form.pdf", &path).unwrap();

        let mut app = PdfUtilityApp::new(Some(path.clone()));
        form_fields::set_field_value(
            app.document.as_mut().unwrap(),
            "applicant_name",
            "Jane Doe",
        )
        .unwrap();

        app.save();

        assert!(app.error_message.is_none());
        let reopened = PdfDocumentState::open(&path).unwrap();
        assert_eq!(
            form_fields::get_field_value(&reopened, "applicant_name"),
            Some("Jane Doe".to_string())
        );
    }

    /// Final review Critical #4, app side: a text box saved by one session
    /// is a live, editable overlay element after File → Open in the next
    /// (not just pixels baked into the page), and saving again in place
    /// doesn't duplicate it.
    #[test]
    fn open_path_restores_saved_overlay_elements_as_editable() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("doc.pdf");
        std::fs::copy("tests/fixtures/sample_plain.pdf", &path).unwrap();
        let rect = egui::Rect::from_min_size(egui::pos2(100.0, 300.0), egui::vec2(200.0, 40.0));

        let mut app = PdfUtilityApp::new(Some(path.clone()));
        app.overlay.add(OverlayElement::TextBox(TextBoxElement {
            id: Uuid::new_v4(),
            page_index: 0,
            rect,
            text: "HELLO WORLD".into(),
        }));
        app.save();
        assert!(app.error_message.is_none());

        let mut app = PdfUtilityApp::new(Some(path.clone()));
        assert!(app.error_message.is_none());
        assert_eq!(app.overlay.elements.len(), 1);
        assert!(matches!(
            &app.overlay.elements[0],
            OverlayElement::TextBox(tb) if tb.text == "HELLO WORLD"
        ));
        // Not also drawn as static pixels under the live element.
        assert_eq!(ink_in(app.document.as_ref().unwrap(), 0, rect), 0);
        // ...and it's selectable through the real click path.
        app.handle_canvas_click(rect.center(), big_image_rect());
        assert_eq!(app.selected_element, Some(app.overlay.elements[0].id()));

        app.save();
        let reopened = PdfDocumentState::open(&path).unwrap();
        assert_eq!(
            pdf_utility::annotations::read_overlay_from_annotations(&reopened, 0).len(),
            1
        );
    }

    /// An app with a private, writable copy of `fixture` open (so Save in a
    /// test can never overwrite a fixture).
    fn app_with_temp_copy(fixture: &str) -> (tempfile::TempDir, PathBuf, PdfUtilityApp) {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("doc.pdf");
        std::fs::copy(fixture, &path).unwrap();
        let app = PdfUtilityApp::new(Some(path.clone()));
        (tmp, path, app)
    }

    fn add_text_box_by_click(app: &mut PdfUtilityApp, page_point: egui::Pos2) {
        app.tool_mode = ToolMode::AddTextBox;
        app.handle_canvas_click(page_point, big_image_rect());
        app.tool_mode = ToolMode::Select;
    }

    /// Final review Important #6: the dirty flag's transitions — clean on
    /// open, set by each kind of mutation, cleared by a successful save.
    #[test]
    fn dirty_flag_tracks_mutations_and_saves() {
        let (_tmp, _path, mut app) = app_with_temp_copy("tests/fixtures/sample_form.pdf");
        assert!(!app.dirty, "freshly opened document is clean");

        // Adding an overlay element.
        add_text_box_by_click(&mut app, egui::pos2(5.0, 5.0));
        assert!(app.dirty);
        app.save();
        assert!(app.error_message.is_none(), "{:?}", app.error_message);
        assert!(!app.dirty, "a successful save clears the flag");

        // Deleting an overlay element.
        let id = app.overlay.elements[0].id();
        app.remove_overlay_element(id);
        assert!(app.dirty);
        app.save();
        assert!(!app.dirty);

        // Placing a signature.
        let dir = app.signature_dir.clone();
        let entry_id = app
            .signature_library
            .add_typed(&dir, "Jane", "Jane Doe", "default");
        app.place_signature_from_library(entry_id, egui::pos2(300.0, 300.0));
        app.signature_library.entries.retain(|entry| entry.id != entry_id);
        assert!(app.dirty);
        app.save();
        assert!(!app.dirty);

        // A committed field edit — but re-committing the same value isn't
        // a change.
        let rect = applicant_field_rect(&app);
        app.handle_canvas_click(rect.center(), big_image_rect());
        app.commit_field_edit();
        assert!(!app.dirty, "focusing a field without changing it is not an edit");
        app.field_values
            .insert("applicant_name".to_string(), "Jane Doe".to_string());
        app.commit_field_edit();
        assert!(app.dirty);
        app.save();
        assert!(!app.dirty);

        // A successful Apply.
        app.apply();
        assert!(app.applied);
        assert!(app.dirty, "an applied-but-unsaved document is unsaved work");
        app.save();
        assert!(!app.dirty);
    }

    /// A failed save leaves the document dirty.
    #[test]
    fn failed_save_leaves_document_dirty() {
        let mut app = app_with_plain_pdf();
        add_text_box_by_click(&mut app, egui::pos2(5.0, 5.0));
        assert!(!app.save_to(Path::new("/nonexistent-dir/for/sure/out.pdf")));
        assert!(app.dirty);
    }

    /// File → Open on a clean document opens straight away.
    #[test]
    fn open_with_no_unsaved_changes_proceeds_immediately() {
        let mut app = app_with_plain_pdf();
        app.request_open(PathBuf::from("tests/fixtures/sample_form.pdf"));
        assert!(app.pending_action.is_none());
        assert_eq!(
            app.current_file_path,
            Some(PathBuf::from("tests/fixtures/sample_form.pdf"))
        );
    }

    /// File → Open with unsaved changes asks first: Cancel keeps the
    /// current document and its edits; Don't Save then opens the new file.
    #[test]
    fn open_with_unsaved_changes_asks_first() {
        let mut app = app_with_plain_pdf();
        add_text_box_by_click(&mut app, egui::pos2(5.0, 5.0));

        app.request_open(PathBuf::from("tests/fixtures/sample_form.pdf"));
        assert_eq!(
            app.pending_action,
            Some(PendingAction::Open(PathBuf::from("tests/fixtures/sample_form.pdf")))
        );
        assert_eq!(
            app.current_file_path,
            Some(PathBuf::from("tests/fixtures/sample_plain.pdf")),
            "opened before the user answered"
        );

        assert!(!app.resolve_pending_action(UnsavedChoice::Cancel));
        assert!(app.pending_action.is_none());
        assert!(app.dirty);
        assert_eq!(app.overlay.elements.len(), 1, "cancel lost the edit");
        assert_eq!(
            app.current_file_path,
            Some(PathBuf::from("tests/fixtures/sample_plain.pdf"))
        );

        app.request_open(PathBuf::from("tests/fixtures/sample_form.pdf"));
        assert!(!app.resolve_pending_action(UnsavedChoice::Discard));
        assert_eq!(
            app.current_file_path,
            Some(PathBuf::from("tests/fixtures/sample_form.pdf"))
        );
        assert!(!app.dirty);
        assert!(app.overlay.elements.is_empty());
    }

    /// An uncommitted field edit still in its editor counts as unsaved.
    #[test]
    fn uncommitted_field_edit_counts_as_unsaved_on_open() {
        let mut app = app_with_form_pdf();
        let rect = applicant_field_rect(&app);
        app.handle_canvas_click(rect.center(), big_image_rect());
        app.field_values
            .insert("applicant_name".to_string(), "Jane Doe".to_string());

        app.request_open(PathBuf::from("tests/fixtures/sample_plain.pdf"));

        assert!(matches!(app.pending_action, Some(PendingAction::Open(_))));
    }

    /// Closing with unsaved changes is intercepted; Don't Save lets the
    /// close through (and doesn't intercept the follow-up request).
    #[test]
    fn close_with_unsaved_changes_is_intercepted_until_confirmed() {
        let mut app = app_with_plain_pdf();
        assert!(!app.intercept_close(), "a clean document closes freely");

        add_text_box_by_click(&mut app, egui::pos2(5.0, 5.0));
        assert!(app.intercept_close());
        assert_eq!(app.pending_action, Some(PendingAction::Close));

        assert!(!app.resolve_pending_action(UnsavedChoice::Cancel));
        assert!(app.intercept_close(), "cancel, then close again: still asks");

        assert!(app.resolve_pending_action(UnsavedChoice::Discard));
        assert!(!app.intercept_close());
    }

    /// Save in the confirmation really writes the file, then closes.
    #[test]
    fn save_choice_writes_the_file_then_closes() {
        let (_tmp, path, mut app) = app_with_temp_copy("tests/fixtures/sample_plain.pdf");
        add_text_box_by_click(&mut app, egui::pos2(5.0, 5.0));
        assert!(app.intercept_close());

        assert!(app.resolve_pending_action(UnsavedChoice::Save));
        assert!(!app.dirty);
        let reopened = PdfDocumentState::open(&path).unwrap();
        assert_eq!(
            pdf_utility::annotations::read_overlay_from_annotations(&reopened, 0).len(),
            1
        );
    }

    /// If that save fails, nothing is lost: the window stays open and the
    /// document stays dirty.
    #[test]
    fn failed_save_choice_does_not_close() {
        let mut app = app_with_plain_pdf();
        add_text_box_by_click(&mut app, egui::pos2(5.0, 5.0));
        app.current_file_path = Some(PathBuf::from("/nonexistent-dir/for/sure/out.pdf"));
        assert!(app.intercept_close());

        assert!(!app.resolve_pending_action(UnsavedChoice::Save));
        assert!(app.dirty);
        assert!(app.error_message.is_some());
        assert!(app.intercept_close(), "the next close attempt still asks");
    }

    /// `save_to` (the shared helper behind both "Save" and "Save As") writes
    /// successfully and reports success, exercising the same path
    /// `save_as` uses after the (untestable, native) save dialog returns a
    /// path — this covers that it works without driving the dialog itself.
    #[test]
    fn save_to_writes_the_document_and_reports_success() {
        let tmp = tempfile::tempdir().unwrap();
        let out_path = tmp.path().join("out.pdf");

        let mut app = app_with_plain_pdf();
        assert!(app.save_to(&out_path));
        assert!(app.error_message.is_none());
        assert!(out_path.exists());
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

    /// `ToolMode::PlaceSignature` must create a real `SignatureElement`
    /// (via `signatures::render_signature_entry`) on an empty-space click,
    /// select it, and revert to `Select` — mirroring
    /// `add_text_box_tool_creates_and_selects_box_on_empty_click`, but for
    /// the one-shot signature-placement path instead of the toggled
    /// text-box tool.
    #[test]
    fn place_signature_tool_creates_and_selects_signature_on_empty_click() {
        let mut app = app_with_plain_pdf();
        let dir = app.signature_dir.clone();
        let entry_id = app
            .signature_library
            .add_typed(&dir, "Jane", "Jane Doe", "default");
        app.tool_mode = ToolMode::PlaceSignature(entry_id);
        let image_rect =
            egui::Rect::from_min_size(egui::pos2(0.0, 0.0), egui::vec2(1000.0, 1000.0));

        app.handle_canvas_click(egui::pos2(50.0, 60.0), image_rect);

        assert_eq!(app.overlay.elements.len(), 1);
        let element = &app.overlay.elements[0];
        assert!(matches!(element, OverlayElement::Signature(_)));
        assert_eq!(app.selected_element, Some(element.id()));
        // One-shot: placing consumes the tool, it doesn't stay active for
        // a second drop.
        assert_eq!(app.tool_mode, ToolMode::Select);
    }

    /// The Review Focus requirement, exercised for a placed signature
    /// through the real click path — mirroring
    /// `text_box_dropped_at_page_edge_is_clamped_to_page` for the
    /// signature-placement path.
    #[test]
    fn placed_signature_at_page_edge_is_clamped_to_page() {
        let mut app = app_with_plain_pdf();
        let dir = app.signature_dir.clone();
        let entry_id = app
            .signature_library
            .add_typed(&dir, "Jane", "Jane Doe", "default");
        app.tool_mode = ToolMode::PlaceSignature(entry_id);
        let page_size = app.document.as_ref().unwrap().page_size_points(0);
        let image_rect =
            egui::Rect::from_min_size(egui::pos2(0.0, 0.0), egui::vec2(2000.0, 2000.0));

        app.handle_canvas_click(egui::pos2(page_size.x - 1.0, page_size.y - 1.0), image_rect);

        let rect = app.overlay.elements[0].rect();
        assert!(rect.min.x >= 0.0 && rect.min.y >= 0.0);
        assert!(rect.max.x <= page_size.x && rect.max.y <= page_size.y);
    }

    /// If the library entry named by `ToolMode::PlaceSignature` no longer
    /// exists (e.g. deleted from the panel between entering the mode and
    /// clicking the page), the click must be a harmless no-op — no
    /// element created — rather than panicking, and the tool still reverts
    /// to `Select` so the user isn't stuck in a dead placement mode.
    #[test]
    fn place_signature_tool_with_unknown_entry_is_a_noop_and_resets_tool_mode() {
        let mut app = app_with_plain_pdf();
        app.tool_mode = ToolMode::PlaceSignature(Uuid::new_v4());
        let image_rect =
            egui::Rect::from_min_size(egui::pos2(0.0, 0.0), egui::vec2(1000.0, 1000.0));

        app.handle_canvas_click(egui::pos2(50.0, 60.0), image_rect);

        assert!(app.overlay.elements.is_empty());
        assert_eq!(app.tool_mode, ToolMode::Select);
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
    /// must not succeed — the opposite edge holds at `MIN_OVERLAY_ELEMENT_SIZE`.
    #[test]
    fn resize_cannot_collapse_below_minimum_size() {
        let rect = egui::Rect::from_min_size(egui::pos2(10.0, 10.0), egui::vec2(50.0, 50.0));
        let resized =
            resize_rect_for_corner(rect, Corner::BottomRight, egui::vec2(-1000.0, -1000.0));
        assert!(resized.width() >= MIN_OVERLAY_ELEMENT_SIZE.x - f32::EPSILON);
        assert!(resized.height() >= MIN_OVERLAY_ELEMENT_SIZE.y - f32::EPSILON);
        assert!(resized.width() > 0.0 && resized.height() > 0.0);
    }

    /// `apply()`'s end-to-end behavior: bakes a filled field and a placed
    /// text box into the document, sets `applied`, clears the now-baked-in
    /// overlay and selection/editing state, and shows the confirmation. A
    /// subsequent save must produce a file with no interactive field left
    /// (the field's value survives only as baked-in page content, not as
    /// something `get_field_value` can read back from an AcroForm widget
    /// that no longer exists).
    #[test]
    fn apply_flattens_locks_and_clears_overlay() {
        let mut app = app_with_form_pdf();
        form_fields::set_field_value(
            app.document.as_mut().unwrap(),
            "applicant_name",
            "Jane Doe",
        )
        .unwrap();
        app.tool_mode = ToolMode::AddTextBox;
        let image_rect =
            egui::Rect::from_min_size(egui::pos2(0.0, 0.0), egui::vec2(1000.0, 1000.0));
        app.handle_canvas_click(egui::pos2(5.0, 5.0), image_rect);
        assert_eq!(app.overlay.elements.len(), 1);
        let text_rect = app.overlay.elements[0].rect();
        if let OverlayElement::TextBox(text_box) = &mut app.overlay.elements[0] {
            text_box.text = "HELLO WORLD".into();
        }
        let ctx = egui::Context::default();
        app.ensure_texture(&ctx);

        app.apply();

        assert!(app.texture_key.is_none(), "stale pre-Apply page texture kept");
        assert!(
            ink_in(app.document.as_ref().unwrap(), 0, text_rect) > 50,
            "text box not baked into the page"
        );
        assert!(app.applied);
        assert!(app.error_message.is_none());
        assert!(app.info_message.is_some());
        assert!(app.overlay.elements.is_empty());
        assert!(app.selected_element.is_none());
        assert!(app.editing_text_box.is_none());
        assert!(app.focused_field.is_none());
        assert_eq!(app.tool_mode, ToolMode::Select);

        let document = app.document.as_ref().unwrap();
        assert!(form_fields::fields_on_page(document, 0).is_empty());
        assert!(pdf_utility::annotations::read_overlay_from_annotations(document, 0).is_empty());
    }

    /// `apply()` with no document open must be a harmless no-op, mirroring
    /// `save_with_no_document_is_a_noop`.
    #[test]
    fn apply_with_no_document_is_a_noop() {
        let mut app = PdfUtilityApp::new(None);
        app.apply();
        assert!(!app.applied);
        assert!(app.error_message.is_none());
    }

    /// The Review Focus requirement: once `applied`, `handle_canvas_click`
    /// must be an unconditional no-op — no field focus, no overlay
    /// selection, and (with the "Add text box" tool still nominally
    /// selected) no new element creation — rather than silently accepting
    /// an edit that can never be baked into the now-flattened document.
    #[test]
    fn canvas_clicks_are_inert_after_apply() {
        let mut app = app_with_form_pdf();
        app.apply();
        assert!(app.applied);

        // Force the tool back on directly (bypassing the now-disabled
        // toolbar button) to prove the lockout is enforced in the click
        // handler itself, not just by the button being greyed out.
        app.tool_mode = ToolMode::AddTextBox;
        let image_rect =
            egui::Rect::from_min_size(egui::pos2(0.0, 0.0), egui::vec2(1000.0, 1000.0));

        app.handle_canvas_click(egui::pos2(5.0, 5.0), image_rect);
        assert!(app.overlay.elements.is_empty());
        assert!(app.selected_element.is_none());

        let field_center = {
            let document = app.document.as_ref().unwrap();
            let fields = form_fields::fields_on_page(document, 0);
            // The field itself is gone post-flatten, so there's nothing to
            // click, but clicking where it used to be must still be inert.
            assert!(fields.is_empty());
            egui::pos2(100.0, 100.0)
        };
        app.handle_canvas_click(field_center, image_rect);
        assert!(app.focused_field.is_none());
    }

    /// A placed signature must be just as locked out as a text box once
    /// `applied` — exercised via `place_signature_from_library` directly,
    /// since the panel's own "Place on page" button lives in
    /// `ui::signature_panel` and is gated there by the `locked` parameter
    /// `app.applied` is threaded into.
    #[test]
    fn placing_a_signature_is_inert_after_apply() {
        let mut app = app_with_plain_pdf();
        let dir = app.signature_dir.clone();
        let entry_id = app
            .signature_library
            .add_typed(&dir, "Jane", "Jane Doe", "default");
        app.apply();
        assert!(app.applied);

        app.tool_mode = ToolMode::PlaceSignature(entry_id);
        let image_rect =
            egui::Rect::from_min_size(egui::pos2(0.0, 0.0), egui::vec2(1000.0, 1000.0));
        app.handle_canvas_click(egui::pos2(50.0, 60.0), image_rect);

        assert!(app.overlay.elements.is_empty());
    }

    /// Saving after Apply must still work (the brief's manual-verification
    /// step) and produce a file with the baked-in field value readable only
    /// as page content, not as a live AcroForm field.
    #[test]
    fn save_after_apply_still_writes_the_flattened_document() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("doc.pdf");
        std::fs::copy("tests/fixtures/sample_form.pdf", &path).unwrap();

        let mut app = PdfUtilityApp::new(Some(path.clone()));
        form_fields::set_field_value(
            app.document.as_mut().unwrap(),
            "applicant_name",
            "Jane Doe",
        )
        .unwrap();
        app.apply();
        assert!(app.applied);

        app.save();

        assert!(app.error_message.is_none());
        let reopened = PdfDocumentState::open(&path).unwrap();
        assert!(form_fields::fields_on_page(&reopened, 0).is_empty());
    }
}

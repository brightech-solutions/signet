//! The signature-library side panel: lists `SignatureLibrary` entries (as
//! a thumbnail and name), and drives adding entries (from an image file via
//! a native file picker, or typed text in a chosen font) and removing them.
//!
//! Every mutation here (`add_image`, `add_typed`, `remove`) is immediately
//! followed by `SignatureLibrary::save`, so the library on disk never lags
//! behind what's shown — the design calls for signatures to "persist across
//! app launches", and the simplest way to guarantee that is to never leave
//! an in-memory-only change sitting around waiting for some other save
//! trigger that might not come (e.g. if the app is quit without an
//! explicit "save" action anywhere else in the UI).
//!
//! The one open item from the design doc this panel resolves is the typed
//! signature's font: it bundles **Shadows Into Light** (Kimberly Geswein,
//! OFL-1.1 — see `assets/fonts/OFL.txt`), a real open-licensed handwriting
//! font, registered as an egui font family named [`SCRIPT_FONT_NAME`] in
//! `src/main.rs` at startup. The dropdown also offers the UI's own default
//! font as a second choice; both are just entries in [`FONT_CHOICES`] and a
//! stored `font_id` string, so adding a third bundled font later is a
//! matter of registering it and adding one more entry here, not a wiring
//! change.

use std::collections::HashMap;
use std::path::Path;

use pdf_utility::signatures::{SignatureLibrary, SignatureSource};
use uuid::Uuid;

/// The `font_id` stored for a typed signature rendered in the bundled
/// script font.
pub const SCRIPT_FONT_ID: &str = "shadows-into-light";
/// The egui font name [`SCRIPT_FONT_ID`] resolves to. Must match the name
/// passed to `egui::Context::add_font` in `src/main.rs`'s
/// `FontInsert::new`, since that's the name `font_family_for` builds an
/// `egui::FontFamily::Name` from.
pub const SCRIPT_FONT_NAME: &str = "ShadowsIntoLight";
/// The `font_id` stored for a typed signature rendered in egui's own
/// default (non-script) font — the fallback choice for anyone who doesn't
/// want a handwriting style.
pub const DEFAULT_FONT_ID: &str = "default";

/// Font choices offered in the "Add typed…" dropdown: a stable `font_id`
/// (persisted verbatim in `SignatureEntry::source`, see
/// `pdf_utility::signatures::SignatureSource::Typed`) paired with the label
/// shown in the dropdown.
pub const FONT_CHOICES: &[(&str, &str)] = &[
    (SCRIPT_FONT_ID, "Shadows Into Light (script)"),
    (DEFAULT_FONT_ID, "Default"),
];

/// Maps a stored `font_id` to the egui font family to render it with.
/// Falls back to the default proportional font for any id other than
/// [`SCRIPT_FONT_ID`] — including [`DEFAULT_FONT_ID`] and any id this build
/// doesn't recognize (e.g. one saved by a future version that bundles a
/// font this build doesn't have), so an unrecognized font never fails to
/// render, it just renders in the ordinary UI font.
pub fn font_family_for(font_id: &str) -> egui::FontFamily {
    if font_id == SCRIPT_FONT_ID {
        egui::FontFamily::Name(SCRIPT_FONT_NAME.into())
    } else {
        egui::FontFamily::Proportional
    }
}

/// Transient UI state for the panel: the in-progress "add typed" form, and
/// a cache of thumbnail textures for image-backed entries (so each one's
/// file is only decoded and uploaded to the GPU once, not every frame).
pub struct SignaturePanelState {
    new_typed_text: String,
    new_typed_font_id: String,
    thumbnails: HashMap<Uuid, egui::TextureHandle>,
    /// Set when `add_image` fails (e.g. an unreadable or undecodable file),
    /// so the panel can show it instead of silently doing nothing.
    error: Option<String>,
}

impl Default for SignaturePanelState {
    fn default() -> Self {
        Self {
            new_typed_text: String::new(),
            new_typed_font_id: SCRIPT_FONT_ID.to_string(),
            thumbnails: HashMap::new(),
            error: None,
        }
    }
}

impl SignaturePanelState {
    pub fn new() -> Self {
        Self::default()
    }

    /// Draws the panel's contents (meant to be called inside an
    /// `egui::Panel`/`SidePanel`'s `show`). `library` is mutated directly
    /// by the add/delete actions below and saved to `dir` immediately
    /// after each one.
    pub fn ui(&mut self, ui: &mut egui::Ui, library: &mut SignatureLibrary, dir: &Path) {
        ui.heading("Signatures");
        ui.separator();

        self.draw_entries(ui, library, dir);

        ui.separator();
        self.draw_add_image(ui, library, dir);

        ui.separator();
        self.draw_add_typed(ui, library, dir);

        if let Some(error) = &self.error {
            ui.separator();
            ui.colored_label(egui::Color32::from_rgb(200, 40, 40), error);
        }
    }

    fn draw_entries(&mut self, ui: &mut egui::Ui, library: &mut SignatureLibrary, dir: &Path) {
        let mut to_remove: Option<Uuid> = None;

        // Snapshot ids/sources up front: `draw_thumbnail` below needs a
        // mutable borrow of `self.thumbnails` while iterating, which can't
        // coexist with an active borrow of `library.entries`.
        let ids: Vec<Uuid> = library.entries.iter().map(|entry| entry.id).collect();

        for id in ids {
            let Some(entry) = library.entries.iter().find(|entry| entry.id == id) else {
                continue;
            };
            let name = entry.name.clone();
            let source = entry.source.clone();

            ui.horizontal(|ui| {
                self.draw_thumbnail(ui, library, dir, id, &source);
                ui.label(&name);
                if ui.small_button("Delete").clicked() {
                    to_remove = Some(id);
                }
            });
        }

        if let Some(id) = to_remove {
            library.remove(id);
            self.thumbnails.remove(&id);
            let _ = library.save(dir);
        }
    }

    /// Draws a single entry's thumbnail: a GPU texture (cached in
    /// `self.thumbnails`, keyed by entry id) for an image-backed entry, or
    /// the typed text itself, rendered live in its chosen font, for a typed
    /// entry.
    fn draw_thumbnail(
        &mut self,
        ui: &mut egui::Ui,
        library: &SignatureLibrary,
        dir: &Path,
        id: Uuid,
        source: &SignatureSource,
    ) {
        match source {
            SignatureSource::Image { .. } => {
                if !self.thumbnails.contains_key(&id)
                    && let Some(path) = library.image_path_for(dir, id)
                    && let Ok(decoded) = image::open(&path)
                {
                    let rgba = decoded.to_rgba8();
                    let size = [rgba.width() as usize, rgba.height() as usize];
                    let color_image = egui::ColorImage::from_rgba_unmultiplied(size, rgba.as_raw());
                    let texture = ui.ctx().load_texture(
                        format!("signature-thumb-{id}"),
                        color_image,
                        egui::TextureOptions::default(),
                    );
                    self.thumbnails.insert(id, texture);
                }

                if let Some(texture) = self.thumbnails.get(&id) {
                    ui.add(egui::Image::new(texture).fit_to_exact_size(egui::vec2(48.0, 32.0)));
                } else {
                    ui.label("(missing)");
                }
            }
            SignatureSource::Typed { text, font_id } => {
                ui.label(
                    egui::RichText::new(text)
                        .font(egui::FontId::new(20.0, font_family_for(font_id))),
                );
            }
        }
    }

    fn draw_add_image(&mut self, ui: &mut egui::Ui, library: &mut SignatureLibrary, dir: &Path) {
        if ui.button("Add from image…").clicked()
            && let Some(path) = rfd::FileDialog::new()
                .add_filter("Images", &["png", "jpg", "jpeg", "bmp"])
                .pick_file()
        {
            let name = path
                .file_stem()
                .map(|stem| stem.to_string_lossy().into_owned())
                .unwrap_or_else(|| "Signature".to_string());

            match library.add_image(dir, &name, &path) {
                Ok(_) => {
                    self.error = None;
                    if let Err(err) = library.save(dir) {
                        self.error = Some(format!("Couldn't save signature library: {err}"));
                    }
                }
                Err(err) => {
                    self.error = Some(format!("Couldn't add signature image: {err}"));
                }
            }
        }
    }

    fn draw_add_typed(&mut self, ui: &mut egui::Ui, library: &mut SignatureLibrary, dir: &Path) {
        ui.label("Add typed signature");
        ui.text_edit_singleline(&mut self.new_typed_text);

        egui::ComboBox::from_label("Font")
            .selected_text(font_label(&self.new_typed_font_id))
            .show_ui(ui, |ui| {
                for (font_id, label) in FONT_CHOICES {
                    ui.selectable_value(&mut self.new_typed_font_id, font_id.to_string(), *label);
                }
            });

        if !self.new_typed_text.trim().is_empty() {
            ui.label(
                egui::RichText::new(&self.new_typed_text).font(egui::FontId::new(
                    28.0,
                    font_family_for(&self.new_typed_font_id),
                )),
            );
        }

        let can_add = !self.new_typed_text.trim().is_empty();
        if ui
            .add_enabled(can_add, egui::Button::new("Add typed…"))
            .clicked()
        {
            library.add_typed(
                dir,
                self.new_typed_text.trim(),
                self.new_typed_text.trim(),
                &self.new_typed_font_id,
            );
            if let Err(err) = library.save(dir) {
                self.error = Some(format!("Couldn't save signature library: {err}"));
            } else {
                self.error = None;
            }
            self.new_typed_text.clear();
        }
    }
}

/// The dropdown label for a `font_id`, falling back to the id itself for an
/// unrecognized one (shouldn't normally happen, since the dropdown only
/// ever sets `new_typed_font_id` to a value from [`FONT_CHOICES`]).
fn font_label(font_id: &str) -> &str {
    FONT_CHOICES
        .iter()
        .find(|(id, _)| *id == font_id)
        .map(|(_, label)| *label)
        .unwrap_or(font_id)
}

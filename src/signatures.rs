//! Personal signature library: a small JSON manifest plus, for image-backed
//! entries, the processed image files it points to — both living in a
//! single directory on disk (resolved once, in `src/app.rs`, to
//! `~/Library/Application Support/<bundle id>/signatures/`).
//!
//! An entry is either [`SignatureSource::Image`] (an uploaded photo/scan,
//! background-stripped via [`crate::signature_image::strip_light_background`]
//! and saved as its own PNG under the library directory) or
//! [`SignatureSource::Typed`] (just a name and a chosen font; rendering to
//! an image happens at placement time, in Task 8 — nothing is written to
//! disk for these). [`SignatureLibrary::load`]/[`SignatureLibrary::save`]
//! round-trip the manifest (`<dir>/signatures.json`) as plain JSON, so the
//! whole library survives an app restart with no database involved.

use std::fmt;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use ab_glyph::{Font, FontRef, ScaleFont};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::signature_image::strip_light_background;

/// The manifest file's name within the library directory.
const MANIFEST_FILE_NAME: &str = "signatures.json";

/// Pixels with all three color channels at or above this value are treated
/// as background by [`strip_light_background`] when an image is uploaded.
/// See that function's doc comment for why a flat per-channel threshold is
/// sufficient for typical signature photos.
const BACKGROUND_THRESHOLD: u8 = 240;

/// The `font_id` stored for a typed signature rendered in the bundled
/// handwriting font (see [`SCRIPT_FONT_BYTES`]). This is the single source
/// of truth for the id — `crate::ui::signature_panel` (which drives the
/// live egui preview, in a font family rather than raw bytes) matches
/// against this same constant rather than its own copy of the string, so
/// the two layers can't drift apart.
pub const SCRIPT_FONT_ID: &str = "shadows-into-light";
/// The `font_id` stored for a typed signature rendered in a plain
/// (non-script) font — the fallback choice, and also what any
/// unrecognized `font_id` (e.g. one saved by a future version) renders as.
pub const DEFAULT_FONT_ID: &str = "default";

/// The bundled handwriting font ("Shadows Into Light" by Kimberly Geswein,
/// SIL Open Font License 1.1 — see `assets/fonts/OFL.txt`) used to
/// rasterize a [`SCRIPT_FONT_ID`] typed signature in [`render_signature_entry`].
const SCRIPT_FONT_BYTES: &[u8] = include_bytes!("../assets/fonts/ShadowsIntoLight.ttf");

/// Pixel height a typed signature's text is rasterized at before being
/// placed on a page. The placed element is further scaled to whatever rect
/// the user resizes it to (same as an image-backed signature), so this
/// only needs to be large enough to rasterize cleanly, not any particular
/// final size.
const TYPED_SIGNATURE_FONT_PX: f32 = 64.0;
/// Transparent margin (pixels), on every side, left around the rendered
/// glyphs' ink. Without this, a resize handle anchored exactly on a
/// stroke's edge would feel like it's grabbing the ink itself rather than
/// a small frame around it.
const TYPED_SIGNATURE_PADDING_PX: f32 = 8.0;

/// Where a signature's visual content comes from.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum SignatureSource {
    /// An uploaded image, already background-stripped, saved as
    /// `<dir>/<file_name>`.
    Image { file_name: String },
    /// Text to be rendered in a chosen font at placement time. `font_id`
    /// identifies the font (resolved to an actual `egui::FontFamily` by
    /// `crate::ui::signature_panel`); it is stored verbatim and never
    /// interpreted here.
    Typed { text: String, font_id: String },
}

/// One saved signature: either an uploaded image or a typed name, under a
/// user-chosen display `name`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SignatureEntry {
    pub id: Uuid,
    pub name: String,
    pub source: SignatureSource,
    /// When the entry was added, as a Unix timestamp (seconds since the
    /// epoch) rendered as a decimal string. Not parsed back by anything in
    /// this crate; it exists purely for display (e.g. sorting/labeling in
    /// `signature_panel`) and so is kept as a plain string rather than a
    /// typed timestamp, avoiding a dependency on a date/time crate for a
    /// value nothing here needs to do arithmetic on.
    pub created: String,
}

/// The full set of saved signatures for the current user.
#[derive(Debug, Default, Serialize, Deserialize)]
pub struct SignatureLibrary {
    pub entries: Vec<SignatureEntry>,
}

/// Errors from [`SignatureLibrary::add_image`]: either the source image
/// couldn't be read/decoded, or the processed result couldn't be written to
/// the library directory.
#[derive(Debug)]
pub enum SignatureError {
    Io(std::io::Error),
    Image(image::ImageError),
}

impl fmt::Display for SignatureError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            SignatureError::Io(err) => write!(f, "I/O error: {err}"),
            SignatureError::Image(err) => write!(f, "image error: {err}"),
        }
    }
}

impl std::error::Error for SignatureError {}

impl From<std::io::Error> for SignatureError {
    fn from(err: std::io::Error) -> Self {
        SignatureError::Io(err)
    }
}

impl From<image::ImageError> for SignatureError {
    fn from(err: image::ImageError) -> Self {
        SignatureError::Image(err)
    }
}

impl SignatureLibrary {
    /// Loads the library's manifest from `<dir>/signatures.json`. Returns an
    /// empty library — never an error — if the directory or manifest
    /// doesn't exist yet (first run) or the manifest can't be parsed (so a
    /// corrupt manifest doesn't prevent the app from starting; it just
    /// starts the user over with an empty library rather than crashing).
    pub fn load(dir: &Path) -> SignatureLibrary {
        std::fs::read_to_string(dir.join(MANIFEST_FILE_NAME))
            .ok()
            .and_then(|contents| serde_json::from_str(&contents).ok())
            .unwrap_or_default()
    }

    /// Writes the manifest to `<dir>/signatures.json`, creating `dir` (and
    /// any missing parents) first if needed.
    pub fn save(&self, dir: &Path) -> std::io::Result<()> {
        std::fs::create_dir_all(dir)?;
        let json = serde_json::to_string_pretty(self).expect("SignatureLibrary always serializes");
        std::fs::write(dir.join(MANIFEST_FILE_NAME), json)
    }

    /// Reads the image at `source_image_path`, strips its light/white
    /// background (see [`strip_light_background`]), and saves the result
    /// as a new `<dir>/<id>.png`, then appends a [`SignatureEntry`] for it.
    /// `dir` is created first if it doesn't exist yet. Returns the new
    /// entry's id.
    pub fn add_image(
        &mut self,
        dir: &Path,
        name: &str,
        source_image_path: &Path,
    ) -> Result<Uuid, SignatureError> {
        let image = image::open(source_image_path)?.to_rgba8();
        let stripped = strip_light_background(image, BACKGROUND_THRESHOLD);

        std::fs::create_dir_all(dir)?;
        let id = Uuid::new_v4();
        let file_name = format!("{id}.png");
        stripped.save(dir.join(&file_name))?;

        self.entries.push(SignatureEntry {
            id,
            name: name.to_string(),
            source: SignatureSource::Image { file_name },
            created: unix_timestamp_now(),
        });

        Ok(id)
    }

    /// Appends a typed-signature entry. No file is written — a typed
    /// signature's text and font are all that's needed to render it (at
    /// placement time, in Task 8). `dir` is accepted for symmetry with
    /// [`Self::add_image`] (and so a future caller can rely on either
    /// method's directory argument, without needing to know which variant
    /// writes files) but is otherwise unused. Returns the new entry's id.
    pub fn add_typed(&mut self, _dir: &Path, name: &str, text: &str, font_id: &str) -> Uuid {
        let id = Uuid::new_v4();
        self.entries.push(SignatureEntry {
            id,
            name: name.to_string(),
            source: SignatureSource::Typed {
                text: text.to_string(),
                font_id: font_id.to_string(),
            },
            created: unix_timestamp_now(),
        });
        id
    }

    /// Removes the entry with the given `id`, if present. A no-op if no
    /// entry with that id exists. Does not delete an image-backed entry's
    /// file on disk — the manifest is the source of truth for what's "in"
    /// the library, and leaving an orphaned file behind is harmless; wiring
    /// up file cleanup is left to whichever caller cares (none does yet).
    pub fn remove(&mut self, id: Uuid) {
        self.entries.retain(|entry| entry.id != id);
    }

    /// Resolves an [`SignatureSource::Image`] entry's file to a full path
    /// under `dir`. Returns `None` if no entry with `id` exists, or if it
    /// exists but is a [`SignatureSource::Typed`] entry (which has no file).
    pub fn image_path_for(&self, dir: &Path, id: Uuid) -> Option<PathBuf> {
        let entry = self.entries.iter().find(|entry| entry.id == id)?;
        match &entry.source {
            SignatureSource::Image { file_name } => Some(dir.join(file_name)),
            SignatureSource::Typed { .. } => None,
        }
    }
}

/// The current time as a Unix timestamp (seconds since the epoch),
/// rendered as a decimal string. Falls back to `"0"` in the (practically
/// impossible) case the system clock reads before the epoch, rather than
/// panicking over a cosmetic timestamp.
fn unix_timestamp_now() -> String {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_secs().to_string())
        .unwrap_or_else(|_| "0".to_string())
}

/// Renders `entry`'s visual content as an `egui::ColorImage`, ready to be
/// placed on a page as a [`crate::overlay::SignatureElement`]'s `image`.
///
/// - [`SignatureSource::Image`]: loads and decodes the PNG `add_image`
///   already wrote (background-stripped) at `<dir>/<file_name>`. Returns a
///   single fully-transparent pixel — rather than panicking — if the file
///   is missing or unreadable (e.g. deleted out from under the manifest
///   between sessions); the caller still gets *something* placeable.
/// - [`SignatureSource::Typed`]: rasterizes `text` at a fixed pixel size
///   via `ab_glyph`, in the bundled handwriting font for [`SCRIPT_FONT_ID`]
///   or egui's own default font for any other `font_id` (matching
///   `crate::ui::signature_panel::font_family_for`'s same fallback for the
///   live preview). Returns a single fully-transparent pixel for empty
///   text or a font that fails to parse, rather than panicking.
pub fn render_signature_entry(entry: &SignatureEntry, dir: &Path) -> egui::ColorImage {
    match &entry.source {
        SignatureSource::Image { file_name } => image::open(dir.join(file_name))
            .map(|decoded| color_image_from_rgba(&decoded.to_rgba8()))
            .unwrap_or_else(|_| blank_color_image()),
        SignatureSource::Typed { text, font_id } => render_typed_text(text, font_id),
    }
}

/// A single fully-transparent pixel, used as the fallback result when
/// there's nothing real to render (a missing image file, empty typed
/// text, or an unparseable font) — small and harmless to place, rather
/// than failing the whole placement.
fn blank_color_image() -> egui::ColorImage {
    egui::ColorImage::filled([1, 1], egui::Color32::TRANSPARENT)
}

/// Converts a decoded RGBA image into an `egui::ColorImage`, the same
/// conversion `crate::ui::signature_panel` uses for thumbnails.
fn color_image_from_rgba(rgba: &image::RgbaImage) -> egui::ColorImage {
    let size = [rgba.width() as usize, rgba.height() as usize];
    egui::ColorImage::from_rgba_unmultiplied(size, rgba.as_raw())
}

/// Rasterizes `text` into an `egui::ColorImage`: fully opaque black ink on
/// a transparent background, tightly cropped to the glyphs' combined
/// pixel bounds plus [`TYPED_SIGNATURE_PADDING_PX`] on every side.
///
/// Glyphs are laid out left-to-right along a single baseline using each
/// glyph's horizontal advance, with no kerning — signatures are short
/// strings, and kerning's visual effect is negligible at this scale,
/// especially in a handwriting-style font designed to already look
/// natural letter-to-letter.
fn render_typed_text(text: &str, font_id: &str) -> egui::ColorImage {
    let font_bytes: &[u8] = if font_id == SCRIPT_FONT_ID {
        SCRIPT_FONT_BYTES
    } else {
        // `epaint_default_fonts` is already compiled into the app (egui's
        // own default UI font comes from it); depending on it directly
        // here reuses those exact bytes instead of bundling a second font
        // asset just for this fallback path.
        epaint_default_fonts::UBUNTU_LIGHT
    };

    let Ok(font) = FontRef::try_from_slice(font_bytes) else {
        return blank_color_image();
    };
    let scaled = font.into_scaled(TYPED_SIGNATURE_FONT_PX);

    let mut cursor_x = 0.0_f32;
    let outlined_glyphs: Vec<_> = text
        .chars()
        .filter_map(|ch| {
            let glyph_id = scaled.glyph_id(ch);
            let glyph = glyph_id.with_scale_and_position(
                TYPED_SIGNATURE_FONT_PX,
                ab_glyph::point(cursor_x, scaled.ascent()),
            );
            cursor_x += scaled.h_advance(glyph_id);
            scaled.outline_glyph(glyph)
        })
        .collect();

    if outlined_glyphs.is_empty() {
        return blank_color_image();
    }

    let mut min_x = f32::INFINITY;
    let mut min_y = f32::INFINITY;
    let mut max_x = f32::NEG_INFINITY;
    let mut max_y = f32::NEG_INFINITY;
    for glyph in &outlined_glyphs {
        let bounds = glyph.px_bounds();
        min_x = min_x.min(bounds.min.x);
        min_y = min_y.min(bounds.min.y);
        max_x = max_x.max(bounds.max.x);
        max_y = max_y.max(bounds.max.y);
    }

    let padding = TYPED_SIGNATURE_PADDING_PX;
    let width = ((max_x - min_x).ceil() + 2.0 * padding).max(1.0) as usize;
    let height = ((max_y - min_y).ceil() + 2.0 * padding).max(1.0) as usize;

    let mut image = egui::ColorImage::filled([width, height], egui::Color32::TRANSPARENT);

    for glyph in &outlined_glyphs {
        let bounds = glyph.px_bounds();
        let offset_x = bounds.min.x - min_x + padding;
        let offset_y = bounds.min.y - min_y + padding;

        glyph.draw(|x, y, coverage| {
            if coverage <= 0.0 {
                return;
            }
            let Some(px) = checked_pixel_index(offset_x, offset_y, x, y, width, height) else {
                return;
            };
            let alpha = (coverage.clamp(0.0, 1.0) * 255.0).round() as u8;
            // Glyphs can overlap slightly at the pixel level (antialiased
            // edges of adjacent letters); take the stronger coverage
            // rather than overwrite, so one glyph's edge never visibly
            // cuts into a neighbor's.
            let existing_alpha = image.pixels[px].a();
            image.pixels[px] =
                egui::Color32::from_rgba_unmultiplied(0, 0, 0, alpha.max(existing_alpha));
        });
    }

    image
}

/// Maps a glyph-local `(x, y)` pixel from `OutlinedGlyph::draw`, offset by
/// `(offset_x, offset_y)`, to a flat index into a `width * height` pixel
/// buffer. Returns `None` if the result falls outside the buffer, which
/// can happen for a glyph whose outline extends slightly beyond its own
/// `px_bounds` due to rounding — silently dropping that pixel is
/// preferable to panicking over a sub-pixel rendering artifact.
fn checked_pixel_index(
    offset_x: f32,
    offset_y: f32,
    x: u32,
    y: u32,
    width: usize,
    height: usize,
) -> Option<usize> {
    let px = (offset_x + x as f32).round() as i64;
    let py = (offset_y + y as f32).round() as i64;
    if px < 0 || py < 0 {
        return None;
    }
    let (px, py) = (px as usize, py as usize);
    if px >= width || py >= height {
        return None;
    }
    Some(py * width + px)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `render_typed_text` (exercised here directly rather than through
    /// `render_signature_entry`, which `tests/signatures_test.rs` already
    /// covers end-to-end) must actually draw ink, not just return an
    /// empty/placeholder image, for ordinary text in either bundled font.
    #[test]
    fn render_typed_text_draws_real_ink_for_known_fonts() {
        for font_id in [SCRIPT_FONT_ID, DEFAULT_FONT_ID] {
            let image = render_typed_text("Hi", font_id);
            assert!(image.size[0] > 1 && image.size[1] > 1);
            assert!(image.pixels.iter().any(|pixel| pixel.a() > 0));
        }
    }

    /// Empty text has no glyphs to outline, so this must degrade to the
    /// blank fallback rather than panicking on an empty bounds reduction.
    #[test]
    fn render_typed_text_on_empty_string_returns_blank_fallback() {
        let image = render_typed_text("", SCRIPT_FONT_ID);
        assert_eq!(image.size, [1, 1]);
        assert_eq!(image.pixels[0], egui::Color32::TRANSPARENT);
    }
}

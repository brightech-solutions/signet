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

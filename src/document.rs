//! Document core: opening PDFs and rendering their pages via pdfium.
//!
//! All PDF access in the application goes through [`PdfDocumentState`]. It
//! owns a loaded [`PdfDocument`] and exposes just the operations later tasks
//! need: page count, rasterized page images, and page dimensions.
//!
//! The underlying `pdfium-render` crate keeps a single process-wide set of
//! library bindings (it cannot be re-bound once initialized), so this module
//! lazily initializes one global [`Pdfium`] instance the first time it is
//! needed and reuses it for every document opened afterwards.

use std::fmt;
use std::path::{Path, PathBuf};

use once_cell::sync::OnceCell;
use pdfium_render::prelude::*;

/// Errors that can occur while opening or rendering a PDF document.
#[derive(Debug)]
pub enum DocumentError {
    /// The file could not be read from disk (not found, permissions, etc).
    Io(std::io::Error),
    /// The file is not a well-formed PDF, or pdfium otherwise failed to
    /// parse it.
    Corrupt,
    /// The PDF requires a password to open. This application does not
    /// support password-protected PDFs; the caller should report this
    /// clearly rather than treating it as generic corruption.
    PasswordProtected,
    /// Any other pdfium failure not covered by the variants above (for
    /// example, a failure to load the vendored pdfium library itself).
    Pdfium(String),
}

impl fmt::Display for DocumentError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            DocumentError::Io(err) => write!(f, "could not read file: {err}"),
            DocumentError::Corrupt => write!(f, "file is not a valid PDF"),
            DocumentError::PasswordProtected => write!(f, "PDF is password-protected"),
            DocumentError::Pdfium(message) => write!(f, "pdfium error: {message}"),
        }
    }
}

impl std::error::Error for DocumentError {}

/// The process-wide pdfium library bindings, initialized lazily on first use.
static PDFIUM: OnceCell<Pdfium> = OnceCell::new();

/// Resolves the path to the vendored pdfium dynamic library for the host
/// architecture, matching the layout vendored in Task 1
/// (`vendor/pdfium/<arch>/libpdfium.dylib`).
fn vendored_library_path() -> PathBuf {
    let arch_dir = if cfg!(target_arch = "aarch64") {
        "macos-arm64"
    } else {
        "macos-x86_64"
    };

    Path::new("vendor/pdfium")
        .join(arch_dir)
        .join("libpdfium.dylib")
}

/// Returns the shared [`Pdfium`] instance, binding to the vendored library
/// and initializing it on first use.
fn pdfium_instance() -> Result<&'static Pdfium, DocumentError> {
    PDFIUM.get_or_try_init(|| {
        let bindings = Pdfium::bind_to_library(vendored_library_path())
            .map_err(|err| DocumentError::Pdfium(err.to_string()))?;

        Ok(Pdfium::new(bindings))
    })
}

/// Initializes the pdfium library eagerly. Called from `main` so that a
/// missing or broken vendored library is reported at startup rather than
/// the first time a document is opened.
pub fn init() -> Result<(), DocumentError> {
    pdfium_instance().map(|_| ())
}

/// Maps a pdfium document-load failure to a [`DocumentError`], distinguishing
/// password-protected documents from all other load failures.
fn map_load_error(err: PdfiumError) -> DocumentError {
    match err {
        PdfiumError::IoError(io_err) => DocumentError::Io(io_err),
        PdfiumError::PdfiumLibraryInternalError(PdfiumInternalError::PasswordError) => {
            DocumentError::PasswordProtected
        }
        _ => DocumentError::Corrupt,
    }
}

/// A loaded PDF document, backed by pdfium.
pub struct PdfDocumentState {
    document: PdfDocument<'static>,
}

impl fmt::Debug for PdfDocumentState {
    // `PdfDocument` itself has no `Debug` impl, so this reports just enough
    // to be useful (e.g. in `Result::unwrap_err` panic messages) without
    // depending on pdfium internals.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("PdfDocumentState")
            .field("page_count", &self.page_count())
            .finish()
    }
}

impl PdfDocumentState {
    /// Opens the PDF at `path`.
    ///
    /// Returns [`DocumentError::PasswordProtected`] if the document requires
    /// a password, or [`DocumentError::Corrupt`] if it cannot otherwise be
    /// parsed as a PDF.
    pub fn open(path: &Path) -> Result<Self, DocumentError> {
        let pdfium = pdfium_instance()?;

        let document = pdfium
            .load_pdf_from_file(path, None)
            .map_err(map_load_error)?;

        Ok(Self { document })
    }

    /// Returns the number of pages in the document.
    pub fn page_count(&self) -> usize {
        self.document.pages().len() as usize
    }

    /// Renders the page at `index` (0-based) into an RGBA image, scaled by
    /// `zoom` (1.0 = 72 DPI, i.e. one pixel per PDF point).
    pub fn render_page(&self, index: usize, zoom: f32) -> Result<egui::ColorImage, DocumentError> {
        let page = self
            .document
            .pages()
            .get(index as PdfPageIndex)
            .map_err(|err| DocumentError::Pdfium(err.to_string()))?;

        let config = PdfRenderConfig::new().scale_page_by_factor(zoom);

        let bitmap = page
            .render_with_config(&config)
            .map_err(|err| DocumentError::Pdfium(err.to_string()))?;

        let width = bitmap.width() as usize;
        let height = bitmap.height() as usize;
        let rgba = bitmap.as_rgba_bytes();

        Ok(egui::ColorImage::from_rgba_unmultiplied(
            [width, height],
            &rgba,
        ))
    }

    /// Returns the size of the page at `index`, in PDF points (1/72 inch).
    /// Returns `egui::Vec2::ZERO` if `index` is out of bounds.
    pub fn page_size_points(&self, index: usize) -> egui::Vec2 {
        match self.document.pages().get(index as PdfPageIndex) {
            Ok(page) => egui::vec2(page.width().value, page.height().value),
            Err(_) => egui::Vec2::ZERO,
        }
    }
}

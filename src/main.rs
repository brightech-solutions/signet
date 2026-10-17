mod app;
mod ui;

use std::path::PathBuf;

use app::PdfUtilityApp;
use pdf_utility::document;

fn main() -> eframe::Result<()> {
    if let Err(err) = document::init() {
        eprintln!("Failed to initialize pdfium: {err}");
    }

    // Finder "Open With" / double-click launches pass the file path as the
    // first argument; `cargo run -- <path>` exercises the same path.
    let initial_path: Option<PathBuf> = std::env::args().nth(1).map(PathBuf::from);

    eframe::run_native(
        "PDF Utility",
        eframe::NativeOptions::default(),
        Box::new(|_cc| Ok(Box::new(PdfUtilityApp::new(initial_path)))),
    )
}

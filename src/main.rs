mod app;

use app::PdfUtilityApp;
use pdf_utility::document;

fn main() -> eframe::Result<()> {
    if let Err(err) = document::init() {
        eprintln!("Failed to initialize pdfium: {err}");
    }

    eframe::run_native(
        "PDF Utility",
        eframe::NativeOptions::default(),
        Box::new(|_cc| Ok(Box::new(PdfUtilityApp))),
    )
}

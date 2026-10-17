mod app;

use app::PdfUtilityApp;

fn main() -> eframe::Result<()> {
    eframe::run_native(
        "PDF Utility",
        eframe::NativeOptions::default(),
        Box::new(|_cc| Ok(Box::new(PdfUtilityApp))),
    )
}

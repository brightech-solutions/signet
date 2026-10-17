mod app;
mod ui;

use std::path::PathBuf;

use app::PdfUtilityApp;
use pdf_utility::document;
use ui::signature_panel::SCRIPT_FONT_NAME;

/// The bundled handwriting font for typed signatures: "Shadows Into Light"
/// by Kimberly Geswein, SIL Open Font License 1.1 (see
/// `assets/fonts/OFL.txt`). Registered once below, at startup, under the
/// egui font family name [`SCRIPT_FONT_NAME`] — the same name
/// `ui::signature_panel::font_family_for` builds an
/// `egui::FontFamily::Name` from when rendering a typed signature whose
/// `font_id` is `ui::signature_panel::SCRIPT_FONT_ID`.
const SCRIPT_FONT_BYTES: &[u8] = include_bytes!("../assets/fonts/ShadowsIntoLight.ttf");

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
        Box::new(|cc| {
            cc.egui_ctx.add_font(egui::FontInsert::new(
                SCRIPT_FONT_NAME,
                egui::FontData::from_static(SCRIPT_FONT_BYTES),
                vec![egui::InsertFontFamily {
                    family: egui::FontFamily::Name(SCRIPT_FONT_NAME.into()),
                    priority: egui::FontPriority::Highest,
                }],
            ));

            Ok(Box::new(PdfUtilityApp::new(initial_path)))
        }),
    )
}

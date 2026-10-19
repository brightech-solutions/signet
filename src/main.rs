mod app;
#[cfg(target_os = "macos")]
mod macos_open;
mod ui;

use std::path::PathBuf;

use app::SignetApp;
use signet::document;
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

    // A direct CLI launch (`cargo run -- <path>`) passes the file path as
    // the first argument. A Finder-initiated open (double-click, "Open
    // With") never does — see `macos_open`'s module docs — so on macOS,
    // `SignetApp::new` also checks that module's pending-opens queue.
    let initial_path: Option<PathBuf> = std::env::args().nth(1).map(PathBuf::from);

    // Building the event loop manually (rather than via `eframe::run_native`)
    // gives us the one window `macos_open::install` needs: after the event
    // loop exists, but before it starts running.
    let event_loop = winit::event_loop::EventLoop::<eframe::UserEvent>::with_user_event().build()?;

    #[cfg(target_os = "macos")]
    macos_open::install();

    let mut winit_app = eframe::create_native(
        "Signet",
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

            #[cfg(target_os = "macos")]
            macos_open::set_repaint_context(cc.egui_ctx.clone());

            Ok(Box::new(SignetApp::new(initial_path)))
        }),
        &event_loop,
    );

    event_loop.run_app(&mut winit_app)?;
    Ok(())
}

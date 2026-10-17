//! The root `eframe::App` for PDF Utility.
//!
//! This is currently an empty placeholder: it renders a single central panel
//! with a label. PDF document state and UI panels are added in later tasks.

/// Root application state.
///
/// Empty for now; document state, UI panels, and signature library state
/// will be added as fields in later tasks.
pub struct PdfUtilityApp;

impl eframe::App for PdfUtilityApp {
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        egui::CentralPanel::default().show(ui, |ui| {
            ui.label("PDF Utility");
        });
    }
}

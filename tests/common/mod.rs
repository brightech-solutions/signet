//! Shared outcome-level helpers for integration tests: instead of checking
//! proxies ("the annotation object exists", "save returned Ok"), these
//! measure what actually ended up on the rendered page.

#![allow(dead_code)] // each test crate uses a different subset

use pdf_utility::document::PdfDocumentState;

/// Counts "ink" pixels — opaque and darker than mid-grey — inside
/// `page_rect` (page space: PDF points, origin top-left) of an image
/// rendered at `zoom`. A blank white page region counts 0.
pub fn ink_pixels(image: &egui::ColorImage, page_rect: egui::Rect, zoom: f32) -> usize {
    let [width, height] = image.size;
    let x0 = (page_rect.min.x * zoom).floor().max(0.0) as usize;
    let y0 = (page_rect.min.y * zoom).floor().max(0.0) as usize;
    let x1 = ((page_rect.max.x * zoom).ceil() as usize).min(width);
    let y1 = ((page_rect.max.y * zoom).ceil() as usize).min(height);

    let mut count = 0;
    for y in y0..y1 {
        for x in x0..x1 {
            let pixel = image.pixels[y * width + x];
            let sum = pixel.r() as u32 + pixel.g() as u32 + pixel.b() as u32;
            if pixel.a() > 128 && sum < 384 {
                count += 1;
            }
        }
    }
    count
}

/// Renders `page_index` of `doc` at zoom 1.0 and counts ink pixels in
/// `page_rect`. Panics (failing the test) if the page can't be rendered.
pub fn ink_in(doc: &PdfDocumentState, page_index: usize, page_rect: egui::Rect) -> usize {
    let image = doc
        .render_page(page_index, 1.0)
        .unwrap_or_else(|err| panic!("page {page_index} failed to render: {err}"));
    ink_pixels(&image, page_rect, 1.0)
}

/// Counts the annotations in the saved file at `path` on page `page_number`
/// (1-based) whose `/T` is the app's marker — read straight from the file's
/// bytes with lopdf, independently of pdfium, so it counts what's really
/// in the file's `/Annots` array.
pub fn app_annotation_count_in_file(path: &std::path::Path, page_number: u32) -> usize {
    let doc = lopdf::Document::load(path).expect("saved file parses");
    let page_id = *doc.get_pages().get(&page_number).expect("page exists");
    let page = doc.get_dictionary(page_id).unwrap();
    let Ok(annots) = page.get(b"Annots") else {
        return 0;
    };
    let (_, annots) = doc.dereference(annots).unwrap();
    annots
        .as_array()
        .unwrap()
        .iter()
        .filter(|entry| {
            let dict = match entry {
                lopdf::Object::Reference(id) => doc.get_dictionary(*id).ok(),
                other => other.as_dict().ok(),
            };
            dict.and_then(|dict| dict.get(b"T").ok())
                .and_then(|title| lopdf::decode_text_string(title).ok())
                .as_deref()
                == Some(pdf_utility::annotations::APP_MARKER)
        })
        .count()
}

/// The solid 40x40pt marker square drawn on page `page_number` (1-based) of
/// `tests/fixtures/sample_multipage.pdf`, in page space.
pub fn multipage_marker_rect(page_number: usize) -> egui::Rect {
    let x = 40.0 + 40.0 * page_number as f32;
    // PDF-space y range 560..600 on a 792pt-tall page.
    egui::Rect::from_min_max(egui::pos2(x, 792.0 - 600.0), egui::pos2(x + 40.0, 792.0 - 560.0))
}

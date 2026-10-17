//! Round-trips overlay elements through real PDF annotation objects.
//!
//! An un-applied overlay element (a text box or placed signature) must
//! survive a save as a normal, portable PDF annotation — never a custom
//! file format or sidecar metadata — so that the file stays openable in any
//! PDF viewer even before the user "applies" (flattens) their edits.
//! [`write_overlay_to_annotations`] writes each [`OverlayElement`] as a
//! `FreeText` annotation (text boxes) or a stamp annotation with an embedded
//! image (signatures), tagging each one's `/T` (title/author) field with
//! [`APP_MARKER`] so this app — and only this app — can tell its own
//! annotations apart from anything already in the PDF.
//! [`read_overlay_from_annotations`] is the inverse: it reconstructs
//! `OverlayElement`s from annotations carrying that marker, and silently
//! skips every other annotation (wrong subtype, wrong/missing title, or
//! both), so a pre-existing annotation in a foreign PDF is never
//! misinterpreted as one of ours.

use image::{DynamicImage, RgbaImage};
use pdfium_render::prelude::*;
use uuid::Uuid;

use crate::document::{DocumentError, PdfDocumentState};
use crate::overlay::{OverlayElement, SignatureElement, TextBoxElement};

/// The value written to every annotation this app creates' `/T` (title /
/// author) field, and the only value [`read_overlay_from_annotations`]
/// treats as "one of ours".
pub const APP_MARKER: &str = "BrightechPDFUtility";

/// Writes `elements` onto page `page_index` of `doc` as real PDF annotation
/// objects: a `FreeText` annotation for each [`OverlayElement::TextBox`], a
/// stamp annotation containing an embedded image for each
/// [`OverlayElement::Signature`]. Every annotation written has its `/T`
/// field set to [`APP_MARKER`].
pub fn write_overlay_to_annotations(
    doc: &mut PdfDocumentState,
    page_index: usize,
    elements: &[OverlayElement],
) -> Result<(), DocumentError> {
    let document = doc.inner_mut();

    let mut page = document
        .pages()
        .get(page_index as PdfPageIndex)
        .map_err(|err| DocumentError::Pdfium(err.to_string()))?;

    let page_height = page.height().value;

    for element in elements {
        match element {
            OverlayElement::TextBox(text_box) => {
                write_text_box_annotation(&mut page, text_box, page_height)?;
            }
            OverlayElement::Signature(signature) => {
                write_signature_annotation(document, &mut page, signature, page_height)?;
            }
        }
    }

    Ok(())
}

fn write_text_box_annotation(
    page: &mut PdfPage<'static>,
    text_box: &TextBoxElement,
    page_height: f32,
) -> Result<(), DocumentError> {
    let bounds = egui_rect_to_pdf_rect(text_box.rect, page_height);

    let mut annotation = page
        .annotations_mut()
        .create_free_text_annotation(&text_box.text)
        .map_err(|err| DocumentError::Pdfium(err.to_string()))?;

    annotation
        .set_bounds(bounds)
        .map_err(|err| DocumentError::Pdfium(err.to_string()))?;
    annotation
        .set_creator(APP_MARKER)
        .map_err(|err| DocumentError::Pdfium(err.to_string()))?;

    Ok(())
}

fn write_signature_annotation(
    document: &PdfDocument<'static>,
    page: &mut PdfPage<'static>,
    signature: &SignatureElement,
    page_height: f32,
) -> Result<(), DocumentError> {
    let bounds = egui_rect_to_pdf_rect(signature.rect, page_height);
    let dynamic_image = color_image_to_dynamic_image(&signature.image);

    let mut image_object = PdfPageImageObject::new_with_size(
        document,
        &dynamic_image,
        bounds.width(),
        bounds.height(),
    )
    .map_err(|err| DocumentError::Pdfium(err.to_string()))?;

    image_object
        .translate(bounds.left(), bounds.bottom())
        .map_err(|err| DocumentError::Pdfium(err.to_string()))?;

    let mut annotation = page
        .annotations_mut()
        .create_stamp_annotation()
        .map_err(|err| DocumentError::Pdfium(err.to_string()))?;

    annotation
        .set_bounds(bounds)
        .map_err(|err| DocumentError::Pdfium(err.to_string()))?;
    annotation
        .set_creator(APP_MARKER)
        .map_err(|err| DocumentError::Pdfium(err.to_string()))?;
    annotation
        .objects_mut()
        .add_image_object(image_object)
        .map_err(|err| DocumentError::Pdfium(err.to_string()))?;

    Ok(())
}

/// Reconstructs the [`OverlayElement`]s previously written to page
/// `page_index` of `doc` by [`write_overlay_to_annotations`].
///
/// Only annotations whose `/T` field equals [`APP_MARKER`] are considered;
/// everything else (a pre-existing annotation in the PDF, or one of a type
/// this app doesn't write) is skipped. Returns an empty `Vec` if the page
/// index is out of bounds. Each reconstructed element is assigned a fresh
/// [`Uuid`] — identity is not expected to survive the round trip, only
/// content and position.
pub fn read_overlay_from_annotations(
    doc: &PdfDocumentState,
    page_index: usize,
) -> Vec<OverlayElement> {
    let document = doc.inner();

    let page = match document.pages().get(page_index as PdfPageIndex) {
        Ok(page) => page,
        Err(_) => return Vec::new(),
    };

    let page_height = page.height().value;
    let annotations = page.annotations();

    let mut elements = Vec::new();

    for annotation in annotations.iter() {
        if annotation.creator().as_deref() != Some(APP_MARKER) {
            continue;
        }

        let Ok(bounds) = annotation.bounds() else {
            continue;
        };
        let rect = pdf_rect_to_egui_rect(bounds, page_height);

        if annotation.as_free_text_annotation().is_some() {
            let Some(text) = annotation.contents() else {
                continue;
            };
            elements.push(OverlayElement::TextBox(TextBoxElement {
                id: Uuid::new_v4(),
                page_index,
                rect,
                text,
            }));
        } else if annotation.as_stamp_annotation().is_some() {
            let Some(image) = annotation
                .objects()
                .iter()
                .find_map(|object| object.as_image_object()?.get_raw_image().ok())
            else {
                continue;
            };
            elements.push(OverlayElement::Signature(SignatureElement {
                id: Uuid::new_v4(),
                page_index,
                rect,
                image: dynamic_image_to_color_image(image),
            }));
        }
    }

    elements
}

/// Converts a page-space `egui::Rect` (PDF points, origin top-left, matching
/// [`crate::document::PdfDocumentState::render_page`] and
/// [`crate::form_fields::fields_on_page`]) into a `PdfRect` (PDF points,
/// origin bottom-left), the inverse of [`pdf_rect_to_egui_rect`].
fn egui_rect_to_pdf_rect(rect: egui::Rect, page_height: f32) -> PdfRect {
    let bottom = PdfPoints::new(page_height - rect.max.y);
    let left = PdfPoints::new(rect.min.x);
    let top = PdfPoints::new(page_height - rect.min.y);
    let right = PdfPoints::new(rect.max.x);

    PdfRect::new(bottom, left, top, right)
}

/// The inverse of [`egui_rect_to_pdf_rect`].
fn pdf_rect_to_egui_rect(rect: PdfRect, page_height: f32) -> egui::Rect {
    let top_left = egui::pos2(rect.left().value, page_height - rect.top().value);
    let bottom_right = egui::pos2(rect.right().value, page_height - rect.bottom().value);

    egui::Rect::from_two_pos(top_left, bottom_right)
}

/// Converts an `egui::ColorImage` into an `image::DynamicImage` suitable for
/// [`PdfPageImageObject::new_with_size`], the inverse of
/// [`dynamic_image_to_color_image`].
fn color_image_to_dynamic_image(image: &egui::ColorImage) -> DynamicImage {
    let [width, height] = image.size;

    let mut raw = Vec::with_capacity(width * height * 4);
    for pixel in &image.pixels {
        raw.extend_from_slice(&pixel.to_srgba_unmultiplied());
    }

    let buffer = RgbaImage::from_raw(width as u32, height as u32, raw)
        .expect("ColorImage's pixel buffer always matches its declared size");

    DynamicImage::ImageRgba8(buffer)
}

/// The inverse of [`color_image_to_dynamic_image`].
fn dynamic_image_to_color_image(image: DynamicImage) -> egui::ColorImage {
    let rgba = image.to_rgba8();
    let size = [rgba.width() as usize, rgba.height() as usize];

    egui::ColorImage::from_rgba_unmultiplied(size, rgba.as_raw())
}

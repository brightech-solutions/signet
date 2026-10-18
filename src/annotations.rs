//! Round-trips overlay elements through real PDF annotation objects.
//!
//! An un-applied overlay element (a text box or placed signature) must
//! survive a save as a normal, portable PDF annotation — never a custom
//! file format or sidecar metadata — so that the file stays openable in any
//! PDF viewer even before the user "applies" (flattens) their edits.
//! [`write_overlay_to_annotations`] writes each [`OverlayElement`] as a
//! stamp annotation whose appearance is made of real page objects — laid-out
//! text objects for a text box (its raw text also kept in `/Contents`), an
//! embedded image for a signature — tagging each one's `/T` (title/author)
//! field with [`APP_MARKER`] so this app — and only this app — can tell its
//! own annotations apart from anything already in the PDF. Every annotation
//! written also gets the Print flag, so it prints and so `apply::flatten`
//! (pdfium's `FLAT_PRINT` flatten) bakes it rather than dropping it.
//!
//! Text boxes used to be `FreeText` annotations, but those have no
//! appearance until pdfium invents one, and pdfium's version ran every
//! line together and drew a black border around the box — which is
//! exactly what got saved and baked. Building the appearance from our own
//! text objects (the same mechanism signatures already used) makes what's
//! saved, shown by other viewers, and baked by Apply match what was typed.
//!
//! [`read_overlay_from_annotations`] is the inverse: it reconstructs
//! `OverlayElement`s from annotations carrying that marker (including
//! `FreeText` text boxes written by earlier builds), and silently skips
//! every other annotation (wrong subtype, wrong/missing title, or both), so
//! a pre-existing annotation in a foreign PDF is never misinterpreted as
//! one of ours.

use image::{DynamicImage, RgbaImage};
use pdfium_render::prelude::*;
use uuid::Uuid;

use crate::document::{DocumentError, PdfDocumentState};
use crate::overlay::{OverlayElement, SignatureElement, TextBoxElement};

/// The value written to every annotation this app creates' `/T` (title /
/// author) field, and the only value [`read_overlay_from_annotations`]
/// treats as "one of ours".
pub const APP_MARKER: &str = "BrightechPDFUtility";

/// Font size (points) text boxes are written at.
pub const TEXT_BOX_FONT_SIZE: f32 = 12.0;
/// Baseline-to-baseline distance (points) between a text box's lines.
pub const TEXT_BOX_LINE_PITCH: f32 = 14.0;
/// Inset (points) of a text box's text from its rect's top/left/right.
pub const TEXT_BOX_PADDING: f32 = 2.0;
/// Helvetica's ascent as a fraction of the font size (its AFM `Ascender`
/// is 718/1000; rounded up slightly so accented capitals stay inside).
const HELVETICA_ASCENT: f32 = 0.77;

/// Writes `elements` onto page `page_index` of `doc` as real PDF annotation
/// objects: a stamp annotation for each element, whose appearance is laid-
/// out text (for an [`OverlayElement::TextBox`]) or an embedded image (for
/// an [`OverlayElement::Signature`]). Every annotation written has its
/// `/T` field set to [`APP_MARKER`] and its Print flag set.
pub fn write_overlay_to_annotations(
    doc: &mut PdfDocumentState,
    page_index: usize,
    elements: &[OverlayElement],
) -> Result<(), DocumentError> {
    let document = doc.inner_mut();
    let font = document.fonts_mut().helvetica();

    let mut page = document
        .pages()
        .get(page_index as PdfPageIndex)
        .map_err(|err| DocumentError::Pdfium(err.to_string()))?;

    let page_height = page.height().value;

    for element in elements {
        match element {
            OverlayElement::TextBox(text_box) => {
                write_text_box_annotation(document, &mut page, text_box, page_height, font)?;
            }
            OverlayElement::Signature(signature) => {
                write_signature_annotation(document, &mut page, signature, page_height)?;
            }
        }
    }

    Ok(())
}

fn write_text_box_annotation(
    document: &PdfDocument<'static>,
    page: &mut PdfPage<'static>,
    text_box: &TextBoxElement,
    page_height: f32,
    font: PdfFontToken,
) -> Result<(), DocumentError> {
    let bounds = egui_rect_to_pdf_rect(text_box.rect, page_height);
    let max_line_width = (bounds.width().value - 2.0 * TEXT_BOX_PADDING).max(1.0);
    let lines = wrap_text(&text_box.text, max_line_width, |candidate| {
        measure_text(document, font, candidate)
    });

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
        .set_is_printed(true)
        .map_err(|err| DocumentError::Pdfium(err.to_string()))?;
    annotation
        .set_contents(&text_box.text)
        .map_err(|err| DocumentError::Pdfium(err.to_string()))?;

    let left = bounds.left().value + TEXT_BOX_PADDING;
    let first_baseline =
        bounds.top().value - TEXT_BOX_PADDING - HELVETICA_ASCENT * TEXT_BOX_FONT_SIZE;

    // `wrap_text` always yields at least one line; an empty one becomes a
    // single-space text object (pdfium-render substitutes " " for ""), so
    // even an empty box gets a real (invisible) appearance stream rather
    // than none — some viewers draw a placeholder for an appearance-less
    // stamp.
    for (index, line) in lines.iter().enumerate() {
        let mut text_object = PdfPageTextObject::new(
            document,
            line,
            font,
            PdfPoints::new(TEXT_BOX_FONT_SIZE),
        )
        .map_err(|err| DocumentError::Pdfium(err.to_string()))?;
        text_object
            .set_fill_color(PdfColor::BLACK)
            .map_err(|err| DocumentError::Pdfium(err.to_string()))?;
        text_object
            .translate(
                PdfPoints::new(left),
                PdfPoints::new(first_baseline - index as f32 * TEXT_BOX_LINE_PITCH),
            )
            .map_err(|err| DocumentError::Pdfium(err.to_string()))?;
        annotation
            .objects_mut()
            .add_text_object(text_object)
            .map_err(|err| DocumentError::Pdfium(err.to_string()))?;
    }

    Ok(())
}

/// The rendered width (points) of `text` in `font` at
/// [`TEXT_BOX_FONT_SIZE`], measured with a detached pdfium text object.
/// Falls back to a deliberately generous estimate if pdfium can't measure
/// it, so a measuring failure wraps early rather than overflowing.
fn measure_text(document: &PdfDocument<'static>, font: PdfFontToken, text: &str) -> f32 {
    PdfPageTextObject::new(document, text, font, PdfPoints::new(TEXT_BOX_FONT_SIZE))
        .and_then(|object| object.width())
        .map(|width| width.value)
        .unwrap_or(text.chars().count() as f32 * TEXT_BOX_FONT_SIZE * 0.7)
}

/// Breaks `text` into display lines no wider than `max_width` as measured
/// by `measure`: every `\n` starts a new line, and within a line words are
/// wrapped greedily at spaces (a single word too long for a line on its
/// own is broken between characters). Always returns at least one line.
pub fn wrap_text(text: &str, max_width: f32, measure: impl Fn(&str) -> f32) -> Vec<String> {
    let mut lines = Vec::new();

    for paragraph in text.split('\n') {
        let paragraph = paragraph.trim_end_matches('\r');
        let mut current = String::new();

        for word in paragraph.split(' ') {
            let candidate = if current.is_empty() {
                word.to_string()
            } else {
                format!("{current} {word}")
            };
            if measure(&candidate) <= max_width {
                current = candidate;
                continue;
            }

            if !current.is_empty() {
                lines.push(std::mem::take(&mut current));
            }
            // `word` alone, broken between characters if even it doesn't fit.
            for ch in word.chars() {
                let mut extended = current.clone();
                extended.push(ch);
                if !current.is_empty() && measure(&extended) > max_width {
                    lines.push(std::mem::take(&mut current));
                    current.push(ch);
                } else {
                    current = extended;
                }
            }
        }

        lines.push(current);
    }

    lines
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
        .set_is_printed(true)
        .map_err(|err| DocumentError::Pdfium(err.to_string()))?;
    annotation
        .objects_mut()
        .add_image_object(image_object)
        .map_err(|err| DocumentError::Pdfium(err.to_string()))?;

    Ok(())
}

/// Deletes every annotation on page `page_index` whose `/T` field is
/// [`APP_MARKER`] — i.e. everything [`write_overlay_to_annotations`] ever
/// wrote there — leaving every other annotation (form widgets, a foreign
/// PDF's own comments) alone. Returns how many were removed. An
/// out-of-bounds `page_index` is an error, like a failed delete.
///
/// Used so the document never holds a stale copy of the overlay: on open
/// (after reading the elements back with [`read_overlay_from_annotations`],
/// so they aren't drawn twice — once baked into the page render, once as
/// the live overlay), and before every Save/Apply writes the overlay's
/// current state (so a repeated save, or a moved/deleted element, can't
/// leave duplicate or ghost annotations behind).
pub fn remove_overlay_annotations(
    doc: &mut PdfDocumentState,
    page_index: usize,
) -> Result<usize, DocumentError> {
    let document = doc.inner_mut();
    let mut page = document
        .pages()
        .get(page_index as PdfPageIndex)
        .map_err(|err| DocumentError::Pdfium(err.to_string()))?;
    let annotations = page.annotations_mut();

    let mut removed = 0;
    // Back to front, so deleting index `i` never shifts an index still to
    // be visited.
    for index in (0..annotations.len()).rev() {
        let annotation = annotations
            .get(index)
            .map_err(|err| DocumentError::Pdfium(err.to_string()))?;
        if annotation.creator().as_deref() != Some(APP_MARKER) {
            continue;
        }
        annotations
            .delete_annotation(annotation)
            .map_err(|err| DocumentError::Pdfium(err.to_string()))?;
        removed += 1;
    }

    Ok(removed)
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

        let has_image = annotation.as_stamp_annotation().is_some()
            && annotation
                .objects()
                .iter()
                .any(|object| object.as_image_object().is_some());

        if annotation.as_free_text_annotation().is_some()
            || (annotation.as_stamp_annotation().is_some() && !has_image)
        {
            // A text box: a stamp of text objects (current format) or a
            // `FreeText` annotation (earlier builds). Either way the raw
            // text — newlines included — is in `/Contents`; an empty box
            // may have none at all.
            elements.push(OverlayElement::TextBox(TextBoxElement {
                id: Uuid::new_v4(),
                page_index,
                rect,
                text: annotation.contents().unwrap_or_default(),
            }));
        } else if has_image {
            // `get_raw_image` explicitly ignores the image's soft mask —
            // i.e. it would hand back a signature's alpha channel as
            // fully opaque everywhere, destroying its transparency.
            // `get_processed_image` takes the mask (and any transform)
            // into account, which is what a round trip needs here.
            let Some(image) = annotation
                .objects()
                .iter()
                .find_map(|object| object.as_image_object()?.get_processed_image(document).ok())
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

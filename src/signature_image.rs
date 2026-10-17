//! Background removal for signature images uploaded into the personal
//! signature library (see [`crate::signatures`]).
//!
//! A signature photographed or scanned on white paper arrives as a fully
//! opaque image: the paper's near-white background is just as opaque as the
//! ink. Placed on top of a PDF page as-is, that would show as an opaque
//! rectangle covering whatever is underneath, rather than looking like ink
//! on the page. [`strip_light_background`] fixes this by making near-white
//! pixels fully transparent, leaving only the ink opaque.

use image::RgbaImage;

/// Returns `img` with every pixel whose red, green, and blue channels are
/// all `>= threshold` made fully transparent (alpha set to `0`). Every
/// other pixel — including its existing alpha — is left unchanged.
///
/// A simple per-channel threshold (rather than, say, distance from pure
/// white) is sufficient for typical signature photos: ink is dark enough on
/// at least one channel to fail the `>= threshold` test, while the paper
/// background is uniformly light across all three channels.
pub fn strip_light_background(mut img: RgbaImage, threshold: u8) -> RgbaImage {
    for pixel in img.pixels_mut() {
        if pixel[0] >= threshold && pixel[1] >= threshold && pixel[2] >= threshold {
            pixel[3] = 0;
        }
    }
    img
}

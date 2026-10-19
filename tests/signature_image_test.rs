use signet::signature_image::strip_light_background;

#[test]
fn near_white_pixels_become_transparent() {
    let mut img = image::RgbaImage::new(2, 1);
    img.put_pixel(0, 0, image::Rgba([250, 250, 250, 255])); // near-white background
    img.put_pixel(1, 0, image::Rgba([10, 10, 10, 255])); // dark ink
    let out = strip_light_background(img, 240);
    assert_eq!(out.get_pixel(0, 0)[3], 0); // background now transparent
    assert_eq!(out.get_pixel(1, 0)[3], 255); // ink stays opaque
}

/// A pixel that fails the threshold on even one channel (here, a mid-gray
/// stroke slightly darker than the background) must be left alone,
/// including its alpha — the whole point of the per-channel-AND test.
#[test]
fn pixel_below_threshold_on_any_channel_is_untouched() {
    let mut img = image::RgbaImage::new(1, 1);
    img.put_pixel(0, 0, image::Rgba([239, 255, 255, 255]));
    let out = strip_light_background(img, 240);
    assert_eq!(*out.get_pixel(0, 0), image::Rgba([239, 255, 255, 255]));
}

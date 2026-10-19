use signet::signatures::{
    DEFAULT_FONT_ID, SCRIPT_FONT_ID, SignatureEntry, SignatureLibrary, SignatureSource,
    render_signature_entry,
};
use uuid::Uuid;

#[test]
fn add_and_reload_image_signature_round_trips() {
    let tmp = tempfile::tempdir().unwrap();
    let source = tmp.path().join("sig.png");
    image::RgbaImage::new(10, 10).save(&source).unwrap();
    let mut lib = SignatureLibrary::load(tmp.path());
    let id = lib.add_image(tmp.path(), "My signature", &source).unwrap();
    lib.save(tmp.path()).unwrap();

    let reloaded = SignatureLibrary::load(tmp.path());
    assert!(
        reloaded
            .entries
            .iter()
            .any(|e| e.id == id && e.name == "My signature")
    );
    assert!(reloaded.image_path_for(tmp.path(), id).unwrap().exists());
}

#[test]
fn add_typed_signature_persists_text_and_font() {
    let tmp = tempfile::tempdir().unwrap();
    let mut lib = SignatureLibrary::load(tmp.path());
    let id = lib.add_typed(tmp.path(), "Typed", "Jane Doe", "script-font-1");
    lib.save(tmp.path()).unwrap();
    let reloaded = SignatureLibrary::load(tmp.path());
    let entry = reloaded.entries.iter().find(|e| e.id == id).unwrap();
    assert!(
        matches!(&entry.source, SignatureSource::Typed { text, font_id }
        if text == "Jane Doe" && font_id == "script-font-1")
    );
}

#[test]
fn remove_deletes_entry() {
    let tmp = tempfile::tempdir().unwrap();
    let mut lib = SignatureLibrary::load(tmp.path());
    let id = lib.add_typed(tmp.path(), "Typed", "Jane Doe", "script-font-1");
    lib.remove(tmp.path(), id);
    assert!(lib.entries.is_empty());
}

/// Final review Important #5: deleting an image signature must delete its
/// backing `<id>.png` too, not leave the user's signature image on disk.
#[test]
fn remove_deletes_image_signature_file() {
    let tmp = tempfile::tempdir().unwrap();
    let source_dir = tempfile::tempdir().unwrap();
    let source = source_dir.path().join("sig.png");
    image::RgbaImage::new(10, 10).save(&source).unwrap();
    let mut lib = SignatureLibrary::load(tmp.path());
    let id = lib.add_image(tmp.path(), "My signature", &source).unwrap();
    let image_path = lib.image_path_for(tmp.path(), id).unwrap();
    assert!(image_path.exists());

    lib.remove(tmp.path(), id);

    assert!(lib.entries.is_empty());
    assert!(!image_path.exists(), "signature image left on disk");
    assert_eq!(std::fs::read_dir(tmp.path()).unwrap().count(), 0);
}

/// The file deletion is best-effort: an image that's already gone must
/// not stop the entry itself being removed (or panic).
#[test]
fn remove_tolerates_an_already_missing_image_file() {
    let tmp = tempfile::tempdir().unwrap();
    let source_dir = tempfile::tempdir().unwrap();
    let source = source_dir.path().join("sig.png");
    image::RgbaImage::new(10, 10).save(&source).unwrap();
    let mut lib = SignatureLibrary::load(tmp.path());
    let id = lib.add_image(tmp.path(), "My signature", &source).unwrap();
    std::fs::remove_file(lib.image_path_for(tmp.path(), id).unwrap()).unwrap();

    lib.remove(tmp.path(), id);

    assert!(lib.entries.is_empty());
}

/// Not in the brief's Step 5 list, but a natural gap to cover alongside
/// the round-trip test: an uploaded image's white background must actually
/// come out transparent on disk, not just get copied through unchanged.
/// This is the integration point between `add_image` and
/// `signature_image::strip_light_background` — a pixel-level check that
/// the real file written by `add_image` has its near-white background
/// stripped.
#[test]
fn add_image_strips_light_background_in_saved_file() {
    let tmp = tempfile::tempdir().unwrap();
    let source = tmp.path().join("sig.png");

    let mut img = image::RgbaImage::new(2, 1);
    img.put_pixel(0, 0, image::Rgba([255, 255, 255, 255])); // white background
    img.put_pixel(1, 0, image::Rgba([0, 0, 0, 255])); // dark ink
    img.save(&source).unwrap();

    let mut lib = SignatureLibrary::load(tmp.path());
    let id = lib.add_image(tmp.path(), "My signature", &source).unwrap();

    let saved_path = lib.image_path_for(tmp.path(), id).unwrap();
    let saved = image::open(&saved_path).unwrap().to_rgba8();
    assert_eq!(saved.get_pixel(0, 0)[3], 0);
    assert_eq!(saved.get_pixel(1, 0)[3], 255);
}

/// `load` on a directory with no manifest yet (first run) must return an
/// empty library rather than erroring or panicking.
#[test]
fn load_with_no_manifest_returns_empty_library() {
    let tmp = tempfile::tempdir().unwrap();
    let lib = SignatureLibrary::load(tmp.path());
    assert!(lib.entries.is_empty());
}

/// `render_signature_entry` for an image-backed entry must load the real
/// pixels `add_image` wrote to disk (already background-stripped), not a
/// stand-in/placeholder image.
#[test]
fn render_signature_entry_loads_real_image_pixels() {
    let tmp = tempfile::tempdir().unwrap();
    let source = tmp.path().join("sig.png");

    let mut img = image::RgbaImage::new(2, 1);
    img.put_pixel(0, 0, image::Rgba([255, 255, 255, 255])); // white background
    img.put_pixel(1, 0, image::Rgba([10, 20, 30, 255])); // dark ink
    img.save(&source).unwrap();

    let mut lib = SignatureLibrary::load(tmp.path());
    let id = lib.add_image(tmp.path(), "My signature", &source).unwrap();
    let entry = lib.entries.iter().find(|e| e.id == id).unwrap();

    let rendered = render_signature_entry(entry, tmp.path());
    assert_eq!(rendered.size, [2, 1]);
    // Background pixel was stripped to fully transparent by `add_image`.
    assert_eq!(rendered.pixels[0].a(), 0);
    // Ink pixel survives with its real color, fully opaque.
    let ink = rendered.pixels[1];
    assert_eq!((ink.r(), ink.g(), ink.b(), ink.a()), (10, 20, 30, 255));
}

/// A missing/unreadable image file must not panic — it's a real failure
/// mode (e.g. the file was deleted out from under the manifest) that has
/// to degrade to *something* renderable instead of crashing the app.
#[test]
fn render_signature_entry_on_missing_image_file_does_not_panic() {
    let tmp = tempfile::tempdir().unwrap();
    let entry = SignatureEntry {
        id: Uuid::new_v4(),
        name: "Gone".to_string(),
        source: SignatureSource::Image {
            file_name: "does-not-exist.png".to_string(),
        },
        created: "0".to_string(),
    };

    let rendered = render_signature_entry(&entry, tmp.path());
    assert_eq!(rendered.size, [1, 1]);
}

/// A typed entry must actually rasterize its text into real glyph ink —
/// not an empty/placeholder image — for both the bundled script font and
/// the fallback default font.
#[test]
fn render_signature_entry_rasterizes_typed_text_for_both_fonts() {
    let tmp = tempfile::tempdir().unwrap();

    for font_id in [SCRIPT_FONT_ID, DEFAULT_FONT_ID] {
        let entry = SignatureEntry {
            id: Uuid::new_v4(),
            name: "Typed".to_string(),
            source: SignatureSource::Typed {
                text: "Jane Doe".to_string(),
                font_id: font_id.to_string(),
            },
            created: "0".to_string(),
        };

        let rendered = render_signature_entry(&entry, tmp.path());
        assert!(
            rendered.size[0] > 1 && rendered.size[1] > 1,
            "font {font_id}: expected a real glyph bitmap, got {:?}",
            rendered.size
        );
        assert!(
            rendered.pixels.iter().any(|pixel| pixel.a() > 0),
            "font {font_id}: expected at least one opaque ink pixel"
        );
    }
}

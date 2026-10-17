use pdf_utility::signatures::{SignatureLibrary, SignatureSource};

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
    lib.remove(id);
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

#[test]
fn builds_and_links() {
    // presence check only: confirms the vendored dylib for this host's arch exists
    let arch_dir = if cfg!(target_arch = "aarch64") {
        "macos-arm64"
    } else {
        "macos-x86_64"
    };
    assert!(
        std::path::Path::new("vendor/pdfium")
            .join(arch_dir)
            .join("libpdfium.dylib")
            .exists()
    );
}

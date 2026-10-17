# PDF Utility Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Build a native macOS desktop app in Rust that can view PDFs, fill them in (real form fields or free-form text boxes), and sign them with a reusable personal signature library.

**Architecture:** A single Rust binary built on `egui`/`eframe` for the UI, with Google's pdfium (via `pdfium-render`) doing all PDF reading, rendering, form-field access, annotation writing, and page flattening. One in-memory document model per open file; "editable after save" state (filled text boxes, placed signatures) round-trips through standard PDF annotation objects marked with an app identifier, so a saved-but-not-applied file stays a normal, portable PDF.

**Tech Stack:** Rust, `egui` + `eframe`, `pdfium-render` (against a vendored prebuilt pdfium binary), `rfd` (native file dialogs), `image` (signature background removal), `serde`/`serde_json` (signature library manifest), `uuid`.

**Spec:** `docs/superpowers/specs/2026-10-17-pdf-utility-design.md`

## Global Constraints

- macOS only; target a universal binary (arm64 + x86_64) by default.
- Rust application code; pdfium is the one non-Rust dependency, vendored as a prebuilt dynamic library bundled inside the `.app` (no separate installer, no background process, no network calls at runtime).
- No AGPL-licensed dependencies.
- Fully offline: no telemetry, no auto-update checks, no network access anywhere in the running app.
- Un-applied edits (filled fields, text boxes, signatures) persist as standard PDF annotation objects (`FreeText` for text boxes, stamp annotations for signatures, real AcroForm field values for filled fields) — never a custom file format or sidecar metadata.
- Apply uses pdfium's native page-flatten (`FPDFPage_Flatten`) to bake annotations/fields into page content and strip interactivity.
- No password-protected/encrypted PDF support (explicit non-goal for now).
- No full undo/redo history; only direct manipulation (move/resize/delete a placed element before saving).

## Review Focus

- Opening a corrupt or non-PDF file must show an error state, not crash or hang — covered in Task 2.
- Opening a password-protected PDF must show a clear "not supported" message, not crash or silently produce a blank document — covered in Task 2.
- Placing a text box or signature with its drop point near a page edge must clamp the element fully inside the page bounds, not leave it partially or fully off-page and unreachable — covered in Task 5.
- Clicking a non-text AcroForm field (checkbox, radio, dropdown) must not crash the app, even if full editing of that field type isn't implemented — covered in Task 4.
- Once Apply has flattened a document, editing tools (placing/moving/resizing text boxes or signatures, typing into fields) must be disabled rather than silently accepted and then lost — covered in Task 10.

---

## File Structure

- `Cargo.toml` — binary crate `pdf_utility`.
- `vendor/pdfium/` — vendored prebuilt pdfium dynamic library (from `bblanchon/pdfium-binaries`) for macOS arm64 and x86_64, plus its license file.
- `src/main.rs` — entry point; `eframe::run_native` setup; reads `std::env::args()` for a launch-time file path (Finder double-click/"Open With" delivers the path as the first CLI argument on macOS).
- `src/app.rs` — `PdfUtilityApp` (implements `eframe::App`), top-level UI state and update loop, wires the modules below together.
- `src/document.rs` — `PdfDocumentState`: wraps a pdfium `PdfDocument`, current page index, zoom level; open/render.
- `src/form_fields.rs` — AcroForm field enumeration, hit-testing, value read/write.
- `src/overlay.rs` — `OverlayElement` (text box / signature) model shared by placement, selection, move/resize/delete, and page-bounds clamping.
- `src/annotations.rs` — round-trip of `OverlayElement`s to/from PDF annotation objects, using an app-identifying marker.
- `src/signatures.rs` — signature library manifest (load/save), adding an image or typed signature.
- `src/signature_image.rs` — background-removal image processing for uploaded signature photos.
- `src/apply.rs` — flatten (Apply) logic.
- `src/save.rs` — Save / Save As logic tying `annotations.rs` + `form_fields.rs` together.
- `src/ui/mod.rs`, `src/ui/toolbar.rs`, `src/ui/canvas.rs`, `src/ui/signature_panel.rs` — UI glue (manual verification; see Task Right-Sizing note below).
- `packaging/Info.plist`, `packaging/bundle.sh` — `.app` bundle assembly, codesigning, pdfium framework bundling.
- `tests/fixtures/sample_plain.pdf`, `tests/fixtures/sample_form.pdf` (one text field named `applicant_name`), `tests/fixtures/sample_encrypted.pdf`, `tests/fixtures/corrupt.pdf` — checked-in test fixtures.

Note on testability: `document.rs`, `form_fields.rs`, `overlay.rs`, `annotations.rs`, `signatures.rs`, `signature_image.rs`, `apply.rs`, and `save.rs` are pure logic over pdfium/image/filesystem and get real unit/integration tests. The `src/ui/*` files are `egui` rendering and mouse-interaction glue with no inspectable automated test surface; their tasks use explicit manual-verification steps instead of unit tests, per the project's GUI constraints.

---

### Task 1: Project scaffolding and pdfium vendoring

**Files:**
- Create: `Cargo.toml`, `src/main.rs`, `src/app.rs`
- Create: `vendor/pdfium/macos-arm64/libpdfium.dylib`, `vendor/pdfium/macos-x86_64/libpdfium.dylib`, `vendor/pdfium/LICENSE`
- Test: `tests/smoke_test.rs`

**Interfaces:**
- Produces: `fn pdfium_lib_path() -> PathBuf` in `src/document.rs` is NOT yet defined here — this task only vendors the binaries and gets a blank window running; Task 2 defines the loader.

- [ ] **Step 1: Create the binary crate**

Run: `cargo init --name pdf_utility .`
Expected: `Cargo.toml` and `src/main.rs` created.

- [ ] **Step 2: Add core dependencies to `Cargo.toml`**

Add `eframe`, `egui`, `pdfium-render`, `rfd`, `image`, `serde` (with `derive`), `serde_json`, `uuid` (with `v4`), each pinned to a current stable version.

- [ ] **Step 3: Vendor the prebuilt pdfium binaries**

Download the macOS arm64 and x86_64 releases from `bblanchon/pdfium-binaries`, extract `libpdfium.dylib` into `vendor/pdfium/macos-arm64/` and `vendor/pdfium/macos-x86_64/` respectively, and copy the project's `LICENSE` file into `vendor/pdfium/LICENSE`. Commit the binaries as binary blobs.

- [ ] **Step 4: Write `PdfUtilityApp` as an empty `eframe::App`**

`struct PdfUtilityApp;` implementing `eframe::App::update` with a single `egui::CentralPanel` showing the text "PDF Utility". `src/main.rs` calls `eframe::run_native("PDF Utility", NativeOptions::default(), Box::new(|_cc| Ok(Box::new(PdfUtilityApp))))`.

- [ ] **Step 5: Write a smoke test**

```rust
#[test]
fn builds_and_links() {
    // presence check only: confirms the vendored dylib for this host's arch exists
    let arch_dir = if cfg!(target_arch = "aarch64") { "macos-arm64" } else { "macos-x86_64" };
    assert!(std::path::Path::new("vendor/pdfium").join(arch_dir).join("libpdfium.dylib").exists());
}
```

- [ ] **Step 6: Run test to verify it passes**

Run: `cargo test smoke_test -- --nocapture`
Expected: PASS

- [ ] **Step 7: Run the app manually to verify the window opens**

Run: `cargo run`
Expected: a window titled "PDF Utility" appears showing the placeholder text.

- [ ] **Step 8: Commit**

```bash
git add Cargo.toml Cargo.lock src/main.rs src/app.rs vendor/ tests/smoke_test.rs .gitignore
git commit -m "feat: scaffold eframe app and vendor pdfium binaries"
```

---

### Task 2: Document core — open and render via pdfium

**Files:**
- Create: `src/document.rs`
- Modify: `src/main.rs` (wire pdfium library path resolution)
- Test: `tests/document_test.rs`

**Interfaces:**
- Consumes: vendored dylib paths from Task 1 (`vendor/pdfium/<arch>/libpdfium.dylib`).
- Produces:
  - `pub enum DocumentError { Io(std::io::Error), Corrupt, PasswordProtected, Pdfium(String) }`
  - `pub struct PdfDocumentState { /* fields private */ }`
  - `impl PdfDocumentState { pub fn open(path: &Path) -> Result<Self, DocumentError>; pub fn page_count(&self) -> usize; pub fn render_page(&self, index: usize, zoom: f32) -> Result<egui::ColorImage, DocumentError>; pub fn page_size_points(&self, index: usize) -> egui::Vec2; }`

- [ ] **Step 1: Write failing tests for open() on each fixture**

```rust
#[test]
fn opens_plain_pdf() {
    let doc = PdfDocumentState::open(Path::new("tests/fixtures/sample_plain.pdf")).unwrap();
    assert!(doc.page_count() >= 1);
}

#[test]
fn rejects_corrupt_pdf() {
    let err = PdfDocumentState::open(Path::new("tests/fixtures/corrupt.pdf")).unwrap_err();
    assert!(matches!(err, DocumentError::Corrupt));
}

#[test]
fn rejects_password_protected_pdf() {
    let err = PdfDocumentState::open(Path::new("tests/fixtures/sample_encrypted.pdf")).unwrap_err();
    assert!(matches!(err, DocumentError::PasswordProtected));
}
```

Note: `tests/fixtures/sample_plain.pdf`, `corrupt.pdf` (a truncated/garbage byte file), and `sample_encrypted.pdf` (any PDF saved with an open password) must exist; create and check them in as part of this step if not already present.

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test document_test -- --nocapture`
Expected: FAIL (`PdfDocumentState` not defined)

- [ ] **Step 3: Implement `PdfDocumentState::open`, `page_count`, `render_page`, `page_size_points` in `src/document.rs`**

Resolve the pdfium library path by host architecture (as in Task 1's smoke test), initialize `pdfium_render::prelude::Pdfium` with it, load the document via `Pdfium::load_pdf_from_file`. Map pdfium's password-required error to `DocumentError::PasswordProtected` and any other load failure to `DocumentError::Corrupt`. `render_page` renders at the given zoom (1.0 = 72 DPI) into an RGBA buffer and converts it to `egui::ColorImage::from_rgba_unmultiplied`.

- [ ] **Step 4: Run tests to verify they pass**

Run: `cargo test document_test -- --nocapture`
Expected: PASS

- [ ] **Step 5: Commit**

```bash
git add src/document.rs tests/document_test.rs tests/fixtures/
git commit -m "feat: open and render PDFs via pdfium"
```

---

### Task 3: Viewer UI — open, scroll, zoom, launch-arg open

**Files:**
- Modify: `src/app.rs`
- Create: `src/ui/mod.rs`, `src/ui/canvas.rs`
- Modify: `src/main.rs` (pass launch-arg path into `PdfUtilityApp::new`)
- Create: `packaging/Info.plist`

**Interfaces:**
- Consumes: `PdfDocumentState::{open, page_count, render_page, page_size_points}` (Task 2).
- Produces: `impl PdfUtilityApp { pub fn new(initial_path: Option<PathBuf>) -> Self }`; `pub struct ViewerState { pub current_page: usize, pub zoom: f32 }` on `PdfUtilityApp`, read by later tasks' overlay/canvas code to map page coordinates to screen coordinates.

- [ ] **Step 1: Write a failing test for the page/screen coordinate transform**

```rust
#[test]
fn page_point_to_screen_accounts_for_zoom_and_scroll() {
    let screen = page_to_screen(egui::pos2(100.0, 50.0), /*zoom*/ 2.0, /*scroll_offset*/ egui::vec2(10.0, 0.0));
    assert_eq!(screen, egui::pos2(190.0, 100.0));
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test page_point_to_screen -- --nocapture`
Expected: FAIL

- [ ] **Step 3: Implement `pub fn page_to_screen(page_point: egui::Pos2, zoom: f32, scroll_offset: egui::Vec2) -> egui::Pos2` in `src/ui/canvas.rs`**

`screen = page_point * zoom - scroll_offset`.

- [ ] **Step 4: Run test to verify it passes**

Run: `cargo test page_point_to_screen -- --nocapture`
Expected: PASS

- [ ] **Step 5: Wire File → Open (via `rfd::FileDialog::pick_file` filtered to `pdf`), page scroll (egui `ScrollArea`), and zoom in/out buttons into `PdfUtilityApp`**

On successful `PdfDocumentState::open`, store it as `self.document: Option<PdfDocumentState>` and reset `ViewerState { current_page: 0, zoom: 1.0 }`. On `DocumentError`, show an `egui` modal/error label with a message matching the error variant (corrupt → "This file isn't a valid PDF."; password-protected → "This PDF is password-protected, which isn't supported yet.").

- [ ] **Step 6: Wire launch-arg open**

`src/main.rs` reads `std::env::args().nth(1)` and passes it as `Option<PathBuf>` into `PdfUtilityApp::new`, which opens it the same way as File → Open.

- [ ] **Step 7: Write `packaging/Info.plist` declaring PDF document support**

Include `CFBundleDocumentTypes` with `CFBundleTypeName = "PDF Document"`, `LSItemContentTypes = ["com.adobe.pdf"]`, `CFBundleTypeRole = "Editor"`.

- [ ] **Step 8: Manual verification**

Run: `cargo run -- tests/fixtures/sample_plain.pdf`
Expected: window opens directly showing page 1 of the fixture; scrolling and the zoom buttons work; File → Open on `corrupt.pdf` shows the "not a valid PDF" message instead of crashing; File → Open on `sample_encrypted.pdf` shows the "password-protected" message.

- [ ] **Step 9: Commit**

```bash
git add src/app.rs src/ui/ src/main.rs packaging/Info.plist
git commit -m "feat: PDF viewer with open, scroll, zoom, and launch-arg open"
```

---

### Task 4: AcroForm field enumeration and fill

**Files:**
- Create: `src/form_fields.rs`
- Modify: `src/app.rs`, `src/ui/canvas.rs` (click-to-edit overlay for a focused field)
- Test: `tests/form_fields_test.rs`

**Interfaces:**
- Consumes: `PdfDocumentState` (Task 2); `ViewerState` and `page_to_screen` (Task 3).
- Produces:
  - `pub enum FieldKind { Text, Other }`
  - `pub struct FormFieldHit { pub name: String, pub page_rect: egui::Rect, pub kind: FieldKind }`
  - `pub fn fields_on_page(doc: &PdfDocumentState, page_index: usize) -> Vec<FormFieldHit>`
  - `pub fn hit_test<'a>(fields: &'a [FormFieldHit], page_point: egui::Pos2) -> Option<&'a FormFieldHit>`
  - `pub fn set_field_value(doc: &mut PdfDocumentState, name: &str, value: &str) -> Result<(), DocumentError>`

- [ ] **Step 1: Write failing tests**

```rust
#[test]
fn finds_known_text_field() {
    let doc = PdfDocumentState::open(Path::new("tests/fixtures/sample_form.pdf")).unwrap();
    let fields = fields_on_page(&doc, 0);
    let field = fields.iter().find(|f| f.name == "applicant_name").unwrap();
    assert_eq!(field.kind, FieldKind::Text);
}

#[test]
fn hit_test_finds_field_at_its_rect_center() {
    let doc = PdfDocumentState::open(Path::new("tests/fixtures/sample_form.pdf")).unwrap();
    let fields = fields_on_page(&doc, 0);
    let field = &fields[0];
    let center = field.page_rect.center();
    assert_eq!(hit_test(&fields, center).unwrap().name, field.name);
}

#[test]
fn hit_test_returns_none_outside_any_field() {
    let doc = PdfDocumentState::open(Path::new("tests/fixtures/sample_form.pdf")).unwrap();
    let fields = fields_on_page(&doc, 0);
    assert!(hit_test(&fields, egui::pos2(-100.0, -100.0)).is_none());
}

#[test]
fn set_field_value_round_trips() {
    let mut doc = PdfDocumentState::open(Path::new("tests/fixtures/sample_form.pdf")).unwrap();
    set_field_value(&mut doc, "applicant_name", "Jane Doe").unwrap();
    let fields = fields_on_page(&doc, 0);
    assert!(fields.iter().any(|f| f.name == "applicant_name"));
    // value readback is exercised end-to-end in Task 9's save/reopen test
}
```

Note: `tests/fixtures/sample_form.pdf` must contain exactly one text field named `applicant_name` on page 0; create and check it in if not already present (e.g. authored in Preview, LibreOffice, or a one-off `printpdf`/`lopdf` script).

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test form_fields_test -- --nocapture`
Expected: FAIL

- [ ] **Step 3: Implement `fields_on_page`, `hit_test`, `set_field_value` in `src/form_fields.rs`**

Use `pdfium-render`'s form-fill API (`PdfDocument::form()` / `PdfPage::annotations()` filtered to widget annotations) to enumerate fields and their page-space rects; map any non-text field type to `FieldKind::Other`. `hit_test` is a linear scan checking `field.page_rect.contains(page_point)`. `set_field_value` looks up the field by name and sets its text value through pdfium's form API, returning `DocumentError::Pdfium` if the field doesn't exist or isn't a text field.

- [ ] **Step 4: Run tests to verify they pass**

Run: `cargo test form_fields_test -- --nocapture`
Expected: PASS

- [ ] **Step 5: Wire click-to-edit into the canvas UI**

On click inside the page area, convert the click to a page-space point, call `hit_test`. If it hits a `FieldKind::Text` field, show an inline `egui::TextEdit` at that field's screen rect; on text change, call `set_field_value`. If it hits a `FieldKind::Other` field, do nothing (no crash, no editor shown) — this is the Review Focus case for non-text fields. If it hits no field, no-op here (Task 5 adds free-form text box placement for this case).

- [ ] **Step 6: Manual verification**

Run: `cargo run -- tests/fixtures/sample_form.pdf`
Expected: clicking inside the `applicant_name` field's rect shows an editable text box; typing updates it; clicking outside any field does nothing; if the fixture has a checkbox/dropdown, clicking it doesn't crash.

- [ ] **Step 7: Commit**

```bash
git add src/form_fields.rs src/app.rs src/ui/canvas.rs tests/form_fields_test.rs
git commit -m "feat: enumerate, hit-test, and fill AcroForm text fields"
```

---

### Task 5: Free-form text box overlay — placement, selection, move, resize, delete

**Files:**
- Create: `src/overlay.rs`
- Modify: `src/app.rs`, `src/ui/canvas.rs`
- Test: `tests/overlay_test.rs`

**Interfaces:**
- Consumes: `page_to_screen` (Task 3); `PdfDocumentState::page_size_points` (Task 2); `hit_test`/`fields_on_page` (Task 4) to decide whether a click should start a text box instead of editing a field.
- Produces:
  - `pub struct TextBoxElement { pub id: Uuid, pub page_index: usize, pub rect: egui::Rect, pub text: String }`
  - `pub enum OverlayElement { TextBox(TextBoxElement), Signature(SignatureElement) }` (`SignatureElement` defined fully in Task 8; stub it here as `pub struct SignatureElement { pub id: Uuid, pub page_index: usize, pub rect: egui::Rect, pub image: egui::ColorImage }` so `OverlayElement` compiles)
  - `pub fn clamp_to_page(rect: egui::Rect, page_size: egui::Vec2) -> egui::Rect`
  - `pub struct OverlayLayer { pub elements: Vec<OverlayElement> }` with `add`, `remove(id: Uuid)`, `move_element(id: Uuid, new_origin: egui::Pos2, page_size: egui::Vec2)`, `resize_element(id: Uuid, new_rect: egui::Rect, page_size: egui::Vec2)` — the latter two call `clamp_to_page` before storing.

- [ ] **Step 1: Write failing tests for clamping**

```rust
#[test]
fn clamp_keeps_fully_inside_page_when_dropped_at_edge() {
    let page_size = egui::vec2(600.0, 800.0);
    let rect = egui::Rect::from_min_size(egui::pos2(590.0, 790.0), egui::vec2(100.0, 40.0));
    let clamped = clamp_to_page(rect, page_size);
    assert!(clamped.min.x >= 0.0 && clamped.min.y >= 0.0);
    assert!(clamped.max.x <= page_size.x && clamped.max.y <= page_size.y);
}

#[test]
fn clamp_is_noop_when_already_inside() {
    let page_size = egui::vec2(600.0, 800.0);
    let rect = egui::Rect::from_min_size(egui::pos2(50.0, 50.0), egui::vec2(100.0, 40.0));
    assert_eq!(clamp_to_page(rect, page_size), rect);
}

#[test]
fn move_element_clamps_result() {
    let mut layer = OverlayLayer { elements: vec![] };
    let id = Uuid::new_v4();
    layer.add(OverlayElement::TextBox(TextBoxElement { id, page_index: 0, rect: egui::Rect::from_min_size(egui::pos2(10.0,10.0), egui::vec2(100.0,40.0)), text: String::new() }));
    layer.move_element(id, egui::pos2(590.0, 790.0), egui::vec2(600.0, 800.0));
    let rect = layer.elements[0].rect();
    assert!(rect.max.x <= 600.0 && rect.max.y <= 800.0);
}
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test overlay_test -- --nocapture`
Expected: FAIL

- [ ] **Step 3: Implement `clamp_to_page` and `OverlayLayer` in `src/overlay.rs`**

`clamp_to_page` shifts the rect's origin so `rect.min` and `rect.max` both fall within `[0, page_size]` on each axis, preserving the rect's width/height (if the element is larger than the page on an axis, clamp `min` to 0 on that axis). Add an `OverlayElement::rect()` accessor method used by the tests above.

- [ ] **Step 4: Run tests to verify they pass**

Run: `cargo test overlay_test -- --nocapture`
Expected: PASS

- [ ] **Step 5: Wire placement, selection, drag-move, corner-handle resize, and delete into the canvas UI**

Add a toolbar "Add text box" tool-mode toggle. While active, clicking empty page space (where Task 4's `hit_test` found no field) creates a `TextBoxElement` with a default size, selects it, and switches focus to an inline `egui::TextEdit`. Clicking an existing element selects it (draw a selection outline + corner drag handles); dragging the body moves it via `move_element`; dragging a corner resizes it via `resize_element`; a Delete key or a small "×" button on the selection removes it via `remove`.

- [ ] **Step 6: Manual verification**

Run: `cargo run -- tests/fixtures/sample_plain.pdf`
Expected: "Add text box" lets you click anywhere on the page to drop a box and type into it; dragging it near the page edge keeps it fully on the page; you can select, move, resize, and delete it.

- [ ] **Step 7: Commit**

```bash
git add src/overlay.rs src/app.rs src/ui/canvas.rs tests/overlay_test.rs
git commit -m "feat: free-form text box placement, selection, move, resize, delete"
```

---

### Task 6: Annotation round-trip (save-time marker)

**Files:**
- Create: `src/annotations.rs`
- Test: `tests/annotations_test.rs`

**Interfaces:**
- Consumes: `OverlayElement`, `TextBoxElement` (Task 5); `SignatureElement` (stubbed in Task 5, completed in Task 8); `PdfDocumentState` (Task 2).
- Produces:
  - `pub const APP_MARKER: &str = "BrightechPDFUtility"`
  - `pub fn write_overlay_to_annotations(doc: &mut PdfDocumentState, page_index: usize, elements: &[OverlayElement]) -> Result<(), DocumentError>`
  - `pub fn read_overlay_from_annotations(doc: &PdfDocumentState, page_index: usize) -> Vec<OverlayElement>`

- [ ] **Step 1: Write a failing round-trip test**

```rust
#[test]
fn text_box_round_trips_through_annotations() {
    let mut doc = PdfDocumentState::open(Path::new("tests/fixtures/sample_plain.pdf")).unwrap();
    let original = vec![OverlayElement::TextBox(TextBoxElement {
        id: Uuid::new_v4(), page_index: 0,
        rect: egui::Rect::from_min_size(egui::pos2(20.0, 30.0), egui::vec2(120.0, 18.0)),
        text: "Hello".into(),
    })];
    write_overlay_to_annotations(&mut doc, 0, &original).unwrap();
    let read_back = read_overlay_from_annotations(&doc, 0);
    assert_eq!(read_back.len(), 1);
    match &read_back[0] {
        OverlayElement::TextBox(tb) => assert_eq!(tb.text, "Hello"),
        _ => panic!("expected a text box"),
    }
}

#[test]
fn foreign_annotations_without_app_marker_are_ignored() {
    // sample_plain.pdf has no pre-existing annotations; this documents the
    // contract that only APP_MARKER-tagged annotations are read back as
    // OverlayElements — any other annotation subtype/author is skipped.
    let doc = PdfDocumentState::open(Path::new("tests/fixtures/sample_plain.pdf")).unwrap();
    assert!(read_overlay_from_annotations(&doc, 0).is_empty());
}
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test annotations_test -- --nocapture`
Expected: FAIL

- [ ] **Step 3: Implement `write_overlay_to_annotations` and `read_overlay_from_annotations` in `src/annotations.rs`**

For each `TextBoxElement`, create a `FreeText` annotation at `rect` with contents set to `text` and its annotation author/title field (`/T`) set to `APP_MARKER`. For each `SignatureElement`, create a stamp annotation at `rect` with an embedded image XObject from `image`, title also set to `APP_MARKER`. `read_overlay_from_annotations` iterates the page's annotations, keeps only those whose title equals `APP_MARKER`, and reconstructs a `TextBoxElement` (from a `FreeText` subtype) or `SignatureElement` (from a stamp subtype with an image) per annotation, generating a fresh `Uuid` for each (identity is not required to survive a round trip, only content/position).

- [ ] **Step 4: Run tests to verify they pass**

Run: `cargo test annotations_test -- --nocapture`
Expected: PASS

- [ ] **Step 5: Commit**

```bash
git add src/annotations.rs tests/annotations_test.rs
git commit -m "feat: round-trip overlay elements through marked PDF annotations"
```

---

### Task 7: Signature library — manifest, image upload with background removal, typed signatures

**Files:**
- Create: `src/signatures.rs`, `src/signature_image.rs`, `src/ui/signature_panel.rs`
- Test: `tests/signatures_test.rs`, `tests/signature_image_test.rs`

**Interfaces:**
- Produces:
  - `pub enum SignatureSource { Image { file_name: String }, Typed { text: String, font_id: String } }`
  - `pub struct SignatureEntry { pub id: Uuid, pub name: String, pub source: SignatureSource, pub created: String }`
  - `pub struct SignatureLibrary { pub entries: Vec<SignatureEntry> }` with `pub fn load(dir: &Path) -> SignatureLibrary`, `pub fn save(&self, dir: &Path) -> std::io::Result<()>`, `pub fn add_image(&mut self, dir: &Path, name: &str, source_image_path: &Path) -> Result<Uuid, SignatureError>`, `pub fn add_typed(&mut self, dir: &Path, name: &str, text: &str, font_id: &str) -> Uuid`, `pub fn remove(&mut self, id: Uuid)`, `pub fn image_path_for(&self, dir: &Path, id: Uuid) -> Option<PathBuf>`
  - `pub fn strip_light_background(img: image::RgbaImage, threshold: u8) -> image::RgbaImage` in `src/signature_image.rs`

- [ ] **Step 1: Write a failing test for background stripping**

```rust
#[test]
fn near_white_pixels_become_transparent() {
    let mut img = image::RgbaImage::new(2, 1);
    img.put_pixel(0, 0, image::Rgba([250, 250, 250, 255])); // near-white background
    img.put_pixel(1, 0, image::Rgba([10, 10, 10, 255]));    // dark ink
    let out = strip_light_background(img, 240);
    assert_eq!(out.get_pixel(0, 0)[3], 0);   // background now transparent
    assert_eq!(out.get_pixel(1, 0)[3], 255); // ink stays opaque
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test near_white_pixels -- --nocapture`
Expected: FAIL

- [ ] **Step 3: Implement `strip_light_background` in `src/signature_image.rs`**

For each pixel, if `r`, `g`, and `b` are all `>= threshold`, set alpha to 0; otherwise leave the pixel unchanged.

- [ ] **Step 4: Run test to verify it passes**

Run: `cargo test near_white_pixels -- --nocapture`
Expected: PASS

- [ ] **Step 5: Write failing tests for `SignatureLibrary`**

```rust
#[test]
fn add_and_reload_image_signature_round_trips() {
    let tmp = tempfile::tempdir().unwrap();
    let source = tmp.path().join("sig.png");
    image::RgbaImage::new(10, 10).save(&source).unwrap();
    let mut lib = SignatureLibrary::load(tmp.path());
    let id = lib.add_image(tmp.path(), "My signature", &source).unwrap();
    lib.save(tmp.path()).unwrap();

    let reloaded = SignatureLibrary::load(tmp.path());
    assert!(reloaded.entries.iter().any(|e| e.id == id && e.name == "My signature"));
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
    assert!(matches!(&entry.source, SignatureSource::Typed { text, font_id }
        if text == "Jane Doe" && font_id == "script-font-1"));
}

#[test]
fn remove_deletes_entry() {
    let tmp = tempfile::tempdir().unwrap();
    let mut lib = SignatureLibrary::load(tmp.path());
    let id = lib.add_typed(tmp.path(), "Typed", "Jane Doe", "script-font-1");
    lib.remove(id);
    assert!(lib.entries.is_empty());
}
```

- [ ] **Step 6: Run tests to verify they fail**

Run: `cargo test signatures_test -- --nocapture`
Expected: FAIL

- [ ] **Step 7: Implement `SignatureLibrary` in `src/signatures.rs`**

`load`/`save` serialize `entries` to/from `<dir>/signatures.json` via `serde_json`. `add_image` loads `source_image_path` with the `image` crate, runs it through `strip_light_background` (threshold `240`), writes the result as `<dir>/<id>.png`, and appends a `SignatureEntry { source: SignatureSource::Image { file_name: "<id>.png" }, .. }`. `add_typed` just appends an entry with `SignatureSource::Typed`; no file is written (rendering happens at placement time in Task 8). `image_path_for` resolves `SignatureSource::Image`'s `file_name` against `dir`.

- [ ] **Step 8: Run tests to verify they pass**

Run: `cargo test signatures_test -- --nocapture`
Expected: PASS

- [ ] **Step 9: Build the signature-panel UI**

A side panel listing `SignatureLibrary.entries` (name + thumbnail), with "Add from image..." (native file picker via `rfd`, then `add_image`), "Add typed..." (text input + font dropdown, then `add_typed`), and a delete button per entry calling `remove`. The library's storage directory is `~/Library/Application Support/<bundle id>/signatures/`, resolved once in `src/app.rs` at startup.

- [ ] **Step 10: Manual verification**

Run: `cargo run`
Expected: uploading a signature photo on a white background shows a thumbnail with the background gone; adding a typed signature shows it rendered in the chosen font; both persist across an app restart; delete removes an entry.

- [ ] **Step 11: Commit**

```bash
git add src/signatures.rs src/signature_image.rs src/ui/signature_panel.rs tests/signatures_test.rs tests/signature_image_test.rs
git commit -m "feat: signature library with image upload, background removal, and typed signatures"
```

---

### Task 8: Signature placement on page

**Files:**
- Modify: `src/overlay.rs` (complete `SignatureElement`, drop the Task 5 stub)
- Modify: `src/app.rs`, `src/ui/canvas.rs`, `src/ui/signature_panel.rs`
- Test: `tests/overlay_test.rs` (extend)

**Interfaces:**
- Consumes: `SignatureLibrary::image_path_for` and typed-signature fields (Task 7); `OverlayLayer::add/move_element/resize_element` and `clamp_to_page` (Task 5).
- Produces: `SignatureElement { pub id: Uuid, pub page_index: usize, pub rect: egui::Rect, pub image: egui::ColorImage }` (finalized); `pub fn render_signature_entry(entry: &SignatureEntry, dir: &Path) -> egui::ColorImage` in `src/signatures.rs` (rasterizes an image entry from disk, or renders a typed entry's text in its font to an `egui::ColorImage`).

- [ ] **Step 1: Write a failing test for placing a signature clamped to the page**

```rust
#[test]
fn placed_signature_is_clamped_like_a_text_box() {
    let mut layer = OverlayLayer { elements: vec![] };
    let id = Uuid::new_v4();
    let img = egui::ColorImage::new([40, 20], egui::Color32::TRANSPARENT);
    layer.add(OverlayElement::Signature(SignatureElement {
        id, page_index: 0,
        rect: egui::Rect::from_min_size(egui::pos2(590.0, 790.0), egui::vec2(40.0, 20.0)),
        image: img,
    }));
    layer.move_element(id, egui::pos2(590.0, 790.0), egui::vec2(600.0, 800.0));
    let rect = layer.elements[0].rect();
    assert!(rect.max.x <= 600.0 && rect.max.y <= 800.0);
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test placed_signature_is_clamped -- --nocapture`
Expected: FAIL (compile error: `SignatureElement` fields don't yet match, or stub not wired to `OverlayLayer`)

- [ ] **Step 3: Finalize `SignatureElement` and `render_signature_entry`**

Replace Task 5's stub with the real struct above. `render_signature_entry` loads the PNG from `image_path_for` for an image entry, or rasterizes the typed text in the chosen font at a fixed point size for a typed entry, returning an `egui::ColorImage` either way.

- [ ] **Step 4: Run test to verify it passes**

Run: `cargo test placed_signature_is_clamped -- --nocapture`
Expected: PASS

- [ ] **Step 5: Wire drag-from-library-to-page placement into the UI**

From the signature panel, dragging an entry onto the canvas (or selecting it then clicking a page location) creates a `SignatureElement` via `render_signature_entry` + `OverlayLayer::add`, selected and resizable/movable/deletable exactly like a text box (reusing Task 5's selection/drag/resize/delete code path).

- [ ] **Step 6: Manual verification**

Run: `cargo run -- tests/fixtures/sample_plain.pdf`
Expected: a saved signature can be placed on the page, dragged, resized, and deleted; dropping it near an edge keeps it fully on the page.

- [ ] **Step 7: Commit**

```bash
git add src/overlay.rs src/signatures.rs src/app.rs src/ui/canvas.rs src/ui/signature_panel.rs tests/overlay_test.rs
git commit -m "feat: place signatures from the library onto the page"
```

---

### Task 9: Save / Save As

**Files:**
- Create: `src/save.rs`
- Modify: `src/app.rs`
- Test: `tests/save_test.rs`

**Interfaces:**
- Consumes: `PdfDocumentState` (Task 2), `set_field_value`/`fields_on_page` (Task 4), `OverlayLayer` (Tasks 5/8), `write_overlay_to_annotations`/`read_overlay_from_annotations` (Task 6).
- Produces: `pub fn save(doc: &mut PdfDocumentState, overlay: &OverlayLayer, path: &Path) -> Result<(), DocumentError>` — writes all per-page `OverlayElement`s as annotations, then writes the document to `path`. `src/app.rs` adds `current_file_path: Option<PathBuf>`; "Save" calls `save(..., current_file_path)`, "Save As" opens an `rfd::FileDialog::save_file` and calls `save(..., chosen_path)` then updates `current_file_path`.

- [ ] **Step 1: Write a failing end-to-end save/reopen test**

```rust
#[test]
fn saved_field_value_and_text_box_are_both_readable_after_reopen() {
    let tmp = tempfile::tempdir().unwrap();
    let out_path = tmp.path().join("out.pdf");

    let mut doc = PdfDocumentState::open(Path::new("tests/fixtures/sample_form.pdf")).unwrap();
    set_field_value(&mut doc, "applicant_name", "Jane Doe").unwrap();
    let mut overlay = OverlayLayer { elements: vec![] };
    overlay.add(OverlayElement::TextBox(TextBoxElement {
        id: Uuid::new_v4(), page_index: 0,
        rect: egui::Rect::from_min_size(egui::pos2(20.0, 30.0), egui::vec2(120.0, 18.0)),
        text: "Note".into(),
    }));
    save(&mut doc, &overlay, &out_path).unwrap();

    let reopened = PdfDocumentState::open(&out_path).unwrap();
    let fields = fields_on_page(&reopened, 0);
    assert!(fields.iter().any(|f| f.name == "applicant_name"));
    let elements = read_overlay_from_annotations(&reopened, 0);
    assert!(elements.iter().any(|e| matches!(e, OverlayElement::TextBox(tb) if tb.text == "Note")));
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test save_test -- --nocapture`
Expected: FAIL

- [ ] **Step 3: Implement `save` in `src/save.rs`**

Group `overlay.elements` by `page_index`, call `write_overlay_to_annotations` per page, then call pdfium's document-save (`PdfDocument::save_to_file` or equivalent in `pdfium-render`) to `path`. Field values are already live on the in-memory `PdfDocumentState` from `set_field_value` calls made during editing, so no extra step is needed for those — the document save persists them as part of the same write.

- [ ] **Step 4: Run test to verify it passes**

Run: `cargo test save_test -- --nocapture`
Expected: PASS

- [ ] **Step 5: Wire Save / Save As into the app toolbar**

"Save" is disabled (greyed out) when `current_file_path` is `None`; otherwise it calls `save` with that path. "Save As" always opens the native save dialog.

- [ ] **Step 6: Manual verification**

Run: `cargo run -- tests/fixtures/sample_form.pdf`, fill the field, add a text box, Save As to a new path, then File → Open that new path.
Expected: the field value and text box are both present and still editable/movable in the reopened file.

- [ ] **Step 7: Commit**

```bash
git add src/save.rs src/app.rs tests/save_test.rs
git commit -m "feat: Save and Save As, preserving editable annotations"
```

---

### Task 10: Apply (flatten)

**Files:**
- Create: `src/apply.rs`
- Modify: `src/app.rs`
- Test: `tests/apply_test.rs`

**Interfaces:**
- Consumes: `PdfDocumentState` (Task 2); `fields_on_page` (Task 4); `read_overlay_from_annotations` (Task 6).
- Produces: `pub fn flatten(doc: &mut PdfDocumentState) -> Result<(), DocumentError>`; `src/app.rs` adds `applied: bool` to the app's per-document state, set to `true` after a successful Apply.

- [ ] **Step 1: Write a failing test that flatten removes fields and annotations**

```rust
#[test]
fn flatten_removes_fields_and_app_annotations() {
    let mut doc = PdfDocumentState::open(Path::new("tests/fixtures/sample_form.pdf")).unwrap();
    set_field_value(&mut doc, "applicant_name", "Jane Doe").unwrap();
    let mut overlay = OverlayLayer { elements: vec![] };
    overlay.add(OverlayElement::TextBox(TextBoxElement {
        id: Uuid::new_v4(), page_index: 0,
        rect: egui::Rect::from_min_size(egui::pos2(20.0, 30.0), egui::vec2(120.0, 18.0)),
        text: "Note".into(),
    }));
    write_overlay_to_annotations(&mut doc, 0, &overlay.elements).unwrap();

    flatten(&mut doc).unwrap();

    assert!(fields_on_page(&doc, 0).is_empty());
    assert!(read_overlay_from_annotations(&doc, 0).is_empty());
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test apply_test -- --nocapture`
Expected: FAIL

- [ ] **Step 3: Implement `flatten` in `src/apply.rs`**

Call pdfium's `FPDFPage_Flatten` (via whatever `pdfium-render` exposes for it) on every page of the document. If `pdfium-render` doesn't expose it directly, call it through `pdfium-render`'s raw/FFI escape hatch.

- [ ] **Step 4: Run test to verify it passes**

Run: `cargo test apply_test -- --nocapture`
Expected: PASS

- [ ] **Step 5: Wire Apply into the toolbar, and gate editing tools on `applied`**

An "Apply" button calls `flatten`, then sets `applied = true` and shows a confirmation ("This document is now locked; Save or Save As to keep it."). While `applied` is `true`: the "Add text box" tool, the signature panel's drag-to-place, field click-to-edit, and all selection/move/resize/delete interactions are disabled (greyed out / inert), so no further edits can be made or silently lost — this is the Review Focus case for post-Apply editing.

- [ ] **Step 6: Manual verification**

Run: `cargo run -- tests/fixtures/sample_form.pdf`, fill a field, add a text box, click Apply, then try to click into a field or add another text box.
Expected: Apply locks the document and all editing tools become inert; Save/Save As afterward still work and produce a flattened file with no interactive fields when reopened.

- [ ] **Step 7: Commit**

```bash
git add src/apply.rs src/app.rs tests/apply_test.rs
git commit -m "feat: Apply flattens the document and locks further editing"
```

---

### Task 11: App packaging

**Files:**
- Modify: `packaging/Info.plist` (from Task 3)
- Create: `packaging/bundle.sh`
- Modify: `Cargo.toml` (add any packager config section if the chosen tool needs it)

**Interfaces:**
- Consumes: the compiled binary from `cargo build --release` for both `aarch64-apple-darwin` and `x86_64-apple-darwin`; `vendor/pdfium/<arch>/libpdfium.dylib` (Task 1); `packaging/Info.plist` (Task 3).
- Produces: `PDF Utility.app` at a path printed by `packaging/bundle.sh`.

- [ ] **Step 1: Write `packaging/bundle.sh`**

The script: builds release binaries for both `aarch64-apple-darwin` and `x86_64-apple-darwin` (`cargo build --release --target <triple>` for each, installing the targets via `rustup target add` first if missing), combines them into one universal binary with `lipo -create`, assembles `PDF Utility.app/Contents/{MacOS,Resources,Frameworks}`, copies the universal binary into `MacOS/`, copies `packaging/Info.plist` into `Contents/`, copies both `vendor/pdfium/*/libpdfium.dylib` arch slices combined via `lipo` into `Contents/Frameworks/libpdfium.dylib`, fixes up the binary's load path for the dylib with `install_name_tool` if needed, then ad-hoc codesigns both the framework and the app bundle (`codesign --force --deep --sign - "PDF Utility.app"`).

- [ ] **Step 2: Run the script**

Run: `./packaging/bundle.sh`
Expected: exits 0 and prints the path to `PDF Utility.app`.

- [ ] **Step 3: Manual verification — launch via Finder**

Run: `open "PDF Utility.app"` (or double-click it in Finder)
Expected: the app launches without a Gatekeeper block (ad-hoc signed, run locally) and shows the same window as `cargo run`.

- [ ] **Step 4: Manual verification — Finder "Open With" and double-click**

In Finder, right-click `tests/fixtures/sample_plain.pdf` → Open With → "PDF Utility.app" (selecting "Always Open With" if offered).
Expected: the app launches with that file already open; subsequent double-clicks on a `.pdf` open it in this app.

- [ ] **Step 5: Commit**

```bash
git add packaging/
git commit -m "feat: package a universal, ad-hoc-signed .app bundle"
```

---

## Self-Review Notes

- **Spec coverage:** view (Tasks 2–3), AcroForm fill (Task 4), free-form text boxes (Task 5), signature library incl. background removal (Task 7), signature placement (Task 8), Save/Save As with editable round-trip (Tasks 6, 9), Apply/flatten-and-lock (Task 10), Finder double-click/Open-With (Tasks 3, 11), offline/no-separate-install packaging (Tasks 1, 11) are all covered.
- **Review Focus coverage:** corrupt PDF and password-protected PDF (Task 2), page-edge clamping (Task 5, reused by Task 8), non-text field click safety (Task 4), post-Apply editing lockout (Task 10) — all have an owning task and test/manual-verification step.
- **Type consistency:** `OverlayElement`/`TextBoxElement`/`SignatureElement` are introduced once (Task 5, stubbed for `SignatureElement`) and finalized once (Task 8); `PdfDocumentState`, `DocumentError`, `FormFieldHit`, `FieldKind` are each defined in exactly one task and reused by name everywhere after.

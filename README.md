<p align="center">
  <img src="assets/icon/icon.png" alt="Signet app icon" width="160">
</p>

# Signet

A small, native macOS desktop app for viewing, filling, and signing PDFs —
nothing more. Built to be a personal, fully offline alternative to Adobe
Acrobat: no installer, no background daemons, no subscription, no telemetry.
Pure Rust UI ([`egui`](https://github.com/emilk/egui)/`eframe`), with
Google's [pdfium](https://pdfium.googlesource.com/pdfium/) linked in as a
vendored library for PDF rendering and editing — nothing is installed
outside the app bundle, and the app never makes network calls.

## Features

- **View** — open a PDF, scroll and zoom through its pages.
- **Fill** — click into a real AcroForm field and type a value. If a page
  has no form field, drop a free-form text box anywhere and type into it.
- **Sign** — build a personal signature library from an uploaded image
  (background automatically stripped so only the ink shows) or typed text
  rendered in a bundled handwriting font. Drag a saved signature onto a
  page and resize it like a stamp.
- **Save / Save As** — Save overwrites the current file; Save As writes a
  copy. Either way, the document stays editable: reopening it later still
  shows your text boxes and signatures as movable, editable elements.
- **Apply** — a separate, explicit action that permanently flattens all
  additions into the page content and locks the document from further
  editing, in this app or any other PDF tool.
- Opening a PDF works both from File → Open and from Finder — double-click,
  or right-click → Open With — via a small `NSApplicationDelegate` that
  catches the file-open event Finder sends (macOS never passes this as a
  command-line argument the way a direct launch does).

## Building and running from source

Requires Rust (via [rustup](https://rustup.rs)) and the Xcode command line
tools (`xcode-select --install`) for `lipo`/`codesign`.

```bash
cargo build
cargo run -- path/to/file.pdf   # optional: opens a file on launch
```

## Packaging a `.app` / `.dmg`

```bash
./packaging/bundle.sh    # builds dist/Signet.app (universal binary)
./packaging/make_dmg.sh  # also wraps it into dist/Signet.dmg
```

`make_dmg.sh` calls `bundle.sh` internally, so running it alone produces
both. The resulting app is ad-hoc code-signed (not notarized), so the first
launch requires right-click → **Open** (or approving it under System
Settings → Privacy & Security → "Open Anyway") to get past Gatekeeper.
After that, double-click and Finder "Open With" work normally.

To install: drag `dist/Signet.app` into `/Applications`, or open
`dist/Signet.dmg` and drag the app onto the `Applications` shortcut
inside it.

## Running tests

```bash
cargo test
```

## Project layout

- `src/` — application source (`app.rs` is the main UI/state; `document.rs`,
  `form_fields.rs`, `overlay.rs`, `annotations.rs`, `apply.rs`, `save.rs`,
  `signatures.rs` split out the PDF/document logic; `macos_open.rs` handles
  Finder-initiated opens).
- `packaging/` — `Info.plist`, `bundle.sh`, `make_dmg.sh`, and the icon
  generator script.
- `assets/` — bundled font (typed signatures) and app icon (`icon/AppIcon.icns`
  for the app bundle, `icon/icon.png` for display elsewhere, e.g. this README).
- `vendor/pdfium/` — prebuilt pdfium dynamic libraries, loaded at runtime
  from inside the app bundle.
- `tests/fixtures/` — sample PDFs used by the test suite.

## Known limitations

- Password-protected / encrypted PDFs are not supported — opening one
  returns a clear error rather than prompting for a password.
- ⌘Q quits without prompting to save unsaved changes (macOS routes Quit
  around the app's own close-confirmation logic). Use the window's close
  button, or save explicitly, before quitting if you have unsaved edits.
- No merge/split/reorder, OCR, redaction, or encryption features — this
  tool is intentionally scoped to view/fill/sign only.

## AI-assisted development

This project's code, tests, and packaging scripts were written largely by
AI (Claude, via Claude Code), working from a human-authored design and
implementation plan, with human review and direction throughout.

## License

MIT — see [LICENSE](LICENSE).

This project bundles two third-party assets, under their own licenses:
- `vendor/pdfium/` — prebuilt pdfium binaries (BSD-3-Clause; see
  `vendor/pdfium/LICENSE`).
- `assets/fonts/ShadowsIntoLight.ttf` — "Shadows Into Light" by Kimberly
  Geswein, SIL Open Font License 1.1 (see `assets/fonts/OFL.txt`).

# PDF Utility — Design Document

Date: 2026-10-17

## Intent and why

The user wants a personal macOS desktop tool to view PDFs, fill them in, and
sign them — nothing more. The motivation is explicit: they want to own and
be able to customize the tool themselves, and they specifically want to
avoid installing Adobe Acrobat, which bundles background daemons, licensing
services, and update agents they don't want on their machine. This is a
single-user, local-first tool, not a product for distribution.

## Goals

- View PDF documents: open a file, scroll and zoom through its pages.
- Fill existing AcroForm fields: when a PDF has real form fields, clicking
  into one lets the user type a value directly into that field.
- Fill PDFs without form fields: the user can drop a free-form text box
  anywhere on a page and type a value into it, when no real form field
  exists at that location.
- Sign documents using a personal signature library:
  - Signatures are either an uploaded image of the user's actual signature,
    or a typed name rendered in a bundled handwriting/script font.
  - Uploaded signature images are automatically processed to strip the
    light/white background, so only the ink shows and the signature reads
    as if written directly on the page rather than stamping an opaque
    rectangle over existing content.
  - Signatures are saved once and persist across app launches, reusable on
    any PDF opened later.
  - A saved signature can be dragged onto a page and resized like a stamp.
- Save / Save As, with standard semantics:
  - **Save** overwrites the currently open file.
  - **Save As** writes a new file, leaving the original untouched.
  - After either, the document remains editable: reopening it in this app
    later still shows filled text boxes and placed signatures as movable,
    editable elements.
- Apply: an explicit, separate action that flattens all additions (filled
  field values, free-form text boxes, placed signatures) permanently into
  the page content. Once applied, the document is locked from further
  editing, in this app or any other PDF tool — the content is baked into
  the page itself, not stored as editable objects. Apply changes in-memory
  state; the user still uses Save/Save As afterward to persist the
  flattened document to disk.
- Opening files:
  - File → Open is always available from within the app.
  - Best-effort support for opening via Finder double-click/"Open With",
    by registering the app as a PDF-capable application. macOS requires the
    user to select this app at least once via Finder ("Open With" or
    "Always Open With") before double-click will route to it by default —
    the app cannot silently make itself the system default.

## Non-goals / anti-goals

Explicitly out of scope — adding any of these would be considered scope
creep, not a natural extension:

- Merging, splitting, reordering, or extracting pages.
- OCR, text extraction, or searchable-text conversion.
- Redaction tools.
- Encryption, password protection, or permissions management.
- Cloud sync, accounts, or any server-side component.
- Telemetry, crash reporting, or auto-update checks of any kind — the app
  must work fully offline and must not phone home.
- A full undo/redo history. Basic direct manipulation (move, resize,
  delete a placed text box or signature before saving) is in scope; a
  multi-step undo stack is not.
- A dedicated printing workflow (the user did not ask for one; macOS's
  own print dialog on an opened PDF is not something this app needs to
  build toward).
- App Store distribution, notarization for third-party distribution, or
  support for other users' Apple Developer accounts. A locally-run,
  ad-hoc-signed `.app` is sufficient.
- Exotic or uncommon form field types beyond what the chosen PDF engine
  exposes out of the box. Text fields are the primary target; checkboxes,
  radio buttons, and dropdowns should work if the engine supports them
  with little extra effort, but are not a focus of testing.
- Password-protected / encrypted PDFs. Opening one is not supported for
  now; this may be revisited later if it turns out to matter in practice.

## Constraints

- macOS only.
- Rust is the application language. Non-Rust compiled dependencies are
  acceptable when linked directly into the app's own binary (no separate
  installers, no standalone background processes, no network calls) —
  this preserves the "no Adobe-style bloat" goal without requiring a
  from-scratch PDF engine, which is not practical to build.
- No AGPL-licensed dependencies (rules out MuPDF), to keep the licensing
  posture simple for a tool the user may want to keep private or modify
  freely.
- No network access anywhere in the app.

## Decided how (builder should treat these as settled, not open questions)

- **UI toolkit**: `egui` + `eframe` — pure Rust, immediate-mode GUI, no
  webview and no HTML/CSS/JS anywhere in the stack.
- **PDF engine**: Google's pdfium, via the `pdfium-render` crate. Used for
  rendering pages, enumerating and filling AcroForm fields, creating new
  page objects (text, images) for free-form text boxes and signature
  placement, and saving/flattening documents. This is the one place the
  app depends on non-Rust compiled code; it is linked into the app's own
  binary, not installed or run separately.
- **Native file dialogs**: `rfd` crate, for File Open / Save As dialogs
  that look and behave like native macOS dialogs.
- **Signature library storage**: a directory under the app's Application
  Support folder (e.g. `~/Library/Application Support/<bundle id>/signatures/`)
  holding uploaded signature images plus a small JSON manifest (name,
  type — image or typed, font used if typed, created date). No database.
- **Typed-signature font**: one open-licensed handwriting/script font
  bundled with the app's resources. A specific font will be picked during
  implementation and can be swapped later on request.
- **Packaging**: build a standard `.app` bundle (via `cargo-bundle` or
  `cargo-packager`), ad-hoc code-signed (`codesign -s -`) so it runs
  locally without Gatekeeper complaints. `Info.plist` will declare
  `CFBundleDocumentTypes` / a `public.pdf` import so the OS knows this app
  can open PDFs, enabling the Finder "Open With" flow described above.
- **Architecture split**: all three features (view, fill, sign) operate on
  one in-memory document model backed by pdfium; there is no separate
  "editor" vs. "viewer" mode — placing a text box, filling a form field,
  and placing a signature are all just adding/editing page-level objects
  before an eventual Apply/Save.
- **How "editable after Save" is represented on disk**: un-applied
  additions are stored as standard PDF annotation objects — `FreeText`
  annotations for free-form text boxes, stamp annotations (with an
  embedded image or rendered-text appearance stream) for placed
  signatures, and ordinary AcroForm field value writes for filled form
  fields. This keeps a saved-but-not-applied file a plain, portable,
  standard PDF, openable in any PDF viewer (other tools just won't offer
  to re-edit the markup the way this app does) — the same model
  Preview.app's own markup tools use. No custom/proprietary encoding or
  sidecar metadata file.
- **How Apply flattens**: Apply uses pdfium's native page-flatten
  capability (`FPDFPage_Flatten`) to bake annotations and form field
  widgets into the page's content stream and strip the AcroForm/annotation
  dictionaries, producing an ordinary flattened PDF with no interactive
  elements left. This should be verified early in implementation against
  our specific annotation types (FreeText, stamps); if gaps turn up, a
  manual content-stream bake is the fallback, but is not expected to be
  needed.
- **How pdfium itself is obtained**: a prebuilt pdfium binary distribution
  (e.g. the widely-used `bblanchon/pdfium-binaries` releases) is vendored
  rather than built from source (building pdfium requires Chromium's
  GN/Ninja/depot_tools toolchain, which is impractical to stand up for
  this project). It's bundled as a dynamic library inside
  `<App>.app/Contents/Frameworks`, loaded at runtime by `pdfium-render`,
  and signed as part of normal app packaging. This is still fully
  self-contained inside the one `.app` — nothing is installed outside it,
  no separate installer or background process — which preserves the
  original goal of avoiding Adobe-style machine footprint.

- **Signature background removal**: on upload, process the image (via the
  `image` crate) to make light/white-ish pixels transparent — a simple
  luminance/chroma threshold is sufficient for typical signature photos
  on white or lightly-lined paper. The processed, transparent version is
  what's stored in the signature library and placed on pages.

## What's left to the builder

- Exact UI layout: toolbar contents, the signature-library management
  panel (add/rename/delete a saved signature), how placed elements are
  selected/resized/deleted before save.
- Specific crate versions and compatibility pinning between `egui`,
  `eframe`, and `pdfium-render`.
- Hit-testing and interaction code for clicking into form fields, placing
  and resizing free-form text boxes, and placing/resizing signature stamps.
- The specific bundled handwriting font (must confirm its license permits
  bundling and, if relevant, modification/redistribution).
- Build/packaging scripts to produce the final ad-hoc-signed `.app`.
- Whether to target Apple Silicon only or build a universal binary
  (Intel + Apple Silicon); Rust makes this close to free either way, so
  default to a universal binary unless told otherwise.

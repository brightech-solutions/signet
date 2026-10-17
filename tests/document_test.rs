use pdf_utility::document::{DocumentError, PdfDocumentState};
use std::path::Path;

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

//! AcroForm detection tests against the `forms.pdf` fixture.
//!
//! The fixture is produced by `tools/generate_fixtures.py` and contains one
//! widget of every common type, each with a real appearance stream. Tests skip
//! cleanly when the fixture has not been generated, mirroring `corpus.rs`.

use std::path::PathBuf;

use pdfreader_core::{FieldValue, FormFieldType};
use pdfreader_pdf::engine::{PdfEngine, PdfiumEngine};

/// Workspace root, derived from this crate's manifest directory.
fn workspace_root() -> PathBuf {
    // crates/pdf -> ../.. -> repo root
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("..")
        .join("..")
}

/// Locate the staged PDFium library, scanning `target/native/*`.
fn find_pdfium() -> Option<PathBuf> {
    let native = workspace_root().join("target").join("native");
    let entries = std::fs::read_dir(&native).ok()?;
    for entry in entries.flatten() {
        let dir = entry.path();
        if dir.join("pdfium.dll").exists()
            || dir.join("libpdfium.so").exists()
            || dir.join("libpdfium.dylib").exists()
        {
            return Some(dir);
        }
    }
    None
}

/// Skip a test with a clear message when its fixture is missing.
fn require_fixture(name: &str) -> PathBuf {
    let path = workspace_root()
        .join("target")
        .join("test-pdfs")
        .join(name);
    if !path.exists() {
        eprintln!(
            "SKIP: fixture {} not found (run: python3 tools/generate_fixtures.py target/test-pdfs)",
            path.display()
        );
        std::process::exit(0);
    }
    path
}

/// Open the form fixture, skipping when either PDFium or the fixture is absent.
fn open_forms_fixture() -> Option<(PdfiumEngine, pdfreader_pdf::DocumentHandle)> {
    let dir = find_pdfium()?;
    let mut engine = match PdfiumEngine::bind(Some(dir.as_path())) {
        Ok(engine) => engine,
        Err(error) => {
            eprintln!("SKIP: could not bind PDFium: {error}");
            return None;
        }
    };
    let path = require_fixture("forms.pdf");
    let (handle, _info) = engine
        .open(&path, None)
        .unwrap_or_else(|error| panic!("forms fixture should open: {error}"));
    Some((engine, handle))
}

/// Every declared widget must be found, with the right type and value.
#[test]
fn form_fields_are_detected_with_types_and_values() {
    let Some((mut engine, handle)) = open_forms_fixture() else {
        return;
    };

    let form = engine.form_fields(handle).expect("form read");
    assert_eq!(
        form.kind,
        pdfreader_core::FormKind::Acrobat,
        "fixture declares an AcroForm"
    );

    let kinds: Vec<(String, FormFieldType, &FieldValue)> = form
        .fields
        .iter()
        .map(|f| (f.name.clone(), f.kind, &f.value))
        .collect();

    let text = kinds
        .iter()
        .find(|(name, kind, _)| name == "FullName" && *kind == FormFieldType::Text)
        .unwrap_or_else(|| panic!("text field missing, got {kinds:?}"));
    assert_eq!(text.2, &FieldValue::Text("Jane Doe".into()));

    assert!(
        kinds
            .iter()
            .any(|(name, kind, value)| name == "Subscribe"
                && *kind == FormFieldType::CheckBox
                && **value == FieldValue::Checked(true)),
        "checked checkbox missing, got {kinds:?}"
    );

    // A radio group is two widgets sharing one name; both must be found.
    let radios: Vec<_> = kinds
        .iter()
        .filter(|(name, kind, _)| name == "Colour" && *kind == FormFieldType::RadioButton)
        .collect();
    assert_eq!(radios.len(), 2, "radio group widgets missing, got {kinds:?}");

    let combo = kinds
        .iter()
        .find(|(name, kind, _)| name == "Delivery" && *kind == FormFieldType::ComboBox)
        .unwrap_or_else(|| panic!("combo box missing, got {kinds:?}"));
    assert_eq!(combo.2, &FieldValue::Choice(Some("Courier".into())));

    assert!(
        kinds
            .iter()
            .any(|(name, kind, _)| name == "Toppings" && *kind == FormFieldType::ListBox),
        "list box missing, got {kinds:?}"
    );

    assert!(
        kinds
            .iter()
            .any(|(name, kind, _)| name == "Submit" && *kind == FormFieldType::PushButton),
        "push button missing, got {kinds:?}"
    );
}

/// Combo options must come through, so the UI can render a picker.
#[test]
fn combo_box_options_are_enumerated() {
    let Some((mut engine, handle)) = open_forms_fixture() else {
        return;
    };
    let form = engine.form_fields(handle).expect("form read");

    let delivery = form
        .fields
        .iter()
        .find(|f| f.name == "Delivery")
        .expect("combo box present");
    let labels: Vec<&str> = delivery.options.iter().map(|o| o.label.as_str()).collect();
    assert_eq!(labels, vec!["Email", "Courier", "Pickup"]);
    assert!(
        delivery
            .options
            .iter()
            .find(|o| o.label == "Courier")
            .is_some_and(|o| o.selected),
        "the current selection must be marked"
    );
}

/// Widget rectangles must land where the fixture drew them: everything the
/// overlay does depends on these coming back in unrotated PDF space.
#[test]
fn widget_rectangles_are_in_pdf_page_space() {
    let Some((mut engine, handle)) = open_forms_fixture() else {
        return;
    };
    let form = engine.form_fields(handle).expect("form read");
    assert!(!form.fields.is_empty());

    for field in &form.fields {
        assert_eq!(field.id.page, 0, "single-page fixture");
        let rect = field.rect;
        assert!(
            rect.min_x < rect.max_x && rect.min_y < rect.max_y,
            "{} has a degenerate rect {rect:?}",
            field.name
        );
        // The fixture draws every widget inside the media box, y up.
        assert!(rect.min_y > 0.0 && rect.max_y < 841.89, "{rect:?}");
        assert!(rect.min_x > 0.0 && rect.max_x < 595.28, "{rect:?}");
    }

    // The text field is drawn at x = 60..300, y = 728..748.
    let full_name = form
        .fields
        .iter()
        .find(|f| f.name == "FullName")
        .expect("text field present");
    assert!((full_name.rect.min_x - 60.0).abs() < 1.0);
    assert!((full_name.rect.max_x - 300.0).abs() < 1.0);
}

/// A document without a form must report an empty form, not an error: that is
/// the common case and the UI depends on telling it apart from "not read yet".
#[test]
fn a_document_without_a_form_reports_an_empty_one() {
    let Some(dir) = find_pdfium() else {
        eprintln!("SKIP: no PDFium library staged under target/native");
        return;
    };
    let mut engine = PdfiumEngine::bind(Some(dir.as_path())).expect("bind");
    let path = require_fixture("small-3p.pdf");
    let (handle, _info) = engine.open(&path, None).expect("open");

    let form = engine.form_fields(handle).expect("form read");
    assert_eq!(form.kind, pdfreader_core::FormKind::None);
    assert!(form.is_empty());
}

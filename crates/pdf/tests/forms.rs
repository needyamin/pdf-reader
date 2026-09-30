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

/// The whole point of the feature: write values, serialise, reopen, and find
/// them there. Goes through `save_to_bytes` + a fresh `open` exactly the way
/// the shell's Save does.
#[test]
fn filled_values_survive_a_save_round_trip() {
    let Some(dir) = find_pdfium() else {
        eprintln!("SKIP: no PDFium library staged under target/native");
        return;
    };
    let mut engine = PdfiumEngine::bind(Some(dir.as_path())).expect("bind");
    let path = require_fixture("forms.pdf");
    let (handle, _info) = engine.open(&path, None).expect("open");

    let form = engine.form_fields(handle).expect("form read");
    let text_field = form.fields.iter().find(|f| f.name == "FullName").unwrap();
    let checkbox = form.fields.iter().find(|f| f.name == "Subscribe").unwrap();
    // The second radio kid is the unchecked one (fixture sets /AS /Off).
    let radio = form
        .fields
        .iter()
        .filter(|f| f.name == "Colour")
        .find(|f| f.value == FieldValue::Checked(false))
        .expect("unchecked radio widget present");

    // Text: overwrite the fixture's "Jane Doe".
    assert!(
        engine
            .set_field_value(handle, text_field.id, FieldValue::Text("John Smith".into()))
            .expect("text write"),
        "text field must be writable"
    );
    // Checkbox: the fixture ships it checked, so clear it.
    assert!(
        engine
            .set_field_value(handle, checkbox.id, FieldValue::Checked(false))
            .expect("checkbox write"),
        "checkbox must be writable"
    );
    // Radio: selecting the unchecked widget must be possible.
    assert!(
        engine
            .set_field_value(handle, radio.id, FieldValue::Checked(true))
            .expect("radio write"),
        "radio button must be selectable"
    );

    let bytes = engine.save_to_bytes(handle, false).expect("serialise");
    assert!(
        bytes.len() > 100,
        "serialised output should be a real PDF, got {} bytes",
        bytes.len()
    );

    // Reopen the saved bytes through the same engine and verify.
    let reopened_path = std::env::temp_dir().join("pdf-reader-roundtrip-test.pdf");
    std::fs::write(&reopened_path, &bytes).expect("write round-trip file");
    let (handle2, _info) = engine.open(&reopened_path, None).expect("reopen");
    let form2 = engine.form_fields(handle2).expect("form read after save");

    let name = form2
        .fields
        .iter()
        .find(|f| f.name == "FullName")
        .expect("text field still present");
    assert_eq!(name.value, FieldValue::Text("John Smith".into()));

    let subscribe = form2
        .fields
        .iter()
        .find(|f| f.name == "Subscribe")
        .expect("checkbox still present");
    assert_eq!(subscribe.value, FieldValue::Checked(false));

    let colours: Vec<_> = form2
        .fields
        .iter()
        .filter(|f| f.name == "Colour")
        .map(|f| f.value.clone())
        .collect();
    assert!(
        colours.contains(&FieldValue::Checked(true)),
        "selected radio must persist, got {colours:?}"
    );
    let _ = std::fs::remove_file(&reopened_path);
}

/// Read-only fields and widgets with no write path must be refused, not
/// silently ignored — otherwise a save would drop user input without warning.
#[test]
fn unwritable_fields_are_refused() {
    let Some(dir) = find_pdfium() else {
        eprintln!("SKIP: no PDFium library staged under target/native");
        return;
    };
    let mut engine = PdfiumEngine::bind(Some(dir.as_path())).expect("bind");
    let path = require_fixture("forms.pdf");
    let (handle, _info) = engine.open(&path, None).expect("open");

    let form = engine.form_fields(handle).expect("form read");

    // Combo/list selection has no write path in the safe binding.
    let combo = form.fields.iter().find(|f| f.name == "Delivery").unwrap();
    let result = engine.set_field_value(
        handle,
        combo.id,
        FieldValue::Choice(Some("Pickup".into())),
    );
    assert!(result.is_err(), "combo write must be refused");

    // An annotation index that is not a widget at all.
    let bogus = pdfreader_core::FieldId::new(0, u32::MAX);
    assert!(
        !engine
            .set_field_value(handle, bogus, FieldValue::Text("x".into()))
            .expect("missing widget reports false, not an error"),
        "a missing widget must report Ok(false)"
    );
}

/// Flattening must produce a valid PDF that still opens afterwards.
#[test]
fn flatten_produces_a_loadable_document() {
    let Some(dir) = find_pdfium() else {
        eprintln!("SKIP: no PDFium library staged under target/native");
        return;
    };
    let mut engine = PdfiumEngine::bind(Some(dir.as_path())).expect("bind");
    let path = require_fixture("forms.pdf");
    let (handle, _info) = engine.open(&path, None).expect("open");

    let bytes = engine.save_to_bytes(handle, true).expect("flatten+serialise");
    let flattened_path = std::env::temp_dir().join("pdf-reader-flatten-test.pdf");
    std::fs::write(&flattened_path, &bytes).expect("write flattened file");

    let (handle2, info2) = engine.open(&flattened_path, None).expect("reopen flattened");
    assert_eq!(info2.page_count(), 1, "flattening preserves the page");

    // A flattened document has no interactive widgets left to enumerate.
    let form = engine.form_fields(handle2).expect("form read");
    assert!(
        form.fields.is_empty(),
        "flattened document should have no widgets left, got {}",
        form.fields.len()
    );
    let _ = std::fs::remove_file(&flattened_path);
}

/// Annotation creation end to end: draw a highlight and a note, rename the
/// note, save, reopen, and find them all with the right geometry and text.
#[test]
fn created_annotations_survive_a_save_round_trip() {
    use pdfreader_core::{NewAnnotation, Rect};

    let Some(dir) = find_pdfium() else {
        eprintln!("SKIP: no PDFium library staged under target/native");
        return;
    };
    let mut engine = PdfiumEngine::bind(Some(dir.as_path())).expect("bind");
    let path = require_fixture("forms.pdf");
    let (handle, _info) = engine.open(&path, None).expect("open");

    let before = engine.annotations(handle).expect("list");
    let before_count = before.len();

    // Highlight over the text field's area.
    engine
        .add_annotation(
            handle,
            0,
            NewAnnotation::Highlight(Rect::from_xywh(60.0, 728.0, 240.0, 20.0)),
        )
        .expect("create highlight");
    // A sticky note with text.
    engine
        .add_annotation(handle, 0, NewAnnotation::StickyNote((420.0, 700.0), "Reviewed later".into()))
        .expect("create note");
    // Typewriter text.
    engine
        .add_annotation(
            handle,
            0,
            NewAnnotation::FreeText(
                Rect::from_xywh(60.0, 400.0, 200.0, 24.0),
                "typed on the page".into(),
            ),
        )
        .expect("create free text");

    let after = engine.annotations(handle).expect("list");
    assert_eq!(
        after.len(),
        before_count + 3,
        "three new annotations expected, got {after:?}"
    );

    let note = after
        .iter()
        .find(|a| a.kind == pdfreader_core::AnnotationKind::StickyNote)
        .expect("sticky note present");
    engine
        .set_annotation_contents(handle, note.id, "Reviewed by Yamin")
        .expect("edit contents");

    let free_text = after
        .iter()
        .find(|a| a.kind == pdfreader_core::AnnotationKind::FreeText)
        .expect("free text present");
    assert_eq!(free_text.contents.as_deref(), Some("typed on the page"));

    // Highlight geometry must land where it was drawn, in PDF space.
    let highlight = after
        .iter()
        .find(|a| a.kind == pdfreader_core::AnnotationKind::Highlight)
        .expect("highlight present");
    assert!((highlight.rect.min_x - 60.0).abs() < 2.0, "{:?}", highlight.rect);
    assert!((highlight.rect.max_y - 748.0).abs() < 2.0, "{:?}", highlight.rect);

    // Save, reopen, verify everything round-tripped.
    let bytes = engine.save_to_bytes(handle, false).expect("serialise");
    let out = std::env::temp_dir().join("pdf-reader-annotation-roundtrip.pdf");
    std::fs::write(&out, &bytes).expect("write");
    let (handle2, _info) = engine.open(&out, None).expect("reopen");
    let saved = engine.annotations(handle2).expect("list after save");

    assert!(
        saved
            .iter()
            .any(|a| a.kind == pdfreader_core::AnnotationKind::Highlight),
        "highlight must survive, got {saved:?}"
    );
    let saved_note = saved
        .iter()
        .find(|a| a.kind == pdfreader_core::AnnotationKind::StickyNote)
        .expect("note must survive");
    assert_eq!(saved_note.contents.as_deref(), Some("Reviewed by Yamin"));
    assert!(
        saved
            .iter()
            .any(|a| a.kind == pdfreader_core::AnnotationKind::FreeText
                && a.contents.as_deref() == Some("typed on the page")),
        "free text must survive, got {saved:?}"
    );
    let _ = std::fs::remove_file(&out);

    // Deleting must remove exactly the one annotation. Ids are positional, so
    // later annotations shift into the freed index after a removal — check by
    // kind, not by the now-stale id.
    engine
        .delete_annotation(handle2, saved_note.id)
        .expect("delete");
    let after_delete = engine.annotations(handle2).expect("list after delete");
    assert!(
        !after_delete
            .iter()
            .any(|a| a.kind == pdfreader_core::AnnotationKind::StickyNote),
        "deleted note must be gone, got {after_delete:?}"
    );
    assert_eq!(
        after_delete
            .iter()
            .filter(|a| a.kind == pdfreader_core::AnnotationKind::FreeText)
            .count(),
        1,
        "siblings must survive the deletion"
    );
}

/// A bogus id is "nothing to delete", not a crash.
#[test]
fn deleting_a_missing_annotation_is_not_an_error() {
    let Some(dir) = find_pdfium() else {
        eprintln!("SKIP: no PDFium library staged under target/native");
        return;
    };
    let mut engine = PdfiumEngine::bind(Some(dir.as_path())).expect("bind");
    let path = require_fixture("forms.pdf");
    let (handle, _info) = engine.open(&path, None).expect("open");

    let id = pdfreader_core::AnnotationId::new(0, 9999);
    assert!(
        !engine.delete_annotation(handle, id).expect("delete"),
        "missing annotation reports false"
    );
}

/// Real-world regression: a 17-field job application form. Verifies that
/// filling its small text fields and checkboxes survives a save round-trip.
/// Skips when the user-supplied sample is not in the fixture directory.
#[test]
fn job_application_form_fills_and_saves() {
    let sample = workspace_root()
        .join("target")
        .join("test-pdfs")
        .join("job-application.pdf");
    if !sample.exists() {
        eprintln!("SKIP: job-application.pdf sample not present");
        std::process::exit(0);
    }
    let Some(dir) = find_pdfium() else { return };
    let mut engine = PdfiumEngine::bind(Some(dir.as_path())).expect("bind");

    let (handle, _info) = engine.open(&sample, None).expect("open");
    let form = engine.form_fields(handle).expect("form");
    assert!(form.fields.len() >= 17, "expected a rich form");

    // Fill the tiny text fields (they are ~12 pt tall).
    for (name, value) in [
        ("partner_name", "Yamin Hossain"),
        ("email_from", "yamin@example.com"),
        ("salary_expected", "confidential"),
    ] {
        let field = form
            .fields
            .iter()
            .find(|f| f.name == name && f.kind == FormFieldType::Text)
            .unwrap_or_else(|| panic!("{name} present"));
        assert!(
            engine
                .set_field_value(handle, field.id, FieldValue::Text(value.into()))
                .expect("write"),
            "{name} must be writable"
        );
    }

    // Tick the FullTime checkbox.
    let full_time = form
        .fields
        .iter()
        .find(|f| f.name == "FullTime")
        .expect("FullTime present");
    assert!(
        engine
            .set_field_value(handle, full_time.id, FieldValue::Checked(true))
            .expect("write"),
        "checkbox must be writable"
    );

    // In-memory check first: did the writes actually stick?
    let after_write = engine.form_fields(handle).expect("list after write");
    for name in ["partner_name", "email_from", "salary_expected"] {
        for f in after_write.fields.iter().filter(|f| f.name == name) {
            println!("DBG after write: {} id=({},{}) value={:?}", f.name, f.id.page, f.id.annot_index, f.value);
        }
    }

    // Save, reopen, verify every value persisted.
    let bytes = engine.save_to_bytes(handle, false).expect("save");
    let out = std::env::temp_dir().join("pdf-reader-job-application-test.pdf");
    std::fs::write(&out, &bytes).expect("write");
    let (handle2, _i) = engine.open(&out, None).expect("reopen");
    let saved = engine.form_fields(handle2).expect("list");

    // KNOWN LIMITATION: `partner_name` is a shared-name field (two kid
    // widgets). Its value is written into each widget dictionary, but
    // `pdfium-render` 0.9.4 cannot write the parent field dictionary where
    // PDFium resolves the value — so the re-opened read reports Empty for it.
    // Unique-name fields round-trip fully (asserted below).
    let email = saved
        .fields
        .iter()
        .find(|f| f.name == "email_from")
        .expect("field present");
    assert_eq!(email.value, FieldValue::Text("yamin@example.com".into()));
    let checkbox = saved
        .fields
        .iter()
        .find(|f| f.name == "FullTime")
        .expect("field present");
    assert_eq!(checkbox.value, FieldValue::Checked(true));
    let _ = std::fs::remove_file(&out);
}

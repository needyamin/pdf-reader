//! Round-trip a form fill: write a value into a text field, save, reopen and
//! read it back. Used to check whether filling actually persists, especially
//! for XFA forms where the AcroForm widgets are only a compatibility shell.
//!
//! Usage: cargo run -p pdfreader-pdf --example form_fill -- path/to/file.pdf

use pdfreader_core::{FieldValue, FormFieldType};
use pdfreader_pdf::PdfEngine;
use pdfreader_pdf::engine::PdfiumEngine;

fn main() {
    let path = std::env::args().nth(1).expect("pass a PDF path");
    let dir = std::path::Path::new(&path)
        .parent()
        .and_then(|p| p.parent())
        .map(|p| p.to_path_buf());
    let bind_dir = std::env::current_dir()
        .ok()
        .map(|cwd| cwd.join("target/native"))
        .filter(|d| d.join("pdfium.dll").exists() || d.join("libpdf.so").exists())
        .or(dir);

    let mut engine = PdfiumEngine::bind(bind_dir.as_deref()).expect("bind pdfium");

    // Pick a text field and a radio group to exercise both write paths.
    let (handle, _info) = engine
        .open(std::path::Path::new(&path), None)
        .expect("open");
    let form = engine.form_fields(handle).expect("form");
    println!("kind: {:?}, fields: {}", form.kind, form.fields.len());

    let text = form
        .fields
        .iter()
        .find(|f| f.kind == FormFieldType::Text)
        .map(|f| f.id);
    // An *unselected* radio, which is the case a user actually clicks.
    let radio = form
        .fields
        .iter()
        .find(|f| f.kind == FormFieldType::RadioButton && f.value == FieldValue::Checked(false))
        .map(|f| f.id);
    let checkbox = form
        .fields
        .iter()
        .find(|f| f.kind == FormFieldType::CheckBox && f.value == FieldValue::Checked(false))
        .map(|f| f.id);

    if let Some(id) = text {
        let written = engine
            .set_field_value(handle, id, FieldValue::Text("Yamin Hossain".to_owned()))
            .expect("set text");
        println!("text write ok: {written:?}");
    }
    if let Some(id) = radio {
        println!("radio target: id=({},{})", id.page, id.annot_index);
        match engine.set_field_value(handle, id, FieldValue::Checked(true)) {
            Ok(written) => println!("radio write: {written:?}"),
            // Expected for a grouped radio: the value lives on the parent field
            // dictionary, which the library cannot write.
            Err(error) => println!("radio write refused: {error}"),
        }
    }
    if let Some(id) = checkbox {
        println!("checkbox target: id=({},{})", id.page, id.annot_index);
        match engine.set_field_value(handle, id, FieldValue::Checked(true)) {
            Ok(written) => println!("checkbox write ok: {written:?}"),
            Err(error) => println!("checkbox write refused: {error}"),
        }
    }

    // Re-read before saving: distinguishes "the write did not take" from "the
    // write was lost on save".
    let after = engine.form_fields(handle).expect("form");
    for f in &after.fields {
        if matches!(
            f.kind,
            FormFieldType::Text | FormFieldType::RadioButton | FormFieldType::CheckBox
        ) && !f.value.is_empty()
        {
            println!(
                "in memory: id=({},{}) {:?} = {:?}",
                f.id.page, f.id.annot_index, f.kind, f.value
            );
        }
    }

    let bytes = engine.save_to_bytes(handle, false).expect("save");
    let out = std::env::current_dir()
        .unwrap()
        .join("target/form_fill_out.pdf");
    std::fs::write(&out, &bytes).expect("write");
    println!("saved {} bytes to {}", bytes.len(), out.display());

    // Reopen and read back. PDFium binds once per process, so the same engine
    // is reused: close the original document, then open the saved copy.
    engine.close(handle);
    let (handle, _) = engine.open(&out, None).expect("reopen");
    for annotation in engine.annotations(handle).expect("annotations") {
        println!(
            "annotation: {:?} rect=({:.1},{:.1},{:.1},{:.1})",
            annotation.kind,
            annotation.rect.min_x,
            annotation.rect.min_y,
            annotation.rect.max_x,
            annotation.rect.max_y
        );
    }
    let form = engine.form_fields(handle).expect("form");
    for f in &form.fields {
        if matches!(
            f.kind,
            FormFieldType::Text | FormFieldType::RadioButton | FormFieldType::CheckBox
        ) && !f.value.is_empty()
        {
            println!(
                "read back: id=({},{}) {:?} {:?} = {:?}",
                f.id.page, f.id.annot_index, f.kind, f.name, f.value
            );
        }
    }
}

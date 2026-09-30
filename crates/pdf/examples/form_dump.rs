//! Dump the form fields of a PDF: names, types, values, geometry.
//!
//! Usage: cargo run -p pdfreader-pdf --example form_dump -- path/to/file.pdf

use pdfreader_pdf::engine::PdfiumEngine;
use pdfreader_pdf::PdfEngine;

fn main() {
    let path = std::env::args().nth(1).expect("pass a PDF path");
    let dir = std::path::Path::new(&path)
        .parent()
        .and_then(|p| p.parent())
        .map(|p| p.to_path_buf());
    // Locate pdfium.dll next to the usual staging layout, or fall back to the
    // workspace target/native.
    let bind_dir = std::env::current_dir()
        .ok()
        .map(|cwd| cwd.join("target/native"))
        .filter(|d| d.join("pdfium.dll").exists() || d.join("libpdfium.so").exists())
        .or(dir);

    let mut engine = PdfiumEngine::bind(bind_dir.as_deref()).expect("bind pdfium");
    let (handle, info) = engine
        .open(std::path::Path::new(&path), None)
        .expect("open");
    println!("pages: {}", info.page_count());
    let form = engine.form_fields(handle).expect("form");
    println!("kind: {:?}, fields: {}", form.kind, form.fields.len());
    for f in &form.fields {
        println!(
            "{:?} id=({},{}) name={:?} value={:?} rect=({:.0},{:.0},{:.0},{:.0}) ro={} editable={} da_hint={}",
            f.kind,
            f.id.page,
            f.id.annot_index,
            f.name,
            f.value,
            f.rect.min_x,
            f.rect.min_y,
            f.rect.max_x,
            f.rect.max_y,
            !f.is_editable(),
            f.is_editable(),
            f.options.len(),
        );
    }
}

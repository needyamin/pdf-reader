//! `xtask inspect` backend: dump a PDF's structure with the real engine.
//!
//! Opens the document through `PdfiumEngine` exactly as the application would,
//! then prints page count, page geometry, encryption status and the bookmark
//! tree. This is the Phase 3 "open any PDF, get structure out" verification
//! target, and it doubles as a quick corpus smoke check.

use std::path::PathBuf;
use std::process::exit;

use pdfreader_core::OutlineNode;
use pdfreader_pdf::DocumentInfo;
use pdfreader_pdf::engine::{PdfEngine, PdfiumEngine};

fn main() {
    let mut pdf: Option<PathBuf> = None;
    let mut pdfium_dir: Option<PathBuf> = None;
    let mut password: Option<String> = None;

    let args: Vec<String> = std::env::args().skip(1).collect();
    let mut i = 0;
    while i < args.len() {
        match args[i].as_str() {
            "--pdfium-dir" => {
                pdfium_dir = Some(PathBuf::from(&args[i + 1]));
                i += 2;
            }
            "--password" => {
                password = Some(args[i + 1].clone());
                i += 2;
            }
            other => {
                if pdf.is_none() {
                    pdf = Some(PathBuf::from(other));
                }
                i += 1;
            }
        }
    }

    let Some(pdf) = pdf else {
        eprintln!("usage: inspect <pdf> [--pdfium-dir DIR] [--password P]");
        exit(2);
    };

    let mut engine = match PdfiumEngine::bind(pdfium_dir.as_deref()) {
        Ok(engine) => engine,
        Err(err) => {
            eprintln!("could not load PDFium: {err}");
            exit(1);
        }
    };

    match engine.open(&pdf, password.as_deref()) {
        Ok((_handle, info)) => print_info(&pdf, &info),
        Err(err) => {
            eprintln!("could not open {}: {err}", pdf.display());
            exit(1);
        }
    }
}

fn print_info(path: &std::path::Path, info: &DocumentInfo) {
    println!("file:      {}", path.display());
    println!("pages:     {}", info.pages.len());
    println!("encrypted: {}", info.encrypted);

    if let Some(first) = info.pages.first() {
        println!(
            "page size: {:.2} x {:.2} pt (first page)",
            first.width_pt, first.height_pt
        );
    }

    println!(
        "outline:   {} top-level entr(y/ies)",
        info.outline.root.len()
    );
    for node in &info.outline.root {
        print_node(node, 0);
    }
}

fn print_node(node: &OutlineNode, depth: usize) {
    let indent = "  ".repeat(depth);
    let target = node
        .page
        .map(|page| format!(" -> page {}", page + 1))
        .unwrap_or_default();
    println!("{indent}- {}{}", node.title, target);

    for child in &node.children {
        print_node(child, depth + 1);
    }
}
